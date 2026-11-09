//! PHP target conformance checks, with optional exports for reference runtimes.

#[path = "integration/common/mod.rs"]
mod common;

use std::path::Path;
use std::time::Duration;

use phpantom_lsp::{Backend, types::PhpVersion};
use serde_json::{Value, json};
use tower_lsp_server::{LanguageServer, gen_lsp_types::*};

fn audit() -> Value {
    serde_json::from_str(include_str!("../docs/compatibility/php55-php56-audit.json")).unwrap()
}

fn source<'a>(audit: &'a Value, id: &str) -> &'a str {
    audit["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing audit source: {id}"))["source"]
        .as_str()
        .unwrap()
}

async fn project(version: PhpVersion) -> (tempfile::TempDir, Backend) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".phpantom.toml"),
        format!("[php]\nversion = \"{}.{}\"\n", version.major, version.minor),
    )
    .unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let backend = Backend::new_test_with_full_stubs();
    common::initialize_with_options(&backend, &root, None).await;
    assert_eq!(backend.php_version(), version);
    (dir, backend)
}

async fn open(backend: &Backend, root: &Path, id: &str, text: &str) -> Uri {
    let path = root.join(format!("{id}.php"));
    std::fs::write(&path, text).unwrap();
    let uri = common::file_uri(&dunce::canonicalize(path).unwrap())
        .parse()
        .unwrap();
    common::open_php(backend, &uri, text).await;
    uri
}

