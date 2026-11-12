use super::*;

fn save(version: PhpVersion, name: &str, cases: Vec<Value>) {
    if let Ok(output) = std::env::var("PHPANTOM_COMPAT_OUTPUT") {
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            Path::new(&output).join(format!("{name}-{version}.json")),
            serde_json::to_vec_pretty(&json!({"target":version.to_string(), "cases":cases}))
                .unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn generated_edits_create_tests_with_legacy_method_signatures() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("composer.json"), r#"{"autoload":{"psr-4":{"App\\":"src/"}},"autoload-dev":{"psr-4":{"Tests\\":"tests/"}}}"#).unwrap();
        std::fs::write(
            dir.path().join(".phpantom.toml"),
            format!("[php]\nversion = \"{version}\"\n"),
        )
        .unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let backend = Backend::new_test_with_full_stubs();
        backend.initialize(serde_json::from_value(json!({
            "rootUri": Uri::from_file_path(&root).unwrap(),
            "capabilities": {"workspace": {"workspaceEdit": {"resourceOperations": ["create"]}}}
        })).unwrap()).await.unwrap();
        backend.initialized(InitializedParams {}).await;
        let source =
            "<?php\nnamespace App;\nclass Invoice {\npublic function total() { return 42; }\n}\n";
        let uri = open(&backend, &dir.path().join("src"), "Invoice", source).await;
        let pos = common::position_of(source, "Invoice");
        let actions = common::code_actions_via_server(&backend, &uri, Range::new(pos, pos))
            .await
            .unwrap();
        let action = common::find_action_titled(&actions, "Create test for Invoice")
            .unwrap_or_else(|| panic!("{actions:?}"));
        let edit = action.edit.as_ref().unwrap();
        let created = edit
            .document_changes
            .as_ref()
            .unwrap()
            .iter()
            .find_map(|op| match op {
                DocumentChange::CreateFile(file) => Some(&file.uri),
                _ => None,
            })
            .unwrap();
        let generated = common::apply_edits("", &common::doc_change_edits_for_uri(edit, created));
        assert_eq!(
            generated.contains("(): void"),
            version.supports_void_return_type(),
            "{generated}"
        );
        // A small stand-in exercises the generated method without installing PHPUnit.
        let bootstrap = "<?php\nnamespace PHPUnit\\Framework;\nclass TestCase { public static function markTestIncomplete() {} }\n";
        let combined = format!(
            "{bootstrap}\n{}\n$test = new InvoiceTest(); $test->testTotal(); $probe = true;",
            generated.trim_start_matches("<?php")
        );
        save(
            version,
            "generated-test",
            vec![runtime_case(
                "create-test",
                &combined,
                &json!(["boolean", true]),
            )],
        );
    }
}

async fn apply(
    backend: &Backend,
    uri: &Uri,
    source: &str,
    needle: &str,
    select: bool,
    title: &str,
) -> String {
    let start = source.find(needle).unwrap();
    let range = Range::new(
        common::offset_position(source, start),
        common::offset_position(source, start + if select { needle.len() } else { 0 }),
    );
    let actions = common::code_actions_via_server(backend, uri, range)
        .await
        .unwrap();
    let action = common::find_action_titled(&actions, title)
        .unwrap_or_else(|| panic!("missing {title}: {actions:?}"));
    let resolved = if action.edit.is_some() {
        action.clone()
    } else {
        common::resolve_action(backend, uri.as_str(), source, action)
    };
    common::apply_workspace_edit(source, resolved.edit.as_ref().unwrap())
}

