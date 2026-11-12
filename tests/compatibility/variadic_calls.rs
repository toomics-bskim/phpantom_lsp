use super::*;

#[tokio::test]
async fn call_arguments_reach_unannotated_function_returns() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let source = "<?php\nfunction identity($value) { return $value; }\n$a = identity('first');\n$b = identity(42);\necho $a;\necho $b;\n";
        let uri = open(&backend, dir.path(), "ordinary_arguments", source).await;
        for (needle, expected) in [("echo $a", "'first'"), ("echo $b", "42")] {
            let pos = common::position_after(source, needle);
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character - 1).await;
            assert!(hover.contains(expected), "{version}: {hover}");
        }
    }
}

#[tokio::test]
async fn variadic_and_unpacked_calls_preserve_argument_positions() {
    for version in [PhpVersion::new(5, 6), PhpVersion::new(8, 5)] {
        let (dir, backend) = project(version).await;
        let source = r#"<?php
function first(...$items) { return $items[0]; }
function second($first, $second) { return $second; }
class Picks {
    function first(...$items) { return $items[0]; }
    function second($first, $second) { return $second; }
}

$a = first('first', 42);
$b = second(...array('a', 'b'));
$c = first(...array('packed', 42));
$object = new Picks();
$d = $object->first('method', 42);
$e = $object->second(...array('a', 'second'));
echo $a;
echo $b;
echo $c;
echo $d;
echo $e;
"#;
        let uri = open(&backend, dir.path(), "packed_arguments", source).await;
        for (variable, expected) in [
            ("a", "'first'"),
            ("b", "'b'"),
            ("c", "'packed'"),
            ("d", "'method'"),
            ("e", "'second'"),
        ] {
            let pos = common::position_after(source, &format!("echo ${variable}"));
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character - 1).await;
            assert!(hover.contains(expected), "{version}: {variable}: {hover}");
        }
    }
}

#[tokio::test]
async fn unpacked_call_chains_share_argument_types_with_completion_and_diagnostics() {
    for version in [PhpVersion::new(5, 6), PhpVersion::new(8, 5)] {
        let (dir, backend) = project(version).await;
        let mut config = backend.config();
        config.diagnostics.unresolved_member_access = Some(true);
        backend.set_config(config);
        let source = r#"<?php
class Item { function found() {} }
function first(...$items) { return $items[0]; }
function second($first, $second) { return $second; }
class Picks { function second($first, $second) { return $second; } }
first(...array(new Item()))->found();
second(...array(42, new Item()))->found();
(new Picks())->second(...array(42, new Item()))->found();
first(...array(42))->found();
"#;
        let uri = open(&backend, dir.path(), "unpacked_chains", source).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        let scalar_lines: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == Some(Code::String("scalar_member_access".into())))
            .map(|d| d.range.start.line)
            .collect();
        assert_eq!(scalar_lines, [8], "{version}: {diagnostics:?}");
        assert!(!diagnostics.iter().any(|d| matches!(&d.code, Some(Code::String(code)) if code == "unknown_member" || code == "unresolved_member_access")), "{version}: {diagnostics:?}");
        for needle in [
            "first(...array(new Item()))->",
            "second(...array(42, new Item()))->",
            "(new Picks())->second(...array(42, new Item()))->",
        ] {
            let pos = common::position_after(source, needle);
            let labels =
                common::complete_labels_at_opened(&backend, &uri, pos.line, pos.character).await;
            assert!(
                labels.iter().any(|label| label.starts_with("found")),
                "{version}: {needle}: {labels:?}"
            );
        }
    }
}