async fn ready_diagnostics(backend: &Backend, uri: &Uri) -> Vec<Diagnostic> {
    tokio::time::timeout(Duration::from_secs(30), async {
        backend.publish_test_diagnostics(uri.as_str()).await;
        let report = backend
            .diagnostic(DocumentDiagnosticParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                identifier: None,
                previous_result_id: None,
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .await
            .unwrap();
        if let DocumentDiagnosticReport::RelatedFullDocumentDiagnosticReport(report) = report {
            let report = report.full_document_diagnostic_report;
            if report.result_id.as_deref().is_some_and(|id| id != "0") {
                return report.items;
            }
        }
        panic!("missing completed diagnostic report: {uri}");
    })
    .await
    .unwrap_or_else(|_| panic!("diagnostics never became ready: {uri}"))
}

async fn format(backend: &Backend, uri: &Uri, text: &str, range: bool) -> String {
    let options = FormattingOptions {
        tab_size: 4,
        insert_spaces: true,
        ..Default::default()
    };
    let edits = if range {
        let offset = text
            .rmatch_indices("$probe")
            .find(|(offset, name)| text[offset + name.len()..].trim_start().starts_with('='))
            .expect("the range fixture must assign $probe")
            .0;
        backend
            .range_formatting(DocumentRangeFormattingParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range::new(
                    common::offset_position(text, offset),
                    common::offset_position(text, offset + "$probe".len()),
                ),
                options,
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
    } else {
        backend
            .formatting(DocumentFormattingParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                options,
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
    }
    .unwrap()
    .unwrap_or_default();
    common::apply_edits(text, &edits)
}

fn runtime_case(id: &str, text: &str, value: &Value) -> Value {
    json!({"id": id, "source": text, "lint": true, "runtime": {
        "probe": "$probe", "expected": value
    }})
}

#[test]
fn parser_preserves_versioned_expression_structure_and_offsets() {
    use mago_span::HasSpan;
    use mago_syntax::cst::{BinaryOperator, Expression, Statement};
    use mago_syntax::settings::ParserSettings;

    let text = "<?php\r\n// 한글 😀\r\n'10' . 1 + 2;\r\n";
    for version in [
        Some((5, 5)),
        Some((5, 6)),
        Some((7, 4)),
        Some((8, 0)),
        Some((8, 5)),
        None,
    ] {
        let arena = mago_allocator::LocalArena::new();
        let mut settings = ParserSettings::default();
        if let Some((major, minor)) = version {
            settings.php_version = mago_php_version::PHPVersion::new(major, minor, 0);
        }
        let program = mago_syntax::parser::parse_file_content_with_settings(
            &arena,
            mago_database::file::FileId::new(b"precedence.php"),
            text.as_bytes(),
            settings,
        );
        assert!(
            program.errors.is_empty(),
            "{version:?}: {:?}",
            program.errors
        );
        let expression = program
            .statements
            .iter()
            .find_map(|statement| match statement {
                Statement::Expression(statement) => Some(statement.expression),
                _ => None,
            })
            .unwrap();
        let span = expression.span();
        assert_eq!(
            &text[span.start.offset as usize..span.end.offset as usize],
            "'10' . 1 + 2"
        );
        let Expression::Binary(binary) = expression else {
            panic!("expected a binary expression")
        };
        if version.is_some_and(|(major, _)| major < 8) {
            assert!(matches!(binary.operator, BinaryOperator::Addition(_)));
            assert!(
                matches!(binary.lhs, Expression::Binary(child) if matches!(child.operator, BinaryOperator::StringConcat(_)))
            );
        } else {
            assert!(matches!(binary.operator, BinaryOperator::StringConcat(_)));
            assert!(
                matches!(binary.rhs, Expression::Binary(child) if matches!(child.operator, BinaryOperator::Addition(_)))
            );
        }
    }
}

#[test]
fn formatter_custom_parser_settings_retain_target_version() {
    use std::borrow::Cow;

    let arena = mago_allocator::LocalArena::new();
    let formatter = mago_formatter::Formatter::new(
        &arena,
        mago_php_version::PHPVersion::new(5, 5, 0),
        Default::default(),
    )
    .with_parser_settings(Default::default());
    let formatted = formatter
        .format_code(
            Cow::Borrowed(b"precedence.php"),
            Cow::Borrowed(b"<?php $probe='10' . 1 + 2;"),
        )
        .unwrap();
    assert!(
        std::str::from_utf8(formatted)
            .unwrap()
            .contains("$probe = '10' . 1 + 2;")
    );
}

async fn check_controls(backend: &Backend, root: &Path, audit: &Value, exported: &mut Vec<Value>) {
    let expectations: Value =
        serde_json::from_str(include_str!("compatibility/expectations.json")).unwrap();
    for case in expectations["controls"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        for (ending, newline) in [("lf", "\n"), ("crlf", "\r\n")] {
            let id = format!("{id}_{ending}");
            let text = source(audit, case["id"].as_str().unwrap()).replace('\n', newline);
            let uri = open(backend, root, &id, &text).await;
            let diagnostics = ready_diagnostics(backend, &uri).await;
            assert!(diagnostics.is_empty(), "{id}: {diagnostics:?}");
            let position = common::position_of_last(&text, "$probe;");
            let hover =
                common::markup_hover_at(backend, &uri, position.line, position.character + 2).await;
            assert!(
                hover.contains(&format!("$probe = {}\n", case["type"].as_str().unwrap())),
                "{id}: {hover}"
            );
            if case["id"] == "trait_alias" {
                let position = common::position_of_last(&text, "aliasValue()");
                let target = common::goto_definition_at(
                    backend,
                    &uri,
                    position.line,
                    position.character + 2,
                )
                .await
                .unwrap();
                let target = serde_json::to_value(target).unwrap();
                let target = target.as_array().map_or(&target, |items| &items[0]);
                assert_eq!(target["uri"], uri.as_str());
                assert_eq!(
                    target["range"]["start"],
                    json!(common::position_of(&text, "value()"))
                );
            }
            exported.push(runtime_case(&id, &text, &case["value"]));
            for range in [false, true] {
                let formatted = format(backend, &uri, &text, range).await;
                let formatted_id = format!("{id}_{}", if range { "range" } else { "document" });
                assert_ne!(
                    formatted, text,
                    "{formatted_id}: the unformatted control must receive an edit"
                );
                let formatted_uri = open(backend, root, &formatted_id, &formatted).await;
                assert_eq!(
                    format(backend, &formatted_uri, &formatted, range).await,
                    formatted,
                    "{formatted_id}: formatting must be idempotent"
                );
                exported.push(runtime_case(&formatted_id, &formatted, &case["value"]));
            }
        }
    }
}

async fn check_generation(
    backend: &Backend,
    root: &Path,
    version: PhpVersion,
    exported: &mut Vec<Value>,
) {
    for (id, property, is_static) in [
        ("untyped", "private $name;", false),
        ("documented", "/** @var string */\nprivate $name;", false),
        ("nullable", "/** @var ?string */\nprivate $name;", false),
        ("static", "/** @var string */\nprivate static $name;", true),
    ] {
        let text = format!("<?php\nclass CompatibilityAccessor {{\n{property}\n}}\n")
            .replace('\n', "\r\n");
        let uri = open(backend, root, &format!("generate_{id}"), &text).await;
        let pos = common::position_of(&text, "$name");
        let actions = common::code_actions_via_server(backend, &uri, Range::new(pos, pos))
            .await
            .unwrap();
        for (title, available) in [
            (
                "Generate promoted constructor",
                !is_static && version >= PhpVersion::new(8, 0),
            ),
            (
                "Generate get hook",
                !is_static && version >= PhpVersion::new(8, 4),
            ),
        ] {
            assert_eq!(
                common::find_action_titled(&actions, title).is_some(),
                available,
                "{id}: {title}"
            );
        }
        for title in [
            "Generate constructor",
            "Generate getter",
            "Generate setter",
            "Generate getter and setter",
        ] {
            if is_static && title == "Generate constructor" {
                continue;
            }
            let action = common::find_action_titled(&actions, title)
                .unwrap_or_else(|| panic!("{id}: missing {title}"));
            let edited = common::apply_workspace_edit(&text, action.edit.as_ref().unwrap());
            let setup = if title == "Generate constructor" {
                "$object = new CompatibilityAccessor('checked');"
            } else if title.contains("setter") {
                if is_static {
                    "CompatibilityAccessor::setName('checked'); $object = new CompatibilityAccessor();"
                } else {
                    "$object = new CompatibilityAccessor(); if ($object->setName('checked') !== $object) { throw new Exception('setter must return its receiver'); }"
                }
            } else {
                "$object = new CompatibilityAccessor();"
            };
            let read = if title.contains("getter") {
                if is_static {
                    "$probe = CompatibilityAccessor::getName();"
                } else {
                    "$probe = $object->getName();"
                }
            } else {
                "$property = new ReflectionProperty('CompatibilityAccessor', 'name'); $property->setAccessible(true); $probe = $property->getValue($object);"
            };
            let expected = if title == "Generate getter" {
                json!(["NULL", null])
            } else {
                json!(["string", "checked"])
            };
            exported.push(runtime_case(
                &format!("generate_{id}_{}", title.replace(' ', "_")),
                &format!("{edited}\n{setup}\n{read}\n"),
                &expected,
            ));
        }
    }
}

async fn check_formatting(
    backend: &Backend,
    root: &Path,
    version: PhpVersion,
    exported: &mut Vec<Value>,
) {
    let fixtures: Value =
        serde_json::from_str(include_str!("compatibility/formatting.json")).unwrap();
    for case in fixtures.as_array().unwrap() {
        if case["maximum_major"]
            .as_u64()
            .is_some_and(|major| u64::from(version.major) > major)
        {
            continue;
        }
        let expected = if version.major == 5 && !case["php5"].is_null() {
            &case["php5"]
        } else if version.major < 8 && !case["pre8"].is_null() {
            &case["pre8"]
        } else {
            &case["value"]
        };
        for (ending, newline) in [("lf", "\n"), ("crlf", "\r\n")] {
            let id = format!("format_{}_{ending}", case["id"].as_str().unwrap());
            let text = format!(
                "{}\n{}\n// before selection\n$probe={};\n// after selection\n",
                case["tag"].as_str().unwrap_or("<?php"),
                case["setup"].as_str().unwrap_or(""),
                case["expression"].as_str().unwrap(),
            )
            .replace('\n', newline);
            let uri = open(backend, root, &id, &text).await;
            exported.push(runtime_case(&id, &text, expected));
            for range in [false, true] {
                let formatted_id = format!("{id}_{}", if range { "range" } else { "document" });
                let formatted = format(backend, &uri, &text, range).await;
                assert_ne!(
                    formatted, text,
                    "{formatted_id}: expected a formatting edit"
                );
                if range {
                    let prefix = text.split("$probe=").next().unwrap();
                    assert!(
                        formatted.starts_with(prefix),
                        "{formatted_id}: changed preceding lines"
                    );
                    assert!(
                        formatted.ends_with(&format!("// after selection{newline}")),
                        "{formatted_id}: changed following lines"
                    );
                }
                let expression_key = if version.major < 8 {
                    "pre8_expression"
                } else {
                    "modern_expression"
                };
                if let Some(expression) = case[expression_key].as_str() {
                    assert!(
                        formatted.contains(&format!("$probe = {expression};")),
                        "{formatted_id}: {formatted}"
                    );
                }
                let formatted_uri = open(backend, root, &formatted_id, &formatted).await;
                assert_eq!(
                    format(backend, &formatted_uri, &formatted, range).await,
                    formatted,
                    "{formatted_id}: formatting must be idempotent"
                );
                exported.push(runtime_case(&formatted_id, &formatted, expected));
            }
        }
    }
}

async fn check_calls_and_boundaries(
    backend: &Backend,
    root: &Path,
    version: PhpVersion,
    audit: &Value,
    exported: &mut Vec<Value>,
) {
    let text = source(audit, "namespace_alias").replace("$probe;", "$probe->format('c');");
    let uri = open(backend, root, "completion_alias", &text).await;
    let pos = common::position_after(&text, "$probe->");
    let items = common::response_items(
        common::completion_response(backend, &uri, pos.line, pos.character, None).await,
    )
    .unwrap();
    assert!(
        items
            .iter()
            .any(|item| item.filter_text.as_deref() == Some("format")),
        "a namespace alias must retain DateTime completion"
    );

    let text = source(audit, "modern_constructor_control");
    let uri = open(backend, root, "constructor_signature", text).await;
    let pos = common::position_after(text, "new AuditModernCtor(");
    let signature = backend
        .signature_help(
            serde_json::from_value(json!({
                "textDocument": { "uri": uri }, "position": pos
            }))
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(signature.signatures.len(), 1);
    assert!(signature.signatures[0].label.contains("DateTime $value"));
    let diagnostics = ready_diagnostics(backend, &uri).await;
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == Some(Code::String("argument_count_mismatch".into()))),
        "constructor arguments must be checked: {diagnostics:?}"
    );

    for (id, minimum) in [
        ("variadic", PhpVersion::new(5, 6)),
        ("return_type", PhpVersion::new(7, 0)),
    ] {
        if version < minimum {
            continue;
        }
        let text = source(audit, id);
        let uri = open(backend, root, id, text).await;
        let diagnostics = ready_diagnostics(backend, &uri).await;
        assert!(
            diagnostics.is_empty(),
            "{id} is valid on {version:?}: {diagnostics:?}"
        );
        exported.push(json!({"id": format!("boundary_{id}"), "source": text, "lint": true}));
    }
}

#[tokio::test]
async fn legacy_compatibility_matrix() {
    let audit = audit();
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 4),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let mut cases = Vec::new();
        // Reference lint results are independent of PHPantom's diagnostic snapshots.
        if version.major == 5 {
            let key = format!("php{}{}", version.major, version.minor);
            for case in audit["cases"].as_array().unwrap() {
                cases.push(json!({"id": format!("syntax_{}", case["id"].as_str().unwrap()), "source": case["source"], "lint": case[&key]["reference"]["lintExit"] == 0}));
            }
        }
        check_controls(&backend, dir.path(), &audit, &mut cases).await;
        check_generation(&backend, dir.path(), version, &mut cases).await;
        check_calls_and_boundaries(&backend, dir.path(), version, &audit, &mut cases).await;
        check_formatting(&backend, dir.path(), version, &mut cases).await;
        if let Some(output) = std::env::var_os("PHPANTOM_COMPAT_OUTPUT") {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(Path::new(&output).join(format!("{}.{}.json", version.major, version.minor)), serde_json::to_vec_pretty(&json!({"target": format!("{}.{}", version.major, version.minor), "cases": cases})).unwrap()).unwrap();
        }
    }
}

