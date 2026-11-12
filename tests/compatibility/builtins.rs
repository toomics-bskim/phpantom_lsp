use super::*;
use phpantom_lsp::php_type::PhpType;

#[tokio::test]
async fn legacy_builtin_contracts_follow_target_and_stub_load_order() {
    for (major, minor) in [(5, 5), (5, 6), (7, 1), (7, 2), (7, 4), (8, 0), (8, 1)] {
        let version = PhpVersion::new(major, minor);
        for constants_first in [false, true] {
            let (dir, backend) = project(version).await;
            if constants_first {
                let text = "<?php\necho COUNT_NORMAL;\necho IMG_PNG;\necho SOCK_STREAM;\n";
                let uri = open(&backend, dir.path(), "constant-first", text).await;
                for line in 1..=3 {
                    assert!(
                        common::hover_response(&backend, &uri, line, 8)
                            .await
                            .is_some()
                    );
                }
            }
            for name in ["count", "sizeof"] {
                let func = backend.find_or_load_function(&[name]).unwrap();
                assert_eq!(
                    func.parameters[0].type_hint,
                    Some(PhpType::parse(if version < PhpVersion::new(7, 2) {
                        "mixed"
                    } else {
                        "Countable|array"
                    })),
                    "{version}: {name}"
                );
                assert!(
                    func.return_type
                        .as_ref()
                        .unwrap()
                        .is_subtype_of(&PhpType::int())
                );
            }
            for (name, class, boundary) in [
                ("curl_init", "CurlHandle", 0),
                ("openssl_pkey_new", "OpenSSLAsymmetricKey", 0),
                ("imagecreate", "GdImage", 0),
                ("socket_create", "Socket", 0),
                ("finfo_open", "finfo", 1),
                ("pg_connect", "\\PgSql\\Connection", 1),
            ] {
                let func = backend.find_or_load_function(&[name]).unwrap();
                let expected = if version < PhpVersion::new(8, boundary) {
                    "resource|false".to_owned()
                } else {
                    format!("{class}|false")
                };
                assert_eq!(
                    func.return_type.as_ref().map(|t| t.to_string()),
                    Some(PhpType::parse(&expected).to_string()),
                    "{version}: {name}"
                );
            }
            for (name, class) in [
                ("imagepng", "GdImage"),
                ("socket_close", "Socket"),
                ("curl_exec", "CurlHandle"),
            ] {
                let func = backend.find_or_load_function(&[name]).unwrap();
                let param = &func.parameters[0];
                assert_eq!(
                    param.type_hint.as_ref().map(|t| t.to_string()),
                    Some(if major < 8 { "resource" } else { class }.to_owned()),
                    "{version}: {name}"
                );
            }
            for name in ["bcdiv", "bcsqrt"] {
                let func = backend.find_or_load_function(&[name]).unwrap();
                assert_eq!(
                    func.return_type.as_ref().unwrap().accepts_null(),
                    major < 8,
                    "{version}: {name}"
                );
            }
            let substr = backend.find_or_load_function(&["substr"]).unwrap();
            assert_eq!(
                substr.return_type_str().as_deref(),
                Some(if major < 8 { "string|false" } else { "string" })
            );
            let strpos = backend.find_or_load_function(&["strpos"]).unwrap();
            assert!(strpos.return_type_str().unwrap().contains("false"));
            let encoding = backend
                .find_or_load_function(&["mb_convert_encoding"])
                .unwrap();
            assert_eq!(
                encoding.parameters[0].type_hint,
                Some(PhpType::parse(if version < PhpVersion::new(7, 2) {
                    "string"
                } else {
                    "array|string"
                }))
            );
        }
    }
}

#[tokio::test]
async fn legacy_builtin_calls_keep_valid_values_and_reject_invalid_arguments() {
    let source = "<?php\ncount(42);\nsizeof(null);\ncount(new stdClass());\ncount(array(1));\ncount();\ncount(array(), array());\nsubstr(array(), 1);\nimagepng(42);\nsocket_close(42);\nmb_convert_encoding(array('a'), 'UTF-8');\nfunction shadow_count(array $value) {}\nshadow_count(42);\n";
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 1),
        PhpVersion::new(7, 2),
        PhpVersion::new(8, 0),
    ] {
        let (dir, backend) = project(version).await;
        let uri = open(&backend, dir.path(), "builtin-calls", source).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        let errors = |line| {
            diagnostics.iter().any(|d| d.range.start.line == line
            && matches!(&d.code, Some(Code::String(c)) if c.starts_with("type_mismatch") || c.contains("argument_count")))
        };
        for line in [1, 2, 3, 10] {
            assert_eq!(
                errors(line),
                if line == 10 {
                    version < PhpVersion::new(7, 2)
                } else {
                    version >= PhpVersion::new(7, 2)
                },
                "{version}: line {line}: {diagnostics:?}"
            );
        }
        assert!(!errors(4), "{diagnostics:?}");
        for line in [5, 6, 7, 8, 9, 12] {
            assert!(
                errors(line),
                "{version}: missing invalid call at {line}: {diagnostics:?}"
            );
        }
    }
}

#[tokio::test]
async fn legacy_builtin_runtime_controls() {
    let fixtures: Value = serde_json::from_str(include_str!("builtins.json")).unwrap();
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        let mut exported = Vec::new();
        for case in fixtures.as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let source = format!("<?php\n{}\n$probe;\n", case["body"].as_str().unwrap());
            let uri = open(&backend, dir.path(), id, &source).await;
            let diagnostics = ready_diagnostics(&backend, &uri).await;
            assert!(
                !diagnostics.iter().any(
                    |d| matches!(&d.code, Some(Code::String(c)) if c.starts_with("type_mismatch"))
                ),
                "{id}: {diagnostics:?}"
            );
            let pos = common::position_of_last(&source, "$probe;");
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
            assert!(
                hover.contains(case["hover"].as_str().unwrap()),
                "{id}: {hover}"
            );
            exported.push(runtime_case(id, &source, &case["value"]));
        }
        if let Ok(output) = std::env::var("PHPANTOM_COMPAT_OUTPUT") {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(
                Path::new(&output).join(format!("builtins-{version}.json")),
                serde_json::to_vec_pretty(&json!({"target":version.to_string(), "cases":exported}))
                    .unwrap(),
            )
            .unwrap();
        }
    }
}

#[tokio::test]
async fn legacy_builtin_resource_consumers_and_count_branches_remain_sound() {
    let source = r#"<?php
function check_resources() {
    $image = imagecreate(1, 1);
    if ($image !== false) { imagepng($image); }
    $socket = socket_create(2, 1, 0);
    if ($socket !== false) { socket_close($socket); }
    $curl = curl_init();
    if ($curl !== false) { curl_exec($curl); }
    $value = false;
    if (count($value) > 0) { $scalar = $value; $scalar; }
    $value = null;
    if (count($value) === 0) { $nothing = $value; $nothing; }
}
"#;
    for version in [PhpVersion::new(5, 5), PhpVersion::new(5, 6)] {
        let (dir, backend) = project(version).await;
        let uri = open(&backend, dir.path(), "resource-consumers", source).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        assert!(
            !diagnostics.iter().any(
                |d| matches!(&d.code, Some(Code::String(c)) if c.starts_with("type_mismatch"))
            ),
            "{diagnostics:?}"
        );
        for (name, expected) in [("$scalar;", "false"), ("$nothing;", "null")] {
            let pos = common::position_of_last(source, name);
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
            assert!(hover.contains(expected), "{hover}");
        }
    }
}
