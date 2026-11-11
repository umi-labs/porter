use porter::adapter::{TargetOptions, TargetWriter};
use porter::targets::payload::PayloadTarget;
use serde_json::json;
use std::fs;
use swc_common::{sync::Lrc, FileName, SourceMap};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsConfig};

fn assert_valid_typescript(path: &std::path::Path) {
    let source = fs::read_to_string(path).unwrap();
    let source_map = Lrc::new(SourceMap::default());
    let fm = source_map.new_source_file(FileName::Anon, source.clone());
    let lexer = Lexer::new(
        Syntax::Typescript(TsConfig::default()),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let parsed = parser.parse_module();
    assert!(
        parsed.is_ok() && parser.take_errors().is_empty(),
        "{} is not valid TypeScript:\n{}",
        path.display(),
        source
    );
}

#[test]
fn seed_files_are_valid_typescript_and_unique() {
    let out = tempfile::tempdir().unwrap();
    let docs = vec![
        json!({ "title": "Home", "slug": "home" }),
        json!({ "title": "Home", "slug": "home-2" }),
        json!({ "title": "Draft page", "_status": "draft" }),
    ];
    let opts = TargetOptions {
        collection: Some("pages".to_string()),
        locale: None,
        related_collections: None,
    };

    PayloadTarget::new()
        .emit_seed(&docs, out.path().to_str().unwrap(), &opts)
        .unwrap();

    let mut files: Vec<_> = fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let names: Vec<_> = files
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["draft_page.ts", "home.ts", "home_1.ts"]);

    for file in &files {
        assert_valid_typescript(file);
    }

    let home = fs::read_to_string(out.path().join("home.ts")).unwrap();
    assert!(home.contains(r#""_status": "published""#));
    assert!(fs::read_to_string(out.path().join("home_1.ts")).unwrap().contains(r#""slug": "home-2""#));
    // An explicit status from the source is kept
    assert!(fs::read_to_string(out.path().join("draft_page.ts")).unwrap().contains(r#""_status": "draft""#));
}