#[tokio::test]
async fn legacy_source_forms_have_no_syntax_errors() {
    let audit = audit();
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        for id in [
            "curly_offset",
            "reserved_match_name",
            "hash_bracket_comment",
            "asp_tag",
        ] {
            let uri = open(&backend, dir.path(), id, source(&audit, id)).await;
            let diagnostics = ready_diagnostics(&backend, &uri).await;
            assert!(
                !diagnostics
                    .iter()
                    .any(|d| d.code == Some(Code::String("syntax_error".into()))),
                "{id}: {diagnostics:?}"
            );
        }
    }
}

#[test]
fn legacy_lexer_boundaries_and_original_spans() {
    use mago_span::HasSpan;
    use mago_syntax::{cst::Statement, settings::ParserSettings};
    for (major, minor) in [(5, 5), (5, 6), (7, 3), (7, 4), (8, 0), (8, 1), (8, 5)] {
        for (name, since) in [
            ("Match", (8, 0)),
            ("fn", (7, 4)),
            ("Enum", (8, 1)),
            ("Readonly", (8, 1)),
        ] {
            let text = format!("<?php /* 한글 😀 */ class {name} {{}}");
            let arena = mago_allocator::LocalArena::new();
            let settings = ParserSettings {
                php_version: mago_php_version::PHPVersion::new(major, minor, 0),
                ..Default::default()
            };
            let program = mago_syntax::parser::parse_file_content_with_settings(
                &arena,
                mago_database::file::FileId::new(b"legacy.php"),
                text.as_bytes(),
                settings,
            );
            if (major, minor) < since {
                assert!(
                    program.errors.is_empty(),
                    "{major}.{minor} {name}: {:?}",
                    program.errors
                );
                let class = program
                    .statements
                    .iter()
                    .find_map(|s| {
                        if let Statement::Class(c) = s {
                            Some(c)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let span = class.name.span();
                assert_eq!(
                    &text[span.start.offset as usize..span.end.offset as usize],
                    name
                );
            }
        }
    }
}

#[test]
fn source_lexer_keeps_modern_syntax_and_tag_contents_distinct() {
    use mago_syntax::{cst::DirectVariable, settings::ParserSettings, walker::Walker};
    struct Variables;
    impl<'ast, 'arena> Walker<'ast, 'arena, usize> for Variables {
        fn walk_in_direct_variable(&self, _: &'ast DirectVariable<'arena>, count: &mut usize) {
            *count += 1;
        }
    }
    let text = "<p>$html</p><% $asp; // comment %><%= $echo %><script language=php>$script;</script><? $short ?><?php $full; #[Marker] class Tagged {}\n$tail; ?>";
    for (major, expected) in [(5, 6), (8, 3)] {
        let settings = ParserSettings {
            php_version: mago_php_version::PHPVersion::new(major, 5, 0),
            ..Default::default()
        };
        let arena = mago_allocator::LocalArena::new();
        let program = mago_syntax::parser::parse_file_content_with_settings(
            &arena,
            mago_database::file::FileId::new(b"tags.php"),
            text.as_bytes(),
            settings,
        );
        assert!(program.errors.is_empty(), "{:?}", program.errors);
        let mut variables = 0;
        Variables.walk_program(program, &mut variables);
        let classes = program
            .statements
            .iter()
            .filter(|s| matches!(s, mago_syntax::cst::Statement::Class(_)))
            .count();
        assert_eq!(variables, expected);
        assert_eq!(classes, usize::from(major >= 8));
    }
    let arena = mago_allocator::LocalArena::new();
    let text = b"<?php #[Attribute] class Marker {} enum Kind { case One; } readonly class Model {} $fn = fn() => match (1) { 1 => new Model() };";
    let program = mago_syntax::parser::parse_file_content(
        &arena,
        mago_database::file::FileId::new(b"modern.php"),
        text,
    );
    assert!(program.errors.is_empty(), "{:?}", program.errors);
}

#[tokio::test]
async fn legacy_source_forms_keep_types_navigation_and_recovery() {
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        let mut exported = Vec::new();
        for (id, opening, closing) in [
            ("full", "<?php", "?>"),
            ("short", "<?", "?>"),
            ("asp", "<%", "%>"),
            ("script", "<ScRiPt\tlanguage = 'PHP' >", "</sCrIpT >"),
        ] {
            for newline in ["\n", "\r\n"] {
                let text = format!("<p>한글 😀</p>\n{opening}\n#[ordinary comment, not an attribute\nclass Match {{\n    /** @return self 한글 😀 */ public function next() {{ return $this; }}\n}}\n$values = array(new Match());\n$probe = $values{{0}};\n/* 한글 😀 */ $probe->next();\n$values{{0}}->next();\n{closing}<p>끝</p>").replace('\n', newline);
                let uri = open(&backend, dir.path(), id, &text).await;
                for (kind, source) in [
                    ("original", text.clone()),
                    ("formatted", format(&backend, &uri, &text, false).await),
                ] {
                    exported.push(runtime_case(
                        &format!("{id}_{kind}_{}", newline.len()),
                        &format!("{source}<?php $probe = get_class($probe);"),
                        &json!(["string", "Match"]),
                    ));
                }
                let diagnostics = ready_diagnostics(&backend, &uri).await;
                assert!(diagnostics.is_empty(), "{id}: {diagnostics:?}");
                let pos = common::position_of(&text, "$probe->next");
                let hover =
                    common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
                assert!(hover.contains("Match"), "{id}: {hover}");
                for call in ["$probe->next", "$values{0}->next"] {
                    let offset = text.find(call).unwrap() + call.len() - 4;
                    let pos = common::offset_position(&text, offset);
                    let target =
                        common::goto_definition_at(&backend, &uri, pos.line, pos.character + 1)
                            .await
                            .expect("legacy member definition");
                    let target = serde_json::to_value(target).unwrap();
                    let target = target.as_array().map_or(&target, |items| &items[0]);
                    assert_eq!(target["uri"], uri.as_str());
                    assert_eq!(
                        target["range"]["start"],
                        json!(common::position_of(&text, "next()"))
                    );
                }
                // Reparse after incomplete edits must retain the earlier class and its grammar.
                let cut = text.find("$probe->next").unwrap();
                for subject in ["$probe->", "$values{0}->"] {
                    let edited = format!("{}{subject}", &text[..cut]);
                    let uri = open(&backend, dir.path(), id, &edited).await;
                    let pos = common::offset_position(&edited, edited.len());
                    let response =
                        common::completion_response(&backend, &uri, pos.line, pos.character, None)
                            .await;
                    let response = serde_json::to_value(response).unwrap();
                    assert!(
                        response.to_string().contains("next"),
                        "{id} {subject}: {response}"
                    );
                }
            }
        }
        if let Some(output) = std::env::var_os("PHPANTOM_COMPAT_OUTPUT") {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(Path::new(&output).join(format!("sources-{}.{}.json", version.major, version.minor)), serde_json::to_vec_pretty(&json!({"target": format!("{}.{}", version.major, version.minor), "cases": exported})).unwrap()).unwrap();
        }
    }
}

#[tokio::test]
async fn legacy_unopened_files_are_discovered_and_resolved_by_namespace() {
    for composer in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join(".phpantom.toml"), "[php]\nversion = '5.5'\n").unwrap();
        if composer {
            std::fs::write(
                root.join("composer.json"),
                r#"{"autoload":{"psr-4":{"Legacy\\":"src/"}}}"#,
            )
            .unwrap();
        }
        std::fs::create_dir(root.join("src")).unwrap();
        let declaration = "<?php\r\n#[not an attribute\r\nnamespace Legacy;\r\n#[ isn't an attribute: ' class Phantom {}\r\nclass Match { /** @return self */ public function next() { return $this; } }\r\n";
        std::fs::write(root.join("src/Match.php"), declaration).unwrap();
        std::fs::write(
            root.join("Global.php"),
            "<?php class Match { function wrong() {} }",
        )
        .unwrap();
        let backend = Backend::new_test_with_full_stubs();
        common::initialize_with_options(&backend, &root, None).await;
        let text = "<?php\nnamespace Caller;\nuse Legacy\\Match as Item;\n$values = array(new Item());\n$probe = $values{0};\n$probe->next();\n";
        let uri = open(&backend, &root, "caller", text).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        assert!(
            diagnostics.is_empty(),
            "composer={composer}: {diagnostics:?}"
        );
        let pos = common::position_of(text, "next()");
        let targets = common::definition_locations(
            common::goto_definition_at(&backend, &uri, pos.line, pos.character + 1).await,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].uri.as_str(),
            common::file_uri(&dunce::canonicalize(root.join("src/Match.php")).unwrap())
        );
        assert_eq!(
            targets[0].range.start,
            common::position_of(declaration, "next()")
        );
    }
}

