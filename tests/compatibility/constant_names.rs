use super::*;

#[tokio::test]
async fn legacy_constants_resolve_case_without_folding_sensitive_names() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let source = "<?php\ndefine('AUDIT_CASE_CONST', 'folded', true);\ndefine('Audit_Case_Const', 'exact');\n$fold = audit_case_const;\n$exact = Audit_Case_Const;\necho $fold, $exact;\n";
        let uri = open(&backend, dir.path(), "constants", source).await;
        let pos = common::position_of(source, "audit_case_const;");
        if version.major < 8 {
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
            assert!(hover.contains("folded"), "{version}: {hover}");
        }
        let pos = common::position_of(source, "Audit_Case_Const;");
        let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
        assert!(hover.contains("exact"), "{version}: {hover}");
    }
}

#[tokio::test]
async fn legacy_constant_references_rename_and_navigation_select_the_runtime_key() {
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        let source = "<?php\ndefine('SAME_NAME', 'folded', true);\ndefine('SAME_NAME', 'exact');\ndefine('Same_Name', 'mixed');\n$a = same_name;\n$b = SaMe_NaMe;\n$c = SAME_NAME;\n$d = Same_Name;\n";
        let uri = open(&backend, dir.path(), "identity", source).await;
        for (needle, declaration_line, count) in [
            ("same_name;", 1, 3),
            ("SAME_NAME;", 2, 2),
            ("Same_Name;", 3, 2),
        ] {
            let pos = common::position_of(source, needle);
            let targets = common::definition_locations(
                common::goto_definition_at(&backend, &uri, pos.line, pos.character + 2).await,
            );
            assert_eq!(targets.len(), 1, "{version}: {targets:?}");
            assert_eq!(
                targets[0].range.start.line, declaration_line,
                "{version}: {needle}"
            );
            let references = common::find_references(&backend, &uri, pos, true).await;
            assert_eq!(
                references.len(),
                count,
                "{version}: {needle}: {references:?}"
            );
        }
        let pos = common::position_of(source, "same_name;");
        let edit = common::rename(&backend, &uri, pos.line, pos.character + 2, "RENAMED")
            .await
            .unwrap();
        let renamed = common::apply_workspace_edit(source, &edit);
        assert!(
            renamed.contains("define('RENAMED', 'folded', true)"),
            "{renamed}"
        );
        assert!(
            renamed.contains("$a = RENAMED;") && renamed.contains("$b = RENAMED;"),
            "{renamed}"
        );
        assert!(
            renamed.contains("$c = SAME_NAME;") && renamed.contains("$d = Same_Name;"),
            "{renamed}"
        );
        let pos = common::position_of(source, "SAME_NAME', 'folded'");
        let references = common::find_references(&backend, &uri, pos, true).await;
        assert_eq!(references.len(), 3, "declaration identity: {references:?}");
    }
}

#[tokio::test]
async fn constant_namespaces_imports_and_builtins_keep_lookup_order() {
    for version in [PhpVersion::new(5, 6), PhpVersion::new(8, 5)] {
        let (dir, backend) = project(version).await;
        let declarations =
            "<?php\nnamespace Lib;\nconst Token = 'namespace';\nconst PHP_MAJOR_VERSION = 42;\n";
        let declaration_uri = open(&backend, dir.path(), "declarations", declarations).await;
        let source = "<?php\nnamespace Lib;\nuse const Lib\\Token as Imported;\n$a = lIB\\Token;\n$b = \\lIB\\Token;\n$c = Imported;\n$d = PHP_MAJOR_VERSION;\n$e = \\PHP_MAJOR_VERSION;\n$f = Other\\PHP_MAJOR_VERSION;\n";
        let uri = open(&backend, dir.path(), "namespace_reads", source).await;
        for (needle, value) in [
            ("\\lIB\\Token;", "namespace"),
            ("= Imported;", "namespace"),
            ("= PHP_MAJOR_VERSION;", "42"),
        ] {
            let pos = common::position_of(source, needle);
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 3).await;
            assert!(hover.contains(value), "{version}: {hover}");
            let targets = common::definition_locations(
                common::goto_definition_at(&backend, &uri, pos.line, pos.character + 3).await,
            );
            assert_eq!(targets.len(), 1, "{targets:?}");
            assert_eq!(targets[0].uri, declaration_uri);
        }
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        let missing: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == Some(Code::String("unknown_constant".into())))
            .map(|d| d.range.start.line)
            .collect();
        assert_eq!(missing, [3, 8], "{version}: {diagnostics:?}");
    }
}