#[tokio::test]
async fn generated_edits_keep_legacy_syntax_and_runtime_behavior() {
    let cases = [
        (
            "resource_constructor",
            "<?php\nclass StreamHolder {\n/** @var resource */\npublic $stream;\n}\n",
            "$stream",
            false,
            "Generate constructor",
            "$stream=fopen('php://memory', 'r'); $probe=is_resource((new StreamHolder($stream))->stream); fclose($stream);",
            json!(["boolean", true]),
        ),
        (
            "class_constructor",
            "<?php\nclass ClockHolder {\n/** @var DateTime */\npublic $clock;\n}\n",
            "$clock",
            false,
            "Generate constructor",
            "$clock=new DateTime(); $probe=(new ClockHolder($clock))->clock === $clock;",
            json!(["boolean", true]),
        ),
        (
            "data_provider",
            "<?php\nclass PriceTest {\n/** @dataProvider prices */\npublic function testPrice($n) {}\n}\n",
            "prices",
            false,
            "Create data provider 'prices'",
            "$probe=PriceTest::prices();",
            json!(["array", []]),
        ),
        (
            "implement",
            "<?php\ninterface Contract {\n/** @return string */\npublic function read();\n}\nclass Implementation implements Contract {\n}\n",
            "Implementation",
            false,
            "Implement `read`",
            "$probe=method_exists(new Implementation(), 'read');",
            json!(["boolean", true]),
        ),
        (
            "extract_pair",
            "<?php\nfunction pair($x) {\n    $a = $x + 1;\n    $b = $x + 2;\n    return array($a, $b);\n}\n",
            "$a = $x + 1;\n    $b = $x + 2;",
            true,
            "Extract function",
            "$probe=pair(3);",
            json!(["array", [4, 5]]),
        ),
        (
            "extract_constant",
            "<?php\nclass Amount {\n    public function value() {\n        return 42;\n    }\n}\n",
            "42",
            true,
            "Extract constant",
            "$probe=(new Amount())->value();",
            json!(["integer", 42]),
        ),
        (
            "sort_imports",
            "<?php\nnamespace Demo;\nuse DateTime;\nuse ArrayObject;\n$probe = new DateTime() instanceof DateTime;\n",
            "use DateTime",
            false,
            "Sort use statements",
            "",
            json!(["boolean", true]),
        ),
        (
            "extract_variable",
            "<?php\nfunction answer() {\n    return 20 + 22;\n}\n",
            "20 + 22",
            true,
            "Extract variable",
            "$probe=answer();",
            json!(["integer", 42]),
        ),
    ];
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 0),
        PhpVersion::new(7, 1),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let mut exported = Vec::new();
        for (id, source, needle, select, title, run, expected) in &cases {
            for nl in ["\n", "\r\n"] {
                let source = source.replace('\n', nl);
                let needle = needle.replace('\n', nl);
                let uri = open(&backend, dir.path(), id, &source).await;
                let edited = apply(&backend, &uri, &source, &needle, *select, title).await;
                if *id == "resource_constructor" {
                    assert!(edited.contains("@param resource $stream"), "{edited}");
                    assert!(!edited.contains("__construct(resource"), "{edited}");
                }
                if *id == "class_constructor" {
                    assert!(edited.contains("__construct(DateTime"), "{edited}");
                }
                if *id == "implement" && !version.supports_return_types() {
                    assert!(!edited.contains("read():"), "{edited}");
                    assert!(edited.matches("@return string").count() >= 2, "{edited}");
                }
                if *id == "extract_pair" && !version.supports_short_destructuring() {
                    assert!(edited.contains("list("), "{edited}");
                }
                exported.push(runtime_case(
                    &format!("{id}-{}", nl.len()),
                    &format!("{edited}\n{run}\n"),
                    expected,
                ));
            }
        }
        save(version, "generated-edits", exported);
    }
}