#[test]
fn legacy_class_types_are_distinct_from_scalars() {
    use phpantom_lsp::{atom::atom, php_type::PhpType};
    for name in [
        "int", "float", "string", "bool", "iterable", "object", "mixed", "void", "never",
        "resource", "true", "false", "null",
    ] {
        let class = PhpType::class_named(atom(name));
        let scalar = PhpType::parse(name);
        assert_ne!(class, scalar, "{name}");
        assert!(!class.equivalent(&scalar), "{name}");
        assert!(class.is_object_like(), "{name}");
        assert!(!class.is_scalar(), "{name}");
        assert_eq!(class.base_name(), Some(name));
        assert_eq!(class.truthiness(), Some(true));
        assert!(class.is_subtype_of(&PhpType::object()));
        assert!(!PhpType::int().is_subtype_of(&class));
        assert!(
            class.equivalent(&PhpType::parse(&class.to_string())),
            "{name}"
        );
        let qualified = class.resolve_names(&|name| format!("App\\{name}"));
        assert_eq!(
            qualified.class_name(),
            Some(format!("App\\{name}").as_str())
        );
        assert_eq!(qualified.shorten().to_string(), format!("\\App\\{name}"));
    }
    let union = PhpType::union(vec![PhpType::int(), PhpType::class_named(atom("int"))]);
    assert_eq!(union.to_string(), "int|\\int");
    assert!(PhpType::parse(&union.to_string()).equivalent(&union));
    let late_static = PhpType::static_type(atom("int"));
    assert!(!late_static.is_subtype_of(&PhpType::int()));
    assert!(late_static.is_subtype_of(&PhpType::class_named(atom("int"))));
    assert!(late_static.is_object_like());
}