#[tokio::test]
async fn unpacked_generics_and_unknown_length_packs_keep_element_types() {
    for version in [PhpVersion::new(5, 6), PhpVersion::new(8, 5)] {
        let (dir, backend) = project(version).await;
        let source = r#"<?php
/** @template T
 * @param T $value
 * @return T */
function genericFirst($value) { return $value; }
/** @template T
 * @param T ...$items
 * @return T */
function genericPack(...$items) { return $items[0]; }
/** @template T
 * @param class-string<T> $class
 * @return T */
function createFromName($class) { return new $class(); }
/** @template T
 * @param T[] $items
 * @return T */
function arrayFirst($items) { return $items[0]; }
/** @return ($value is null ? Exception : DateTime) */
function chooseType($value) { return $value === null ? new Exception() : new DateTime(); }
function firstPack(...$items) { return $items[0]; }
function secondValue($first, $second) { return $second; }
/** @param list<DateTime> $items */
function consume($items) {
    $a = genericFirst(...$items);
    $b = genericPack(...$items);
    $c = firstPack(...$items);
    $d = secondValue(...$items);
    echo $a;
    echo $b;
    echo $c;
    echo $d;
}
$e = genericFirst(...array(new DateTime()));
$f = genericPack(...array(new DateTime(), new DateTime()));
echo $e;
$g = createFromName(...array(DateTime::class));
$h = arrayFirst(...array(array(new DateTime())));
$i = chooseType(...array(42));
$j = chooseType(...array(null));
echo $f;
echo $g;
echo $h;
echo $i;
echo $j;
"#;
        let uri = open(&backend, dir.path(), "generic_arguments", source).await;
        for variable in ["a", "b", "c", "d", "e", "f", "g", "h", "i"] {
            let pos = common::position_after(source, &format!("echo ${variable}"));
            let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character - 1).await;
            assert!(hover.contains("DateTime"), "{version}: {variable}: {hover}");
            if variable == "i" {
                assert!(!hover.contains("Exception"), "{version}: {hover}");
            }
        }
        let pos = common::position_after(source, "echo $j");
        let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character - 1).await;
        assert!(
            hover.contains("Exception") && !hover.contains("DateTime"),
            "{version}: {hover}"
        );
    }
}

#[tokio::test]
async fn unpacked_arguments_are_checked_against_their_parameter_types() {
    for version in [PhpVersion::new(5, 6), PhpVersion::new(8, 5)] {
        let (dir, backend) = project(version).await;
        let source = r#"<?php
function accept(DateTime $first, Exception $second) {}
function many(DateTime ...$values) {}
accept(...array(new DateTime(), new Exception()));
accept(...array(new DateTime(), new DateTime()));
many(...array(new DateTime(), new Exception()));
/** @param list<Exception> $items */
function unknownPack($items) { many(...$items); }
/** @param list<DateTime> $items */
function validPack($items) { many(...$items); }
"#;
        let uri = open(&backend, dir.path(), "argument_checks", source).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        let lines: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == Some(Code::String("type_mismatch_argument".into())))
            .map(|d| d.range.start.line)
            .collect();
        assert_eq!(lines, [4, 5, 7], "{version}: {diagnostics:?}");
    }
}

#[tokio::test]
async fn named_unpacking_and_cross_file_calls_keep_declaration_context() {
    let (dir, backend) = project(PhpVersion::new(8, 5)).await;
    open(&backend, dir.path(), "callee", "<?php namespace Library; class Value {} function choose($first, $second) { return $second; } function packed(...$values) { return $values['chosen']; }").await;
    let source = "<?php namespace Caller; class Value {}\n$a = \\Library\\choose(...['second' => new Value(), 'first' => new \\Library\\Value()]);\n$b = \\Library\\packed(chosen: new Value());\necho $a;\necho $b;\n";
    let uri = open(&backend, dir.path(), "caller", source).await;
    for variable in ["a", "b"] {
        let pos = common::position_after(source, &format!("echo ${variable}"));
        let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character - 1).await;
        assert!(
            hover.contains("namespace Caller;") && hover.contains("= Value"),
            "{variable}: {hover}"
        );
    }
}