#[tokio::test]
async fn generated_edits_gate_unavailable_transformations() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 0),
        PhpVersion::new(8, 0),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let mut exported = Vec::new();
        for (id, source, needle, title, available) in [
            (
                "promotion",
                "<?php\nclass Promotable {\nprivate $value;\nfunction __construct($value) { $this->value = $value; }\n}\n",
                "__construct($value",
                "Promote to constructor property",
                version.supports_promoted_properties(),
            ),
            (
                "coalescing",
                "<?php\n$value = isset($x) ? $x : 'default';\n",
                "isset($x)",
                "Simplify to ??",
                version.supports_null_coalescing(),
            ),
        ] {
            let uri = open(&backend, dir.path(), id, source).await;
            let offset = source.find(needle).unwrap()
                + if id == "promotion" {
                    "__construct(".len()
                } else {
                    0
                };
            let pos = common::offset_position(source, offset);
            let actions = common::code_actions_via_server(&backend, &uri, Range::new(pos, pos))
                .await
                .unwrap_or_default();
            assert_eq!(
                common::find_action_titled(&actions, title).is_some(),
                available,
                "{version}: {title}: {actions:?}"
            );
            if let Some(action) = common::find_action_titled(&actions, title) {
                let resolved = if action.edit.is_some() {
                    action.clone()
                } else {
                    common::resolve_action(&backend, uri.as_str(), source, action)
                };
                let edited = common::apply_workspace_edit(source, resolved.edit.as_ref().unwrap());
                let (run, expected) = if id == "promotion" {
                    (
                        "$probe = array_values((array) new Promotable(42));",
                        json!(["array", [42]]),
                    )
                } else {
                    ("$probe = $value;", json!(["string", "default"]))
                };
                exported.push(runtime_case(id, &format!("{edited}\n{run}"), &expected));
            }
        }
        if !exported.is_empty() {
            save(version, "generated-transformations", exported);
        }
    }
}

#[tokio::test]
async fn generated_edits_completion_keywords_follow_version_boundaries() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 0),
        PhpVersion::new(7, 4),
        PhpVersion::new(8, 0),
        PhpVersion::new(8, 1),
    ] {
        let (dir, backend) = project(version).await;
        for (keyword, minimum) in [
            ("fn", PhpVersion::new(7, 4)),
            ("match", PhpVersion::new(8, 0)),
            ("enum", PhpVersion::new(8, 1)),
        ] {
            let source = format!("<?php\n{}", &keyword[..1]);
            let uri = open(&backend, dir.path(), keyword, &source).await;
            let items = common::response_items(
                common::completion_response(&backend, &uri, 1, 1, None).await,
            )
            .unwrap_or_default();
            assert_eq!(
                items.iter().any(|item| item.label == keyword),
                version >= minimum,
                "{version}: {keyword}"
            );
        }
        let source = "<?php\nfunction accept(str";
        let uri = open(&backend, dir.path(), "scalar-hint", source).await;
        let pos = common::offset_position(source, source.len());
        let items = common::response_items(
            common::completion_response(&backend, &uri, pos.line, pos.character, None).await,
        )
        .unwrap();
        assert_eq!(
            items.iter().any(|item| item.label == "string"),
            version >= PhpVersion::new(7, 0)
        );
        for keyword in ["function", "const"] {
            let source = format!("<?php\nuse {}", &keyword[..1]);
            let uri = open(&backend, dir.path(), &format!("import-{keyword}"), &source).await;
            let pos = common::offset_position(&source, source.len());
            let items = common::response_items(
                common::completion_response(&backend, &uri, pos.line, pos.character, None).await,
            )
            .unwrap_or_default();
            assert_eq!(
                items.iter().any(|item| item.label == keyword),
                version.supports_function_imports()
            );
        }
    }
}