#[tokio::test]
async fn legacy_closure_hints_and_late_static_returns_keep_class_identity() {
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        let text = "<?php\nclass int { /** @return static */ function again() { return $this; } }\nfunction take(int $value) {}\n$call = function (int $value) { take($value); take(1); $value->again()->again(); };\n$call(new int());\n";
        let uri = open(&backend, dir.path(), "closure", text).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        assert_eq!(diagnostics.len(), 1, "{version}: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].range.start.line, 3,
            "{version}: {diagnostics:?}"
        );
        let pos = common::position_of_last(text, "again()");
        let targets = common::definition_locations(
            common::goto_definition_at(&backend, &uri, pos.line, pos.character + 1).await,
        );
        assert_eq!(targets.len(), 1, "{version}: {targets:?}");
        assert_eq!(targets[0].range.start, common::position_of(text, "again()"));
    }
}

#[tokio::test]
async fn legacy_native_hints_resolve_classes_and_reject_scalar_arguments() {
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        let mut exported = Vec::new();
        for name in [
            "int", "float", "string", "bool", "iterable", "object", "mixed", "void", "never",
            "resource", "true", "false", "null",
        ] {
            let text = format!(
                "<?php\nclass {name} {{ /** @return self */ function legacyMethod() {{ return $this; }} }}\nfunction take{name}({name} $value) {{ $value->legacyMethod()->legacyMethod(); }}\ntake{name}(new {name}());\ntake{name}(1);\n"
            );
            let uri = open(&backend, dir.path(), name, &text).await;
            let diagnostics = ready_diagnostics(&backend, &uri).await;
            assert_eq!(diagnostics.len(), 1, "{version} {name}: {diagnostics:?}");
            assert!(
                common::message_text(&diagnostics[0].message).contains("expects"),
                "{name}: {diagnostics:?}"
            );
            assert_eq!(
                diagnostics[0].range.start.line, 4,
                "{name}: {diagnostics:?}"
            );
            let hint = text.find(&format!("({name} $value)")).unwrap() + 1;
            let pos = common::offset_position(&text, hint);
            let targets = common::definition_locations(
                common::goto_definition_at(&backend, &uri, pos.line, pos.character).await,
            );
            assert_eq!(targets.len(), 1, "{name} native hint: {targets:?}");
            assert_eq!(
                targets[0].range.start,
                common::position_of(&text, &format!("class {name}"))
            );
            let positive = text.replace(
                &format!("take{name}(1);"),
                &format!("$probe = get_class(new {name}());"),
            );
            exported.push(runtime_case(name, &positive, &json!(["string", name])));
            let rejection = text.replace(&format!("take{name}(1);"), &format!("set_error_handler(function () {{ throw new Exception(); }}); $probe = false; try {{ take{name}(1); }} catch (Exception $error) {{ $probe = true; }} restore_error_handler();"));
            exported.push(runtime_case(
                &format!("{name}_reject_scalar"),
                &rejection,
                &json!(["boolean", true]),
            ));
            let pos = common::position_of(&text, "$value->legacyMethod");
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
            assert!(hover.contains(&format!("\\{name}")), "{name}: {hover}");
            let pos = common::position_of_last(&text, "legacyMethod()");
            let targets = common::definition_locations(
                common::goto_definition_at(&backend, &uri, pos.line, pos.character + 2).await,
            );
            assert_eq!(targets.len(), 1, "{name}: {targets:?}");
            assert_eq!(
                targets[0].range.start,
                common::position_of(&text, "legacyMethod()")
            );
            let cut = text.find("$value->legacyMethod").unwrap() + "$value->".len();
            let edited = &text[..cut];
            let uri = open(&backend, dir.path(), name, edited).await;
            let pos = common::offset_position(edited, edited.len());
            let completion =
                common::completion_response(&backend, &uri, pos.line, pos.character, None).await;
            assert!(
                serde_json::to_value(completion)
                    .unwrap()
                    .to_string()
                    .contains("legacyMethod"),
                "{name}"
            );
        }
        if let Some(output) = std::env::var_os("PHPANTOM_COMPAT_OUTPUT") {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(
                Path::new(&output).join(format!("native-hints-{version}.json")),
                serde_json::to_vec_pretty(
                    &json!({"target": version.to_string(), "cases": exported}),
                )
                .unwrap(),
            )
            .unwrap();
        }
    }
}