#[tokio::test]
async fn variadic_syntax_remains_unavailable_in_php55() {
    let (dir, backend) = project(PhpVersion::new(5, 5)).await;
    let source = "<?php\nfunction first(...$items) { return $items[0]; }\n$probe = first(...array('first'));\n";
    let uri = open(&backend, dir.path(), "syntax_boundary", source).await;
    let diagnostics = ready_diagnostics(&backend, &uri).await;
    for line in [1, 2] {
        assert!(
            diagnostics
                .iter()
                .any(|d| d.range.start.line == line && format!("{:?}", d.message).contains("5.6")),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn variadic_calls_runtime_contract() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let mut cases = vec![runtime_case(
            "ordinary",
            "<?php function identity($value) { return $value; } $probe = identity('first');",
            &json!(["string", "first"]),
        )];
        for (id, source, expected) in [
            (
                "variadic",
                "<?php function first(...$items) { return $items[0]; } $probe = first('first', 42);",
                json!(["string", "first"]),
            ),
            (
                "unpacked",
                "<?php function second($a, $b) { return $b; } $probe = second(...array('a', 'b'));",
                json!(["string", "b"]),
            ),
            (
                "numeric_keys",
                "<?php function second($a, $b) { return $b; } $probe = second(...array(9 => 'a', 2 => 'b'));",
                json!(["string", "b"]),
            ),
            (
                "multiple_packs",
                "<?php function packed(...$items) { return $items; } $probe = packed(...array(1), ...array('two'));",
                json!(["array", [1, "two"]]),
            ),
        ] {
            if version == PhpVersion::new(5, 5) {
                cases.push(json!({"id":id,"source":source,"lint":false}));
            } else {
                cases.push(runtime_case(id, source, &expected));
            }
        }
        if version.major >= 8 {
            cases.push(runtime_case("named_pack", "<?php function packed(...$items) { return $items['chosen']; } $probe = packed(...['chosen' => 'named']);", &json!(["string", "named"])));
        }
        if let Some(output) = std::env::var_os("PHPANTOM_COMPAT_OUTPUT") {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(
                Path::new(&output).join(format!("variadic-calls-{version}.json")),
                serde_json::to_vec_pretty(&json!({"target":version.to_string(),"cases":cases}))
                    .unwrap(),
            )
            .unwrap();
        }
    }
}
#[tokio::test]
async fn argument_packs_keep_numeric_order_empty_tails_and_call_site_cache_keys() {
    let (dir, backend) = project(PhpVersion::new(5, 6)).await;
    let source = r#"<?php
function second($first, $second) { return $second; }
function collectArguments(...$items) { return $items; }
function identity($value) { return $value; }
function acceptDate(DateTime $value) {}
function error(Exception $value) {}
$a = second(...array(9 => 'first', 2 => 'second'));
$b = collectArguments(...array('first'), ...array(42));
$c = collectArguments();
echo $a;
echo $b;
echo $c;
acceptDate(identity(new DateTime()));
error(identity(new Exception()));
acceptDate(identity(new Exception()));
"#;
    let uri = open(&backend, dir.path(), "pack_edges", source).await;
    for (variable, expected) in [("a", "'second'"), ("b", "42"), ("c", "array{}")] {
        let pos = common::position_after(source, &format!("echo ${variable}"));
        let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character - 1).await;
        assert!(hover.contains(expected), "{variable}: {hover}");
        if variable == "a" {
            assert!(!hover.contains("first"), "{hover}");
        }
    }
    let diagnostics = ready_diagnostics(&backend, &uri).await;
    let lines: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == Some(Code::String("type_mismatch_argument".into())))
        .map(|d| d.range.start.line)
        .collect();
    assert_eq!(lines, [14], "{diagnostics:?}");
}

#[tokio::test]
async fn uncertain_unpacked_generic_arguments_include_parameter_defaults() {
    let (dir, backend) = project(PhpVersion::new(5, 6)).await;
    let source = r#"<?php
/** @template T
 * @param T $value
 * @return T */
function optionalValue($value = null) { return $value; }
/** @param list<DateTime> $items */
function caller($items) {
    $value = optionalValue(...$items);
    echo $value;
}
"#;
    let uri = open(&backend, dir.path(), "uncertain_default", source).await;
    let pos = common::position_after(source, "echo $value");
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character - 1).await;
    assert!(
        hover.contains("DateTime") && (hover.contains("null") || hover.contains("?DateTime")),
        "{hover}"
    );
}

#[tokio::test]
async fn unpacked_numeric_string_keys_remain_named_variadic_entries() {
    let (dir, backend) = project(PhpVersion::new(8, 5)).await;
    let source = "<?php\nfunction selected(...$values) { return $values['01']; }\n$value = selected(...['01' => new DateTime()]);\necho $value;\n";
    let uri = open(&backend, dir.path(), "numeric_named_key", source).await;
    let pos = common::position_after(source, "echo $value");
    let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character - 1).await;
    assert!(hover.contains("DateTime"), "{hover}");
}