#[tokio::test]
async fn generated_edits_promoted_properties_keep_pseudo_types_documented() {
    let version = PhpVersion::new(8, 5);
    let (dir, backend) = project(version).await;
    let mut exported = Vec::new();
    for (name, ty, value, probe) in [
        (
            "resource",
            "resource",
            "fopen('php://memory', 'r')",
            "is_resource($object->value)",
        ),
        (
            "callback",
            "callable",
            "function () { return 42; }",
            "call_user_func($object->value) === 42",
        ),
    ] {
        let source = format!("<?php\nclass Holder {{\n/** @var {ty} */\npublic $value;\n}}\n");
        let uri = open(&backend, dir.path(), name, &source).await;
        let edited = apply(
            &backend,
            &uri,
            &source,
            "$value",
            false,
            "Generate promoted constructor",
        )
        .await;
        assert!(edited.contains(&format!("@param {ty} $value")), "{edited}");
        assert!(!edited.contains(&format!("public {ty} $value")), "{edited}");
        exported.push(runtime_case(
            name,
            &format!("{edited}\n$object = new Holder({value}); $probe = {probe};"),
            &json!(["boolean", true]),
        ));
    }
    save(version, "generated-promotion", exported);
}

#[tokio::test]
async fn generated_edits_add_return_contracts_without_legacy_native_hints() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 0),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let source = "<?php\nfunction answer() {\n    return 42;\n}\n";
        let uri = open(&backend, dir.path(), "add-return", source).await;
        common::inject_phpstan_diag(
            &backend,
            uri.as_str(),
            1,
            "Function answer() has no return type specified.",
            "missingType.return",
        );
        let edited = apply(
            &backend,
            &uri,
            source,
            "function answer",
            true,
            "Add return type",
        )
        .await;
        assert_eq!(
            edited.contains("answer(): int"),
            version.supports_return_types(),
            "{edited}"
        );
        if !version.supports_return_types() {
            assert!(edited.contains("@return 42"), "{edited}");
        }
        save(
            version,
            "generated-return",
            vec![runtime_case(
                "return-hint",
                &format!("{edited}\n$probe=answer();"),
                &json!(["integer", 42]),
            )],
        );
    }
}

#[tokio::test]
async fn generated_edits_complete_functions_and_overrides_for_the_target() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let helper = "<?php\nnamespace Library;\nconst NUMBER = 7;\nfunction legacy_helper() { return 7; }\n";
        open(&backend, dir.path(), "library", helper).await;
        let source = "<?php\nnamespace Caller;\n$probe = legacy_hel;\n";
        let uri = open(&backend, dir.path(), "function-completion", source).await;
        let start = source.find("legacy_hel").unwrap();
        let end = start + "legacy_hel".len();
        let pos = common::offset_position(source, end);
        let items = common::response_items(
            common::completion_response(&backend, &uri, pos.line, pos.character, None).await,
        )
        .unwrap();
        let item = items
            .iter()
            .find(|item| item.label.starts_with("legacy_helper("))
            .unwrap_or_else(|| panic!("{items:?}"));
        let mut edits = item.additional_text_edits.clone().unwrap_or_default();
        edits.push(TextEdit {
            range: Range::new(common::offset_position(source, start), pos),
            new_text: item.insert_text.as_ref().unwrap().replace("$0", ""),
        });
        let completed = common::apply_edits(source, &edits);
        assert_eq!(
            completed.contains("use function Library\\legacy_helper;"),
            version.supports_function_imports(),
            "{completed}"
        );
        let mut exported = vec![runtime_case(
            "function-import",
            &format!("{helper}\n{}", completed.trim_start_matches("<?php")),
            &json!(["integer", 7]),
        )];

        for (kind, name, expression) in [
            ("function", "legacy_helper", "\\Library\\legacy_helper()"),
            ("constant", "NUMBER", "\\Library\\NUMBER"),
        ] {
            let source = format!("<?php\nnamespace Caller;\n$probe = {expression};\n");
            let uri = open(
                &backend,
                dir.path(),
                &format!("import-action-{kind}"),
                &source,
            )
            .await;
            let pos = common::position_of(&source, name);
            let actions = common::code_actions_via_server(&backend, &uri, Range::new(pos, pos))
                .await
                .unwrap_or_default();
            let title = format!("Import {kind} `Library\\{name}` and shorten usages");
            let action = common::find_action_titled(&actions, &title);
            assert_eq!(
                action.is_some(),
                version.supports_function_imports(),
                "{version}: {actions:?}"
            );
            let edited = action.map_or_else(
                || source.clone(),
                |action| common::apply_workspace_edit(&source, action.edit.as_ref().unwrap()),
            );
            exported.push(runtime_case(
                &format!("import-action-{kind}"),
                &format!("{helper}\n{}", edited.trim_start_matches("<?php")),
                &json!(["integer", 7]),
            ));
        }

        let source = "<?php\nclass ParentView {\n/** @return string */\npublic function read() { return 'a'; }\n}\nclass ChildView extends ParentView {\n    public function re\n}\n";
        let uri = open(&backend, dir.path(), "override-completion", source).await;
        let start = source.rfind("re\n").unwrap();
        let pos = common::offset_position(source, start + 2);
        let items = common::response_items(
            common::completion_response(&backend, &uri, pos.line, pos.character, None).await,
        )
        .unwrap();
        let item = items
            .iter()
            .find(|item| item.label.starts_with("read("))
            .unwrap_or_else(|| panic!("{items:?}"));
        let mut edits = item.additional_text_edits.clone().unwrap_or_default();
        edits.push(TextEdit {
            range: Range::new(common::offset_position(source, start), pos),
            new_text: item
                .insert_text
                .as_ref()
                .unwrap()
                .replace("$0", "")
                .replace("\\$", "$"),
        });
        let completed = common::apply_edits(source, &edits);
        assert_eq!(
            completed.contains("read(): string"),
            version.supports_return_types(),
            "{completed}"
        );
        exported.push(runtime_case(
            "override",
            &format!("{completed}\n$probe=method_exists(new ChildView(), 'read');"),
            &json!(["boolean", true]),
        ));
        save(version, "generated-completion", exported);
    }
}