#[tokio::test]
async fn legacy_native_hints_keep_declaration_namespaces_and_imports() {
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        let declaration = "<?php\nnamespace { class int { function globalOnly() {} } }\nnamespace Model { class int { function modelOnly() {} function same(int $item) { $item->modelOnly(); } } }\nnamespace Service {\nuse Model\\int as iterable;\nuse int as GlobalInteger;\nclass Consumer {\nfunction accept(iterable $item) { $item->modelOnly(); }\nfunction globalValue(GlobalInteger $item) { $item->globalOnly(); }\n}\n}\n";
        let declared = open(&backend, dir.path(), "declarations", declaration).await;
        assert!(ready_diagnostics(&backend, &declared).await.is_empty());
        // A fresh session must read the callee's native hints from disk.
        let backend = Backend::new_test_with_full_stubs();
        common::initialize_with_options(&backend, &dunce::canonicalize(dir.path()).unwrap(), None)
            .await;
        let caller = "<?php\nnamespace Caller;\n$service = new \\Service\\Consumer();\n$service->accept(new \\Model\\int());\n$service->globalValue(new \\int());\n$service->accept(new \\int());\n$service->globalValue(new \\Model\\int());\n";
        let uri = open(&backend, dir.path(), "caller", caller).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        let lines: Vec<_> = diagnostics.iter().map(|d| d.range.start.line).collect();
        assert_eq!(lines, vec![5, 6], "{version}: {diagnostics:?}");
    }
}