#[tokio::test]
async fn undefined_constants_preserve_legacy_types_and_versioned_diagnostics() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 1),
        PhpVersion::new(7, 2),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let source = "<?php\nnamespace Demo;\nuse const Missing\\ABSENT as Imported;\n$bare = NOT_DEFINED;\n$absolute = \\NOT_DEFINED;\n$qualified = Other\\NOT_DEFINED;\n$relative = namespace\\NOT_DEFINED;\n$imported = Imported;\n$minimum = PHP_INT_MIN;\nif (defined('GUARDED')) { $known = GUARDED; }\necho $bare;\n";
        let source = if version == PhpVersion::new(5, 5) {
            source
                .replace(
                    "use const Missing\\ABSENT as Imported;",
                    "// imports require PHP 5.6",
                )
                .replace("$imported = Imported;", "$imported = \\Missing\\ABSENT;")
        } else {
            source.to_string()
        };
        let source = source.as_str();
        let uri = open(&backend, dir.path(), "missing", source).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        let unknown: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == Some(Code::String("unknown_constant".into())))
            .collect();
        assert_eq!(
            unknown.len(),
            if version.major == 5 { 6 } else { 5 },
            "{version}: {diagnostics:?}"
        );
        let bare = unknown.iter().find(|d| d.range.start.line == 3).unwrap();
        let severity = if version.major >= 8 {
            DiagnosticSeverity::Error
        } else if version >= PhpVersion::new(7, 2) {
            DiagnosticSeverity::Warning
        } else {
            DiagnosticSeverity::Information
        };
        assert_eq!(bare.severity, Some(severity));
        assert!(
            unknown
                .iter()
                .filter(|d| (4..=6).contains(&d.range.start.line))
                .all(|d| d.severity == Some(DiagnosticSeverity::Error)),
            "{version}: {unknown:?}"
        );
        if version.major < 8 {
            let pos = common::position_after(source, "echo $ba");
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character).await;
            assert!(hover.contains("'NOT_DEFINED'"), "{version}: {hover}");
        }
    }
}

#[tokio::test]
async fn legacy_constant_initializers_keep_the_declaration_context() {
    let (dir, backend) = project(PhpVersion::new(5, 6)).await;
    open(
        &backend,
        dir.path(),
        "origin",
        "<?php namespace Original; define('VALUE_FROM_ORIGIN', UNKNOWN_AT_ORIGIN);",
    )
    .await;
    let source = "<?php namespace Caller; const UNKNOWN_AT_ORIGIN = 42; $probe = VALUE_FROM_ORIGIN; echo $probe;";
    let uri = open(&backend, dir.path(), "caller", source).await;
    let pos = common::position_after(source, "echo $pro");
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character).await;
    assert!(hover.contains("'UNKNOWN_AT_ORIGIN'"), "{hover}");
}

#[tokio::test]
async fn legacy_constant_initializers_retain_imported_name_fallbacks() {
    let (dir, backend) = project(PhpVersion::new(5, 6)).await;
    let declaration = "<?php namespace Original; use const Missing\\VALUE as Imported; define('VALUE_FROM_IMPORT', Imported);";
    let declaration_uri = open(&backend, dir.path(), "import_origin", declaration).await;
    let source =
        "<?php namespace Caller; const Imported = 42; $probe = VALUE_FROM_IMPORT; echo $probe;";
    let uri = open(&backend, dir.path(), "import_caller", source).await;
    let pos = common::position_after(source, "echo $pro");
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character).await;
    assert!(hover.contains("'VALUE'"), "{hover}");
    backend.update_ast(
        declaration_uri.as_str(),
        &declaration.replace("Missing\\VALUE", "Missing\\CHANGED"),
    );
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character).await;
    assert!(hover.contains("'CHANGED'"), "{hover}");
    open(
        &backend,
        dir.path(),
        "import_target",
        "<?php namespace Missing; const CHANGED = 99;",
    )
    .await;
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character).await;
    assert!(hover.contains("99"), "{hover}");
}

#[tokio::test]
async fn legacy_constant_arguments_use_the_same_imports_and_string_fallbacks() {
    let (dir, backend) = project(PhpVersion::new(5, 6)).await;
    open(
        &backend,
        dir.path(),
        "argument_constants",
        "<?php namespace Library; const VALUE = 42;",
    )
    .await;
    let source = "<?php\nnamespace Caller;\nuse const Library\\VALUE as Imported;\n/** @param int $value */\nfunction consume($value) {}\nconsume(Imported);\nconsume(MISSING_ARGUMENT);\n";
    let uri = open(&backend, dir.path(), "constant_arguments", source).await;
    let diagnostics = ready_diagnostics(&backend, &uri).await;
    let mismatches: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == Some(Code::String("type_mismatch_argument".into())))
        .map(|d| d.range.start.line)
        .collect();
    assert_eq!(mismatches, [6], "{diagnostics:?}");
}