#[tokio::test]
async fn generated_edits_extract_interfaces_preserving_documented_types() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".phpantom.toml"),
            format!("[php]\nversion = \"{version}\"\n"),
        )
        .unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let backend = Backend::new_test_with_full_stubs();
        backend.initialize(serde_json::from_value(json!({
            "rootUri": Uri::from_file_path(&root).unwrap(),
            "capabilities": {"workspace": {"workspaceEdit": {"resourceOperations": ["create"]}}}
        })).unwrap()).await.unwrap();
        backend.initialized(InitializedParams {}).await;
        assert_eq!(backend.php_version(), version);
        let source = "<?php\nclass Service {\n/** @param DateTime $time\n * @return string */\npublic function read(DateTime $time) { return 'ok'; }\n}\n";
        let uri = open(&backend, dir.path(), "Service", source).await;
        let pos = common::position_of(source, "Service");
        let actions = common::code_actions_via_server(&backend, &uri, Range::new(pos, pos))
            .await
            .unwrap();
        let action = common::find_action_titled(&actions, "Extract interface").unwrap();
        let resolved = common::resolve_action(&backend, uri.as_str(), source, action);
        let edit = resolved.edit.unwrap();
        let interface_uri = edit
            .document_changes
            .as_ref()
            .unwrap()
            .iter()
            .find_map(|op| match op {
                DocumentChange::CreateFile(file) => Some(&file.uri),
                _ => None,
            })
            .unwrap();
        let interface =
            common::apply_edits("", &common::doc_change_edits_for_uri(&edit, interface_uri));
        let class = common::apply_edits(source, &common::doc_change_edits_for_uri(&edit, &uri));
        assert!(interface.contains("@return string"), "{interface}");
        let combined = format!(
            "{interface}\n{}\n$probe=(new Service())->read(new DateTime());",
            class.trim_start_matches("<?php")
        );
        save(
            version,
            "generated-interface",
            vec![runtime_case(
                "extract-interface",
                &combined,
                &json!(["string", "ok"]),
            )],
        );
    }
}
