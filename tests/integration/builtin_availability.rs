//! PHP release boundaries across embedded indexes and lazy stub extraction.

use crate::common;
use phpantom_lsp::{Backend, types::PhpVersion};
use tower_lsp_server::{LanguageServer, gen_lsp_types::*};

async fn diagnostics(backend: &Backend, uri: &Uri) -> Vec<Diagnostic> {
    backend.publish_test_diagnostics(uri.as_str()).await;
    let report = backend
        .diagnostic(DocumentDiagnosticParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            identifier: None,
            previous_result_id: None,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .await
        .unwrap();
    match report {
        DocumentDiagnosticReport::RelatedFullDocumentDiagnosticReport(report) => {
            report.full_document_diagnostic_report.items
        }
        other => panic!("expected full diagnostics: {other:?}"),
    }
}

#[tokio::test]
async fn builtin_availability_fresh_targets_agree_across_lsp_features() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let path = root.join("availability.php");
    let uri: Uri = common::file_uri(&path).parse().unwrap();
    let text = "<?php\nhash_equals('a', 'b');\nrandom_int(1, 2);\nstr_contains('a', 'b');\narray_is_list([]);\nnew Fiber(function () {});\necho PHP_INT_MIN;\nDateTime::createFromTimestamp(0);\ncreate_function('', '');\necho PHP_INT_MAX;\n";
    std::fs::write(&path, text).unwrap();
    for (major, minor) in [(5, 5), (5, 6), (7, 4), (8, 0), (8, 5), (5, 6), (5, 5)] {
        std::fs::write(
            root.join(".phpantom.toml"),
            format!("[php]\nversion = \"{major}.{minor}\"\n"),
        )
        .unwrap();
        let backend = Backend::new_test_with_full_stubs();
        common::initialize_with_options(&backend, &root, None).await;
        common::open_php(&backend, &uri, text).await;
        // Populate shared stub files before checking that unavailable siblings stay absent.
        assert!(backend.find_or_load_function(&["hash"]).is_some());
        assert!(backend.find_or_load_function(&["strlen"]).is_some());
        assert!(common::hover_response(&backend, &uri, 9, 7).await.is_some());
        let reports = diagnostics(&backend, &uri).await;
        let target = major * 100 + minor;
        for (line, name, available) in [
            (1, "hash_equals", target >= 506),
            (2, "random_int", target >= 700),
            (3, "str_contains", target >= 800),
            (4, "array_is_list", target >= 801),
            (5, "Fiber", target >= 801),
            (6, "PHP_INT_MIN", target >= 700),
            (7, "createFromTimestamp", target >= 804),
            (8, "create_function", target < 800),
        ] {
            let pos = common::position_of(text, name);
            let hover = common::hover_response(&backend, &uri, pos.line, pos.character + 2).await;
            assert_eq!(
                hover.is_some(),
                available || (name == "PHP_INT_MIN" && target < 700),
                "{target}: hover {name}: {hover:?}"
            );
            let definition =
                common::goto_definition_at(&backend, &uri, pos.line, pos.character + 2).await;
            // Embedded stubs have no file to navigate to; installed stubs are checked below.
            assert!(
                definition.is_none(),
                "{target}: definition {name}: {definition:?}"
            );
            let unknown = reports.iter().find(|report| report.range.start.line == line
                && matches!(&report.code, Some(Code::String(code)) if code.starts_with("unknown_")));
            assert_eq!(
                unknown.is_some(),
                !available,
                "{target}: diagnostics {name}: {reports:?}"
            );
            if name == "PHP_INT_MIN" && !available {
                assert_eq!(
                    unknown.unwrap().severity,
                    Some(DiagnosticSeverity::Information)
                );
                let text =
                    common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
                assert!(text.contains("'PHP_INT_MIN'"), "{text}");
            }
            let labels = common::response_labels(
                common::completion_response(&backend, &uri, pos.line, pos.character + 3, None)
                    .await,
            );
            assert_eq!(
                labels
                    .iter()
                    .any(|label| label.split('(').next() == Some(name)),
                available,
                "{target}: completion {name}: {labels:?}"
            );
        }
        assert_eq!(
            backend.stub_constant_index().contains_key("PHP_INT_MIN"),
            target >= 700
        );
    }
}