#[tokio::test]
async fn constant_existence_guards_require_a_branch_proof() {
    let (dir, backend) = project(PhpVersion::new(8, 5)).await;
    let source = "<?php\nif (defined('SAFE')) { echo SAFE; }\nif (defined('UNSAFE') || true) { echo UNSAFE; }\ndefined('ALSO_SAFE') && print(ALSO_SAFE);\ndefined('ALSO_UNSAFE') || print(ALSO_UNSAFE);\nif (!defined('AFTER_RETURN')) { return; }\necho AFTER_RETURN;\n$optional = defined('OPTIONAL') ? OPTIONAL : null;\n$other = !defined('OTHER') ? null : OTHER;\n";
    let uri = open(&backend, dir.path(), "guards", source).await;
    let diagnostics = ready_diagnostics(&backend, &uri).await;
    let lines: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == Some(Code::String("unknown_constant".into())))
        .map(|d| d.range.start.line)
        .collect();
    assert_eq!(lines, [2, 4], "{diagnostics:?}");
}

#[tokio::test]
async fn legacy_constants_discover_case_variants_and_withdraw_changed_flags() {
    let (dir, backend) = project(PhpVersion::new(5, 6)).await;
    let path = dir.path().join("lazy.php");
    std::fs::write(&path, "<?php define('LAZY_TOKEN', 'discovered', true);").unwrap();
    let path = dunce::canonicalize(path).unwrap();
    backend
        .autoload_constant_index()
        .write()
        .insert("LAZY_TOKEN".into(), path.clone());
    let source = "<?php $value = lazy_token; echo $value;";
    let uri = open(&backend, dir.path(), "lazy_use", source).await;
    let pos = common::position_of(source, "lazy_token;");
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
    assert!(hover.contains("discovered"), "{hover}");
    let definition_uri = common::file_uri(&path);
    backend.update_ast(
        &definition_uri,
        "<?php define('LAZY_TOKEN', 'sensitive', false);",
    );
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
    assert!(
        hover.contains("'lazy_token'") && !hover.contains("discovered"),
        "{hover}"
    );
    backend.update_ast(
        &definition_uri,
        "<?php define('LAZY_TOKEN', 'restored', true);",
    );
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
    assert!(hover.contains("restored"), "{hover}");
    backend.update_ast(&definition_uri, "<?php");
    let source = "<?php $value = LAZY_TOKEN; echo $value;";
    let uri = open(&backend, dir.path(), "removed_constant", source).await;
    let pos = common::position_of(source, "LAZY_TOKEN;");
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
    assert!(
        hover.contains("'LAZY_TOKEN'") && !hover.contains("discovered"),
        "a stale discovery entry must not reload a removed declaration: {hover}"
    );
}

#[tokio::test]
async fn constant_completion_offers_both_kinds_with_their_values() {
    let (dir, backend) = project(PhpVersion::new(5, 6)).await;
    let source =
        "<?php\ndefine('SAME_NAME', 'folded', true);\ndefine('SAME_NAME', 'exact');\nSAME_";
    let uri = open(&backend, dir.path(), "completion", source).await;
    let pos = common::offset_position(source, source.len());
    let items = common::response_items(
        common::completion_response(&backend, &uri, pos.line, pos.character, None).await,
    )
    .unwrap();
    for (name, value) in [("SAME_NAME", "exact"), ("same_name", "folded")] {
        let item = items
            .iter()
            .find(|item| item.label == name)
            .unwrap_or_else(|| panic!("{name}: {items:?}"));
        assert!(
            item.detail
                .as_deref()
                .is_some_and(|detail| detail.contains(value)),
            "{item:?}"
        );
    }
}

#[tokio::test]
async fn constant_reads_keep_interpolation_keys_and_class_constants_separate() {
    for version in [PhpVersion::new(5, 6), PhpVersion::new(8, 5)] {
        let (dir, backend) = project(version).await;
        let source = "<?php\nclass Example { const Exact = 42; }\n$map = array('key' => 'value');\necho \"$map[key]\";\necho Example::Exact;\necho Example::exact;\n";
        let uri = open(&backend, dir.path(), "separate_names", source).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.code == Some(Code::String("unknown_constant".into()))),
            "{version}: {diagnostics:?}"
        );
        assert!(
            diagnostics.iter().any(|d| d.range.start.line == 5
                && d.code == Some(Code::String("unknown_member".into()))),
            "{version}: {diagnostics:?}"
        );
    }
}

