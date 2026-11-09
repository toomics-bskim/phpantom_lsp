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

#[tokio::test]
#[ignore = "The server does not yet report syntax introduced after the selected PHP version"]
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
            diagnostics
                .iter()
                .any(|d| common::message_text(&d.message).contains("PHP")
                    && common::message_text(&d.message).contains("version")),
            "{id}: expected an unsupported-version diagnostic, got {diagnostics:?}"
        );
    }
}