#[tokio::test]
async fn legacy_hints_preserve_phpdoc_and_modern_native_primitives() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 0),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let text = "<?php\n/** @param int $value */\nfunction documented($value) {}\nfunction nativeArray(array $value) {}\nfunction nativeCallable(callable $value) {}\ndocumented(1);\nnativeArray(array(1));\nnativeCallable(function () {});\ndocumented(array());\nnativeArray(1);\nnativeCallable(1);\n";
        let uri = open(&backend, dir.path(), "controls", text).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        let lines: Vec<_> = diagnostics.iter().map(|d| d.range.start.line).collect();
        assert_eq!(lines, vec![8, 9, 10], "{version}: {diagnostics:?}");
        if version.major >= 7 {
            let text = "<?php function scalar(int $value) {} scalar(1); scalar(array());";
            let uri = open(&backend, dir.path(), "modern", text).await;
            let diagnostics = ready_diagnostics(&backend, &uri).await;
            assert_eq!(diagnostics.len(), 1, "{version}: {diagnostics:?}");
            assert!(
                common::message_text(&diagnostics[0].message).contains("expects int"),
                "{version}: {diagnostics:?}"
            );
        }
    }
}

#[tokio::test]
async fn legacy_targets_report_unsupported_syntax() {
    let audit = audit();
    for (version, id) in [
        (PhpVersion::new(5, 5), "variadic"),
        (PhpVersion::new(5, 6), "return_type"),
    ] {
        let (dir, backend) = project(version).await;
        let uri = open(&backend, dir.path(), id, source(&audit, id)).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        assert!(
            diagnostics.iter().any(
                |d| d.code == Some(Code::String("unsupported_syntax".into()))
                    && common::message_text(&d.message).contains("version")
            ),
            "{id}: expected an unsupported-version diagnostic, got {diagnostics:?}"
        );
    }
}

