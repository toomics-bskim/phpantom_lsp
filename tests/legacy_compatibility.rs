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
#[ignore = "Legacy source forms need version-aware parsing; the expected result is no syntax error"]
async fn legacy_source_forms_have_no_syntax_errors() {
    let audit = audit();
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        for id in [
            "curly_offset",
            "reserved_match_name",
            "hash_bracket_comment",
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
