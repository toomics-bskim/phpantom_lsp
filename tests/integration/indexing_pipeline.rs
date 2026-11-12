use crate::common::{create_psr4_workspace, find_references, line_char_of, open_php};
use tower_lsp_server::LanguageServer;
use tower_lsp_server::gen_lsp_types::*;

#[tokio::test]
async fn workspace_parse_references_use_each_files_own_names_and_imports() {
    let library =
        "<?php\nnamespace Library;\nclass Item {}\nfunction helper() {}\nconst FLAG = 1;\n";
    let global = "<?php\nclass Item {}\nfunction helper() {}\nconst FLAG = 2;\n";
    let caller = "<?php\r\nnamespace First {\r\nuse Library\\Item as Alias;\r\nuse function Library\\helper as call_it;\r\nuse const Library\\FLAG as VALUE;\r\nnew Alias; call_it(); echo VALUE;\r\n}\r\nnamespace Second { new \\Item; \\helper(); echo \\FLAG; }\r\n";
    let (backend, dir) = create_psr4_workspace(
        r#"{"autoload":{"psr-4":{"Library\\":"lib/"}}}"#,
        &[
            ("lib/Item.php", library),
            ("global.php", global),
            ("caller.php", caller),
        ],
    );
    backend.ensure_workspace_indexed();
    let caller_uri =
        Uri::from_file_path(dunce::canonicalize(dir.path().join("caller.php")).unwrap()).unwrap();
    for (file, source, expected_line) in [("lib/Item.php", library, 5), ("global.php", global, 7)] {
        let uri = Uri::from_file_path(dunce::canonicalize(dir.path().join(file)).unwrap()).unwrap();
        open_php(&backend, &uri, source).await;
        for needle in ["Item", "helper", "FLAG"] {
            let (line, character) = line_char_of(source, needle);
            let locations =
                find_references(&backend, &uri, Position { line, character }, false).await;
            assert!(
                locations.iter().any(|location| location.uri == caller_uri
                    && location.range.start.line == expected_line),
                "{file}: {needle}: {locations:?}"
            );
            assert!(
                !locations.iter().any(|location| location.uri == caller_uri
                    && location.range.start.line == if expected_line == 5 { 7 } else { 5 }),
                "cross-namespace false reference: {locations:?}"
            );
        }
    }
}

#[tokio::test]
async fn installed_package_chunks_keep_references_and_refresh_after_edits() {
    let declaration = "<?php\nnamespace Shared;\nclass Base {}\n";
    let (backend, dir) = create_psr4_workspace(
        r#"{"require":{"acme/shared":"*"},"autoload":{"psr-4":{"Shared\\":"src/"}}}"#,
        &[
            ("src/Base.php", declaration),
            (
                "vendor/composer/installed.json",
                r#"{"packages":[{"name":"acme/shared","install-path":"../../packages/shared","autoload":{"psr-4":{"Shared\\":""}}}]}"#,
            ),
            (".phpantom.toml", "[php]\nversion = \"5.5\"\n"),
        ],
    );
    let package = dir.path().join("packages/shared");
    std::fs::create_dir_all(&package).unwrap();
    for index in 0..2050 {
        let parent = if index == 0 || index == 2049 {
            " extends Base"
        } else {
            ""
        };
        std::fs::write(
            package.join(format!("Item{index:04}.php")),
            format!("<?php namespace Shared; class Item{index:04}{parent} {{}}\n"),
        )
        .unwrap();
    }
    backend.initialized(InitializedParams {}).await;
    backend.ensure_workspace_indexed();
    let declaration_uri =
        Uri::from_file_path(dunce::canonicalize(dir.path().join("src/Base.php")).unwrap()).unwrap();
    open_php(&backend, &declaration_uri, declaration).await;
    let (line, character) = line_char_of(declaration, "Base");
    let references = find_references(
        &backend,
        &declaration_uri,
        Position { line, character },
        false,
    )
    .await;
    assert_eq!(references.len(), 2, "{references:?}");
    let changed_path = dunce::canonicalize(package.join("Item2049.php")).unwrap();
    let changed_uri = Uri::from_file_path(&changed_path).unwrap();
    std::fs::write(&changed_path, "<?php namespace Shared; class Item2049 {}\n").unwrap();
    backend
        .did_change_watched_files(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: changed_uri.clone(),
                kind: FileChangeType::Changed,
            }],
        })
        .await;
    let references = find_references(
        &backend,
        &declaration_uri,
        Position { line, character },
        false,
    )
    .await;
    assert_eq!(references.len(), 1, "{references:?}");
    assert_ne!(references[0].uri, changed_uri);
}