#[tokio::test]
async fn legacy_constant_imports_share_cross_file_references() {
    let (dir, backend) = project(PhpVersion::new(5, 6)).await;
    let declarations = "<?php define('Library\\Mixed', 42, true); define('Library\\MiXeD', 99);";
    let definition_uri = open(&backend, dir.path(), "constants_library", declarations).await;
    let source = "<?php namespace Client; use const Library\\MIXED as Imported; $a = Imported; $b = \\LIBRARY\\mIxEd;";
    let uri = open(&backend, dir.path(), "constants_client", source).await;
    let pos = common::position_after(source, "$a = Im");
    let targets = common::definition_locations(
        common::goto_definition_at(&backend, &uri, pos.line, pos.character).await,
    );
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].uri, definition_uri);
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character).await;
    assert!(hover.contains("42"), "{hover}");
    let references = common::find_references(&backend, &uri, pos, true).await;
    assert_eq!(references.len(), 4, "{references:?}");
    let edit = common::rename(&backend, &uri, pos.line, pos.character, "Changed")
        .await
        .unwrap();
    let changes = common::edits_for_uri(&edit, &uri);
    assert!(
        changes.iter().any(|edit| edit.new_text.contains("Changed")),
        "{edit:?}"
    );
    assert!(
        !changes
            .iter()
            .any(|edit| edit.range.start == common::position_of(source, "Imported;")),
        "an explicit import alias stays local: {edit:?}"
    );
}

#[test]
fn changing_only_constant_case_rules_updates_the_index() {
    let backend = Backend::new_test();
    backend.set_php_version(PhpVersion::new(5, 6));
    let uri = common::absolute_uri("constant_flags.php");
    backend.update_ast(uri.as_str(), "<?php define('lower', 42, true);");
    assert!(backend.global_defines().read().contains_key("LOWER"));
    backend.update_ast(uri.as_str(), "<?php define('lower', 42, false);");
    assert!(!backend.global_defines().read().contains_key("LOWER"));
    assert!(backend.global_defines().read().contains_key("lower"));
}

#[tokio::test]
async fn constant_names_runtime_contract() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let mut cases = vec![runtime_case(
            "namespace_case",
            "<?php namespace Demo; const Exact = 42; $probe = \\demo\\Exact;",
            &json!(["integer", 42]),
        )];
        if version.major < 8 {
            cases.push(runtime_case("folded_and_exact", "<?php define('TOKEN', 'folded', true); define('TOKEN', 'exact'); define('Token', 'mixed'); $probe = array(token, TOKEN, Token, ToKeN);", &json!(["array", ["folded", "exact", "mixed", "folded"]])));
            cases.push(runtime_case(
                "bare_fallback",
                "<?php namespace Demo; $probe = NOT_DEFINED;",
                &json!(["string", "NOT_DEFINED"]),
            ));
            cases.push(runtime_case(
                "unavailable_builtin",
                "<?php $probe = PHP_INT_MIN;",
                &json!(["string", "PHP_INT_MIN"]),
            ));
            cases.push(runtime_case("storage_collision", "<?php define('lower_case', 42); define('LOWER_CASE', 99, true); $probe = array(lower_case, defined('LOWER_CASE'));", &json!(["array", [42, false]])));
        } else {
            cases.push(json!({"id":"modern_no_folding", "source":"<?php define('TOKEN', 'folded', true); $probe = token;", "lint":true, "runtime":{"expectedExit":255,"message":"Undefined constant"}}));
            cases.push(json!({"id":"modern_bare_error", "source":"<?php $probe = NOT_DEFINED;", "lint":true, "runtime":{"expectedExit":255,"message":"Undefined constant"}}));
        }
        for (id, read) in [
            ("absolute_missing", "\\NOT_DEFINED"),
            ("qualified_missing", "Other\\NOT_DEFINED"),
            ("relative_missing", "namespace\\NOT_DEFINED"),
        ] {
            cases.push(json!({"id":id, "source":format!("<?php namespace Demo; $probe = {read};"), "lint":true, "runtime":{"expectedExit":255,"message":"Undefined constant"}}));
        }
        if version == PhpVersion::new(5, 6) {
            cases.push(runtime_case(
                "imported_missing",
                "<?php namespace Demo; use const Missing\\VALUE as Imported; $probe = Imported;",
                &json!(["string", "VALUE"]),
            ));
            cases.push(runtime_case("imported_initializer", "<?php namespace Origin; use const Missing\\VALUE as Imported; define('COPIED', Imported); namespace Caller; const Imported = 42; $probe = COPIED;", &json!(["string", "VALUE"])));
        } else if version.major >= 8 {
            cases.push(json!({"id":"imported_missing", "source":"<?php namespace Demo; use const Missing\\VALUE as Imported; $probe = Imported;", "lint":true,"runtime":{"expectedExit":255,"message":"Undefined constant"}}));
        }
        if let Some(output) = std::env::var_os("PHPANTOM_COMPAT_OUTPUT") {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(
                Path::new(&output).join(format!("constant-names-{version}.json")),
                serde_json::to_vec_pretty(&json!({"target":version.to_string(),"cases":cases}))
                    .unwrap(),
            )
            .unwrap();
        }
    }
}