#[test]
fn syntax_capabilities_follow_release_boundaries() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("compatibility/syntax.json")).unwrap();
    let backend = Backend::new_test();
    let root = tempfile::tempdir().unwrap();
    let uri = common::file_uri(&root.path().join("syntax.php"));
    for version in PhpVersion::TARGETS {
        backend.set_php_version(version);
        let mut exported = Vec::new();
        for case in &cases {
            let minimum = PhpVersion::from_exact(case["minimum"].as_str().unwrap()).unwrap();
            // Controls deliberately use names that became reserved later.
            if minimum == PhpVersion::new(5, 5) && version.major != 5 {
                continue;
            }
            let source = case["source"].as_str().unwrap();
            backend.update_ast(&uri, source);
            let mut syntax_errors = Vec::new();
            backend.collect_syntax_error_diagnostics(&uri, source, &mut syntax_errors);
            assert!(
                syntax_errors.is_empty(),
                "{} at {version}: {syntax_errors:?}",
                case["id"]
            );
            let mut diagnostics = Vec::new();
            backend.collect_unsupported_syntax_diagnostics(&uri, source, &mut diagnostics);
            assert_eq!(
                diagnostics.is_empty(),
                version >= minimum,
                "{} at {version}: {diagnostics:?}",
                case["id"]
            );
            for diagnostic in &diagnostics {
                assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::Error));
                assert!(diagnostic.range.start < diagnostic.range.end);
            }
            exported.push(json!({"id": case["id"], "source": source, "lint": version >= minimum}));
        }
        if let Some(output) = std::env::var_os("PHPANTOM_COMPAT_OUTPUT") {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(
                Path::new(&output).join(format!("syntax-{version}.json")),
                serde_json::to_vec_pretty(
                    &json!({"target": version.to_string(), "cases": exported}),
                )
                .unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
fn syntax_diagnostics_match_the_reference_audit() {
    let audit = audit();
    let backend = Backend::new_test();
    let root = tempfile::tempdir().unwrap();
    let uri = common::file_uri(&root.path().join("audit.php"));
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        backend.set_php_version(version);
        let key = format!("php{}{}", version.major, version.minor);
        for case in audit["cases"].as_array().unwrap() {
            let source = case["source"].as_str().unwrap();
            let mut diagnostics = Vec::new();
            backend.collect_unsupported_syntax_diagnostics(&uri, source, &mut diagnostics);
            assert_eq!(
                diagnostics.is_empty(),
                case[&key]["reference"]["lintExit"] == 0,
                "{} at {version}: {diagnostics:?}",
                case["id"]
            );
        }
    }
}

#[tokio::test]
async fn unsupported_syntax_agrees_with_cli_and_preserves_editor_positions() {
    let (dir, backend) = project(PhpVersion::new(5, 5)).await;
    let text = "<?php\r\n/* 한글 😀 */ function example(...$values) { return $values; }\r\nexample(1);\r\n";
    let uri = open(&backend, dir.path(), "syntax", text).await;
    let diagnostics = ready_diagnostics(&backend, &uri).await;
    let unsupported: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == Some(Code::String("unsupported_syntax".into())))
        .collect();
    assert_eq!(unsupported.len(), 1, "{diagnostics:?}");
    assert_eq!(unsupported[0].range.start, common::position_of(text, "..."));
    let position = common::position_of(text, "example(1)");
    let target = common::goto_definition_at(&backend, &uri, position.line, position.character + 2)
        .await
        .unwrap();
    let target = serde_json::to_value(target).unwrap();
    let target = target.as_array().map_or(&target, |items| &items[0]);
    assert_eq!(
        target["range"]["start"],
        json!(common::position_of(text, "example(..."))
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_phpantom_lsp"))
        .args(["analyze", "--project-root"])
        .arg(dir.path())
        .args(["--format", "json", "--no-colour"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("unsupported_syntax"),
        "CLI disagrees with LSP: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Variadic parameters: requires PHP 5.6"),
        "{stdout}"
    );
    let report: Value = serde_json::from_str(&stdout).unwrap();
    let messages: Vec<_> = report["files"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|file| file["messages"].as_array().unwrap())
        .filter(|message| message["identifier"] == "unsupported_syntax")
        .collect();
    assert_eq!(messages.len(), unsupported.len());
    assert_eq!(
        messages[0]["message"],
        common::message_text(&unsupported[0].message)
    );
    assert_eq!(messages[0]["line"], unsupported[0].range.start.line + 1);

    let mut stub_diagnostics = Vec::new();
    backend.collect_unsupported_syntax_diagnostics(
        "phpantom-stub:///test.php",
        "<?php function f(): int {}",
        &mut stub_diagnostics,
    );
    assert!(stub_diagnostics.is_empty());
}

#[tokio::test]
async fn configured_stubs_keep_modern_syntax_on_legacy_targets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("stubs")).unwrap();
    std::fs::write(
        dir.path().join(".phpantom.toml"),
        "[php]\nversion = \"5.5\"\n[stubs]\npaths = [\"stubs\"]\n",
    )
    .unwrap();
    let source = "<?php function modern_stub(int|string $x): mixed {}";
    let path = dir.path().join("stubs/modern.php");
    std::fs::write(&path, source).unwrap();
    let backend = Backend::new_test();
    common::initialize_with_options(&backend, &dunce::canonicalize(dir.path()).unwrap(), None)
        .await;
    let uri = common::file_uri(&dunce::canonicalize(&path).unwrap());
    let mut diagnostics = Vec::new();
    backend.collect_unsupported_syntax_diagnostics(&uri, source, &mut diagnostics);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let project_uri = common::file_uri(&dir.path().join("source.php"));
    backend.collect_unsupported_syntax_diagnostics(&project_uri, source, &mut diagnostics);
    assert!(
        !diagnostics.is_empty(),
        "the same syntax in source must be diagnosed"
    );
}

#[test]
fn future_keyword_recovery_preserves_legacy_calls_and_type_names() {
    let backend = Backend::new_test();
    let root = tempfile::tempdir().unwrap();
    let uri = common::file_uri(&root.path().join("keywords.php"));
    for (version, source) in [
        (
            PhpVersion::new(5, 5),
            "<?php function fn($a) {} function match($a) {} $x = true ? fn(array(1, array(2))) : match(3);",
        ),
        (
            PhpVersion::new(5, 6),
            "<?php function f(readonly &$x, readonly ...$y) {}",
        ),
        (
            PhpVersion::new(8, 0),
            "<?php function f(readonly|stdClass $x) {}",
        ),
        (PhpVersion::new(8, 0), "<?php class C { readonly $x; }"),
    ] {
        backend.set_php_version(version);
        backend.update_ast(&uri, source);
        let mut diagnostics = Vec::new();
        backend.collect_syntax_error_diagnostics(&uri, source, &mut diagnostics);
        backend.collect_unsupported_syntax_diagnostics(&uri, source, &mut diagnostics);
        assert!(diagnostics.is_empty(), "{version}: {diagnostics:?}");
    }
}