#[test]
fn builtin_availability_members_and_parameter_variants() {
    let source = r#"<?php
use JetBrains\PhpStorm\Internal\PhpStormStubsElementAvailable as Available;
class Example {
    /** @since 5.6 */
    public $newProperty;
    #[Available(from: '7.0')]
    public const NEW_CONSTANT = 1;
    /** @since 5.6 */
    public function newMethod() {}
    public function arguments($old, #[Available(from: '5.6')] $new = null) {}
}
/** @since 7.0 */
class FutureClass {}
#[Available(from: '8.1')]
interface FutureInterface {}
"#;
    for (minor, count) in [(5, 1), (6, 2)] {
        let classes = Backend::parse_php_versioned(source, Some(PhpVersion::new(5, minor)));
        assert_eq!(classes.len(), 1);
        let class = &classes[0];
        assert_eq!(class.properties.len(), usize::from(minor == 6));
        assert!(class.constants.is_empty());
        assert_eq!(class.methods.len(), count);
        assert_eq!(
            class
                .methods
                .iter()
                .find(|m| m.name == "arguments")
                .unwrap()
                .parameters
                .len(),
            count
        );
    }
}

#[tokio::test]
async fn builtin_availability_keeps_polyfills_and_package_since_tags() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(root.join("stubs")).unwrap();
    std::fs::write(
        root.join(".phpantom.toml"),
        "[php]\nversion = \"5.5\"\n[stubs]\npaths = [\"stubs\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("stubs/overrides.php"),
        "<?php\n/** @since 9.0 */\nfunction str_contains($a, $b): string {}\n",
    )
    .unwrap();
    let text = "<?php\n/** @since 9.0 */\nfunction random_int($min, $max) { return 1; }\n/** @since 9.0 */\nclass Fiber {}\n/** @since 9.0 */\n\\define('PHP_INT_MIN', -1);\nrandom_int(1, 2);\nstr_contains('a', 'b');\nnew Fiber();\necho PHP_INT_MIN;\n";
    let path = root.join("polyfills.php");
    std::fs::write(&path, text).unwrap();
    let uri = common::file_uri(&path).parse().unwrap();
    let backend = Backend::new_test_with_full_stubs();
    common::initialize_with_options(&backend, &root, None).await;
    common::open_php(&backend, &uri, text).await;
    let position = common::position_of_last(text, "PHP_INT_MIN");
    assert!(
        common::hover_response(&backend, &uri, position.line, position.character + 2)
            .await
            .is_some()
    );
    assert!(backend.find_or_load_function(&["random_int"]).is_some());
    assert_eq!(
        backend
            .find_or_load_function(&["str_contains"])
            .unwrap()
            .return_type_str()
            .as_deref(),
        Some("string")
    );
    let reports = diagnostics(&backend, &uri).await;
    assert!(!reports.iter().any(|report| matches!(&report.code, Some(Code::String(code)) if code.starts_with("unknown_"))), "{reports:?}");
}

#[tokio::test]
async fn builtin_availability_external_checkout_matches_embedded_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let stubs = root.join("stubs");
    std::fs::create_dir_all(stubs.join("Core")).unwrap();
    std::fs::create_dir_all(stubs.join("redis")).unwrap();
    std::fs::write(stubs.join("PhpStormStubsMap.php"), "<?php\nclass PhpStormStubsMap {\nconst CLASSES = array (\n);\nconst FUNCTIONS = array (\n  'old_builtin' => 'Core/functions.php',\n  'new_builtin' => 'Core/functions.php',\n  'redis_helper' => 'redis/functions.php',\n);\nconst CONSTANTS = array (\n  'PHP_INT_MIN' => 'Core/constants.php',\n);\n}\n").unwrap();
    std::fs::write(
        stubs.join("Core/functions.php"),
        "<?php\nfunction old_builtin() {}\n/** @since 5.6 */\nfunction new_builtin() {}\n",
    )
    .unwrap();
    std::fs::write(
        stubs.join("Core/constants.php"),
        "<?php\ndefine('PHP_INT_MIN', -1);\n",
    )
    .unwrap();
    std::fs::write(
        stubs.join("redis/functions.php"),
        "<?php\n/** @since 9.0 */\nfunction redis_helper() {}\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".phpantom.toml"),
        "[php]\nversion = \"5.5\"\n[stubs]\npaths = [\"stubs\"]\n",
    )
    .unwrap();
    let text = "<?php\nold_builtin();\nnew_builtin();\nredis_helper();\necho PHP_INT_MIN;\n";
    let path = root.join("probe.php");
    std::fs::write(&path, text).unwrap();
    let uri = common::file_uri(&path).parse().unwrap();
    let backend = Backend::new_test();
    common::initialize_with_options(&backend, &root, None).await;
    common::open_php(&backend, &uri, text).await;
    assert!(backend.find_or_load_function(&["old_builtin"]).is_some());
    assert!(backend.find_or_load_function(&["new_builtin"]).is_none());
    assert!(backend.find_or_load_function(&["redis_helper"]).is_some());
    let hover = common::markup_hover_at(&backend, &uri, 4, 7).await;
    assert!(hover.contains("'PHP_INT_MIN'"), "{hover}");
    for (line, available) in [(1, true), (2, false), (3, true), (4, false)] {
        assert_eq!(
            common::goto_definition_at(&backend, &uri, line, 7)
                .await
                .is_some(),
            available
        );
    }
}

#[tokio::test]
async fn builtin_availability_cli_uses_the_same_hash_equals_boundary() {
    use phpantom_lsp::analyse::*;
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join("probe.php"), "<?php\nhash_equals('a', 'b');\n").unwrap();
    for (minor, exit_code) in [(5, 1), (6, 0)] {
        std::fs::write(
            root.join(".phpantom.toml"),
            format!("[php]\nversion = \"5.{minor}\"\n"),
        )
        .unwrap();
        assert_eq!(
            run(AnalyseOptions {
                #[cfg(feature = "semantic-export")]
                resolution_dump: None,
                workspace_root: root.clone(),
                path_filters: vec![],
                severity_filter: SeverityFilter::All,
                code_filter: CodeFilter::new(vec!["unknown_function".into()], vec![]),
                use_colour: false,
                output_format: OutputFormat::Json,
                debug: false,
                verbosity: 0,
                global_config: None,
            })
            .await,
            exit_code
        );
    }
}
