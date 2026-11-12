use super::*;

#[tokio::test]
async fn constructor_scope_lookup_keeps_constants_and_properties_distinct() {
    let (dir, backend) = project(PhpVersion::new(5, 5)).await;
    let source = "<?php\nclass Example {\n const __construct = 42;\n static $__construct = 'property';\n function Example(DateTime $value) {}\n}\nExample::__construct;\nExample::$__construct;\n";
    let uri = open(&backend, dir.path(), "members", source).await;
    for (needle, declaration, value) in [
        ("Example::__construct;", "__construct = 42", "42"),
        (
            "Example::$__construct;",
            "$__construct = 'property'",
            "static $__construct",
        ),
    ] {
        let pos =
            common::offset_position(source, source.find(needle).unwrap() + "Example::".len() + 2);
        let locations = common::definition_locations(
            common::goto_definition_at(&backend, &uri, pos.line, pos.character).await,
        );
        assert_eq!(locations.len(), 1, "{locations:?}");
        assert_eq!(
            locations[0].range.start.line,
            common::position_of(source, declaration).line
        );
        let hover = common::markup_hover_at(&backend, &uri, pos.line, pos.character).await;
        assert!(
            hover.contains(value) && !hover.contains("DateTime"),
            "{hover}"
        );
    }
}

