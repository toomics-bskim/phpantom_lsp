use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn write(root: &Path, path: &str, source: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, source).unwrap();
}

fn analyze(root: &Path, destination: &Path, paths: &[&str]) -> Output {
    command(root)
        .arg("--dump-resolutions")
        .arg(destination)
        .args(paths)
        .output()
        .unwrap()
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_phpantom_lsp"));
    command.current_dir(root);
    command
        .args([
            "analyze",
            "--no-colour",
            "--format",
            "json",
            "--project-root",
        ])
        .arg(root)
        // Do not inherit a developer's global project defaults.
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("APPDATA", root.join("config"));
    command
}

fn rows(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn resolution_dump_uses_workspace_context_and_preserves_all_targets() {
    let fixture = r#"<?php
namespace App;
use Acme\Thing as Imported;
function inspect(Thing|Other $value, Imported $imported): void {
    $label = '한글😀'; $value->SAVE();
    $value->missing();
    $imported->send();
    helper();
    strlen('ok');
    missing_function();
    new Thing();
    new \Thing();
    new Missing();
}
"#;
    for newline in ["\n", "\r\n"] {
        let mut snapshots = Vec::new();
        for _ in 0..2 {
            let dir = tempfile::tempdir().unwrap();
            let root = dunce::canonicalize(dir.path()).unwrap();
            write(
                &root,
                "composer.json",
                r#"{"autoload":{"psr-4":{"App\\":"src/"}},"require":{"acme/lib":"*"}}"#,
            );
            write(
                &root,
                ".phpantom.toml",
                "[indexing]\nexclude = [\"src/excluded/**\"]\n",
            );
            write(
                &root,
                "vendor/composer/installed.json",
                r#"{"packages":[{"name":"acme/lib","install-path":"../acme/lib","autoload":{"psr-4":{"Acme\\":"src/"}}}]}"#,
            );
            write(
                &root,
                "vendor/acme/lib/src/Thing.php",
                "<?php namespace Acme; class Thing { public function send(): void {} }",
            );
            write(
                &root,
                "src/Base.php",
                "<?php namespace App; class Base { public function save(): void {} }",
            );
            write(
                &root,
                "src/Thing.php",
                "<?php namespace App; class Thing extends Base {}",
            );
            write(
                &root,
                "src/Other.php",
                "<?php namespace App; class Other { public function save(): void {} }",
            );
            write(&root, "src/global.php", "<?php class Thing {}");
            write(
                &root,
                "src/helpers.php",
                "<?php namespace App; function helper(): void {}",
            );
            write(&root, "src/excluded/Skip.php", "<?php ignored_call();");
            let source = fixture.replace('\n', newline);
            write(&root, "src/use.php", &source);
            let destination = root.join("references.jsonl");
            let output = analyze(&root, &destination, &[]);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let _: Value = serde_json::from_slice(&output.stdout).unwrap();
            let ordinary = command(&root).output().unwrap();
            assert_eq!(ordinary.status.code(), output.status.code());
            assert_eq!(
                serde_json::from_slice::<Value>(&ordinary.stdout).unwrap(),
                serde_json::from_slice::<Value>(&output.stdout).unwrap()
            );
            let records = rows(&destination);
            assert!(
                records
                    .iter()
                    .all(|row| !row["path"].as_str().unwrap().contains("vendor")
                        && !row["path"].as_str().unwrap().contains("excluded"))
            );
            let find = |name| {
                records
                    .iter()
                    .find(|row| row["path"] == "src/use.php" && row["name"] == name)
                    .unwrap()
            };
            assert_eq!(
                find("SAVE")["targets"],
                serde_json::json!(["App\\Base::save", "App\\Other::save"])
            );
            assert_eq!(
                find("send")["targets"],
                serde_json::json!(["Acme\\Thing::send"])
            );
            assert_eq!(
                find("helper")["targets"],
                serde_json::json!(["App\\helper"])
            );
            assert_eq!(find("strlen")["targets"], serde_json::json!(["strlen"]));
            for target in ["Thing", "App\\Thing", "Acme\\Thing"] {
                assert!(
                    records.iter().any(|row| row["path"] == "src/use.php"
                        && row["targets"] == serde_json::json!([target])),
                    "missing class target {target}"
                );
            }
            for name in ["missing", "missing_function", "Missing"] {
                assert_eq!(find(name)["targets"], serde_json::json!([]));
            }
            let call = find("SAVE");
            assert_eq!(call["line"], 5);
            assert_eq!(
                call["start"].as_u64().unwrap() as usize,
                source.find("SAVE").unwrap()
            );
            let prefix = source.lines().nth(4).unwrap().split("SAVE").next().unwrap();
            assert_eq!(
                call["column"].as_u64().unwrap() as usize,
                prefix.encode_utf16().count()
            );
            snapshots.push(fs::read(&destination).unwrap());
            let output = analyze(&root, &destination, &["src/use.php"]);
            assert!(matches!(output.status.code(), Some(0 | 1)));
            assert!(
                rows(&destination)
                    .iter()
                    .all(|row| row["path"] == "src/use.php")
            );
        }
        assert_eq!(
            snapshots[0], snapshots[1],
            "checkout roots must not affect targets or paths"
        );
    }
}

#[test]
fn resolution_dump_reports_io_errors_without_replacing_a_previous_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let destination = root.join("dump.jsonl");
    fs::write(&destination, "previous snapshot").unwrap();
    let output = analyze(&root, &destination, &["missing.php"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "previous snapshot"
    );
    let output = analyze(&root, &destination, &[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(fs::read(&destination).unwrap().is_empty());
    let output = analyze(&root, &root.join("absent/dump.jsonl"), &[]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn resolution_dump_reports_template_source_positions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let source = "<h1>한글</h1>\r\n{{ strlen('text') }}\r\n";
    write(&root, "view.blade.php", source);
    let destination = root.join("dump.jsonl");
    let output = analyze(&root, &destination, &[]);
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = rows(&destination);
    assert_eq!(
        records.len(),
        1,
        "generated template helpers are not source references: {records:?}"
    );
    let call = records.iter().find(|row| row["name"] == "strlen").unwrap();
    assert_eq!(call["line"], 2);
    assert_eq!(call["column"], 3);
    assert_eq!(
        call["start"].as_u64().unwrap() as usize,
        source.find("strlen").unwrap()
    );
    assert_eq!(
        call["end"].as_u64().unwrap() as usize,
        source.find("strlen").unwrap() + 6
    );
}

#[test]
fn resolution_dump_honors_the_configured_php_version() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    write(&root, ".phpantom.toml", "[php]\nversion = \"5.5\"\n");
    write(&root, "calls.php", "<?php get_debug_type(1); strlen('ok');");
    let destination = root.join("dump.jsonl");
    let output = analyze(&root, &destination, &[]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = rows(&destination);
    assert_eq!(
        records
            .iter()
            .find(|row| row["name"] == "get_debug_type")
            .unwrap()["targets"],
        serde_json::json!([])
    );
    assert_eq!(
        records.iter().find(|row| row["name"] == "strlen").unwrap()["targets"],
        serde_json::json!(["strlen"])
    );
}