#[tokio::test]
async fn legacy_constructor_scope_calls_and_access_follow_the_selected_declaration() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(7, 4),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let source = r#"<?php
class LegacyBase {
    function LegacyBase(DateTime $value) {}
}
class Child extends LegacyBase {
    function __construct() { parent::__construct(new DateTime()); }
}
class Inherited extends LegacyBase {
    function repeat() { self::__construct(new DateTime()); }
    function LegacyBase($ordinary) {}
}
class Locked {
    private function Locked() {}
    static function create() { return new LockedChild(); }
}
class LockedChild extends Locked {}
class ModernLocked { private function __construct() {} }
class ModernChild extends ModernLocked {}
class Missing { static function __callStatic($name, $arguments) {} }
class MissingChild extends Missing { function __construct() { parent::__construct(); } }
new LockedChild();
new ModernChild();
$base = new LegacyBase(new DateTime());
$base->__construct(new DateTime());
"#;
        let uri = open(&backend, dir.path(), "access", source).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        for (needle, code, expected) in [
            ("parent::__construct", "unknown_member", version.major >= 8),
            ("self::__construct", "unknown_member", version.major >= 8),
            (
                "new LockedChild();\nnew ModernChild",
                "inaccessible_member",
                version.major < 8,
            ),
            ("new ModernChild();", "inaccessible_member", true),
            ("$base->__construct", "unknown_member", true),
            ("return new LockedChild", "inaccessible_member", false),
            ("class MissingChild", "unknown_member", true),
        ] {
            let line = common::position_of(source, needle).line;
            let found = diagnostics
                .iter()
                .any(|d| d.range.start.line == line && d.code == Some(Code::String(code.into())));
            assert_eq!(found, expected, "{version}: {needle}: {diagnostics:?}");
        }
        if version.major < 8 {
            for subject in ["parent", "self"] {
                let pos = common::position_after(source, &format!("{subject}::"));
                let targets = common::definition_locations(
                    common::goto_definition_at(&backend, &uri, pos.line, pos.character + 2).await,
                );
                assert_eq!(targets.len(), 1, "{version}: {targets:?}");
                assert_eq!(
                    targets[0].range.start,
                    common::position_of(source, "LegacyBase(DateTime")
                );
                let hover =
                    common::markup_hover_at(&backend, &uri, pos.line, pos.character + 2).await;
                assert!(
                    hover.contains("DateTime") && hover.contains("LegacyBase"),
                    "{hover}"
                );
                let pos = common::position_after(source, &format!("{subject}::__construct("));
                let signature = backend
                    .signature_help(
                        serde_json::from_value(json!({"textDocument":{"uri":uri},"position":pos}))
                            .unwrap(),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    signature.signatures[0].label.contains("DateTime $value"),
                    "{signature:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn constructor_access_tracks_scope_traits_and_namespace_identity() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let (dir, backend) = project(version).await;
        let legacy = version.major < 8;
        let constructor = if legacy { "Locked" } else { "__construct" };
        let source = format!(
            r#"<?php
namespace {{
trait Initializer {{ function initialize() {{}} }}
class Locked {{
    use Initializer {{ initialize as private {constructor}; }}
    static function create() {{ return new LockedChild; }}
    static function local() {{ return new self; }}
    static function __callStatic($name, $arguments) {{}}
    function __call($name, $arguments) {{}}
}}
class LockedChild extends Locked {{
    static function denied() {{ return new self; }}
    function scoped() {{ parent::__construct(); }}
}}
new Locked;
new LockedChild;
$class = 'LockedChild'; new $class;
class ProtectedBase {{ protected function __construct() {{}} }}
class Allowed extends ProtectedBase {{
    static function create() {{ return new ProtectedBase; }}
}}
new ProtectedBase;
class Magic {{
    private function __construct() {{}}
    static function __callStatic($name, $args) {{}}
    function __call($name, $args) {{}}
}}
new Magic;
}}
namespace Other {{
use \LockedChild as Imported;
class Locked {{ static function denied() {{ return new Imported; }} }}
}}
"#
        );
        let uri = open(&backend, dir.path(), "scopes", &source).await;
        let diagnostics = ready_diagnostics(&backend, &uri).await;
        for (needle, expected) in [
            ("return new LockedChild", false),
            ("function local()", false),
            ("function denied() { return new self", true),
            ("parent::__construct", true),
            ("new Locked;", true),
            ("new LockedChild;\n$class", true),
            ("new $class", true),
            ("return new ProtectedBase", false),
            ("new ProtectedBase;\nclass", true),
            ("new Magic", true),
            ("return new Imported", true),
        ] {
            let line = common::position_of(&source, needle).line;
            assert_eq!(
                diagnostics.iter().any(|d| d.range.start.line == line
                    && d.code == Some(Code::String("inaccessible_member".into()))),
                expected,
                "{version}: {needle}: {diagnostics:?}"
            );
        }
    }
}

#[tokio::test]
async fn constructor_access_runtime_contract() {
    for version in [
        PhpVersion::new(5, 5),
        PhpVersion::new(5, 6),
        PhpVersion::new(8, 5),
    ] {
        let legacy = version.major < 8;
        let constructor = if legacy { "Base" } else { "__construct" };
        let mut cases = Vec::new();
        let positive = format!(
            "<?php class Base {{ public $value; function {constructor}($value) {{ $this->value = $value; }} }} class Child extends Base {{ function __construct() {{ parent::__construct(42); }} }} $probe = (new Child())->value;"
        );
        cases.push(runtime_case(
            "parent_scope",
            &positive,
            &json!(["integer", 42]),
        ));
        cases.push(json!({"id":"private_scope_never_magic","source":format!("<?php class Base {{ private function {constructor}() {{}} static function __callStatic($name, $args) {{}} function __call($name, $args) {{}} }} class Child extends Base {{ function __construct() {{ parent::__construct(); }} }} new Child;"),"lint":true,"runtime":{"expectedExit":255,"message":"private Base::__construct"}}));
        cases.push(json!({"id":"missing_scope_never_magic","source":"<?php class Base { static function __callStatic($name, $args) {} function __call($name, $args) {} } class Child extends Base { function __construct() { parent::__construct(); } } new Child;","lint":true,"runtime":{"expectedExit":255,"message":"Cannot call constructor"}}));
        for (id, declaration) in [
            ("own_private", "class Child extends Base {}"),
            (
                "trait_private",
                "trait Init { private function init() {} } class Base { use Init { init as private CTOR; } } class Child extends Base {} ",
            ),
        ] {
            let source = if id == "own_private" {
                format!(
                    "<?php class Base {{ private function {constructor}() {{}} }} {declaration} new Child();"
                )
            } else {
                format!(
                    "<?php {} new Child();",
                    declaration.replace("CTOR", constructor)
                )
            };
            cases.push(json!({"id":id,"source":source,"lint":true,"runtime":{"expectedExit":255,"message":"private"}}));
        }
        let allowed = format!(
            "<?php class Base {{ private function {constructor}() {{}} static function create() {{ return new Child; }} }} class Child extends Base {{}} $probe = get_class(Base::create());"
        );
        cases.push(runtime_case(
            "owner_scope",
            &allowed,
            &json!(["string", "Child"]),
        ));
        if legacy {
            cases.push(runtime_case("self_inherited_constructor", "<?php class Base { public $value; function Base($value) { $this->value = $value; } } class Child extends Base { function Base($value) { $this->value = 'ordinary'; } function repeat() { self::__construct(42); } } $child = new Child(0); $child->repeat(); $probe = $child->value;", &json!(["integer",42])));
            cases.push(json!({"id":"arrow_not_alias","source":"<?php class Base { function Base() {} } $base = new Base; $base->__construct();","lint":true,"runtime":{"expectedExit":255,"message":"undefined method Base::__construct"}}));
        } else {
            cases.push(json!({"id":"modern_no_legacy_alias","source":"<?php class Base { function Base() {} } class Child extends Base { function __construct() { parent::__construct(); } } new Child;","lint":true,"runtime":{"expectedExit":255,"message":"Cannot call constructor"}}));
        }
        if let Some(output) = std::env::var_os("PHPANTOM_COMPAT_OUTPUT") {
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(
                Path::new(&output).join(format!("constructor-access-{version}.json")),
                serde_json::to_vec_pretty(&json!({"target":version.to_string(),"cases":cases}))
                    .unwrap(),
            )
            .unwrap();
        }
    }
}
