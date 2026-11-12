use porter_engine::destinations::payload::ts::{DestField, DestSchema, read_payload_config};
use porter_engine::destinations::payload::{schema_check, schema_generate, schema_prompt};
use porter_engine::mapping::summary::{FieldStats, TypeStats, TypeSummary};
use porter_engine::mapping::{BlockMap, BlockMatch, FieldMap, KeyType, MappingConfig, TypeMap};
use porter_engine::project::Project;
use porter_model::{
    DocStatus, Document, FieldValue, IssueKind, ItemId, ItemKind, RichText, Seo, Severity,
    SourceRef,
};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn field<'a>(fs: &'a [DestField], name: &str) -> &'a DestField {
    fs.iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name} in {fs:?}"))
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn stats(variants: &[&str], samples: &[&str]) -> FieldStats {
    FieldStats {
        variants: variants.iter().map(|v| v.to_string()).collect(),
        filled: samples.len().max(1),
        fill_rate: 1.0,
        samples: samples.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn summary(ty: &str, fields: &[(&str, FieldStats)]) -> TypeSummary {
    let mut s = TypeSummary::default();
    s.types.insert(
        ty.into(),
        TypeStats {
            count: 3,
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        },
    );
    s
}

fn mapping(ty: &str, to: &str, fields: &[(&str, &str)]) -> MappingConfig {
    let mut m = MappingConfig::default();
    m.types.insert(
        ty.into(),
        TypeMap {
            to: Some(to.into()),
            fields: fields
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        FieldMap {
                            to: v.to_string(),
                            ..FieldMap::default()
                        },
                    )
                })
                .collect(),
            ..TypeMap::default()
        },
    );
    m
}

const PAGES_WITH_HERO: &str = r#"
import type { CollectionConfig } from 'payload'
export const Pages: CollectionConfig = {
  slug: 'pages',
  fields: [
    { name: 'title', type: 'text', required: true },
    { name: 'legacyId', type: 'text', index: true, unique: true },
    { name: 'hero', type: 'text' },
  ],
}
"#;

fn errors(issues: &[porter_engine::html::PendingIssue]) -> Vec<String> {
    issues
        .iter()
        .filter(|i| i.kind == IssueKind::SchemaMismatch && i.severity == Severity::Error)
        .map(|i| i.message.clone())
        .collect()
}

#[test]
fn reads_test_app_schema() {
    let s = read_payload_config(&repo().join("tests/payload-app")).unwrap();
    let pages = &s.collections["pages"];
    assert!(pages.drafts);
    assert!(!pages.upload);
    assert_eq!(field(&pages.fields, "legacyId").kind, "text");
    assert!(field(&pages.fields, "title").required);
    let layout = field(&pages.fields, "layout");
    assert_eq!(layout.kind, "blocks");
    let names: Vec<&str> = layout.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["richText", "image", "raw"]);
    assert_eq!(field(&layout.fields[1].fields, "image").kind, "upload");
    assert_eq!(
        field(&layout.fields[1].fields, "image").relation_to,
        ["media"]
    );
    assert!(s.blocks.contains_key("raw"));
    let seo = field(&pages.fields, "seo");
    assert_eq!(seo.kind, "group");
    assert_eq!(field(&seo.fields, "description").kind, "textarea");
    assert!(s.collections["media"].upload);
    assert!(s.collections.contains_key("posts"));
}

#[test]
fn reads_fixture_collections_and_warns_on_unresolved() {
    let s = read_payload_config(&repo().join("test-data")).unwrap();
    let hotels = &s.collections["hotels"];
    let amenities = field(&hotels.fields, "amenities");
    assert_eq!(amenities.kind, "relationship");
    assert!(amenities.many);
    assert_eq!(amenities.relation_to, ["amenities"]);
    assert_eq!(field(&hotels.fields, "coordinates").kind, "point");
    // `tags(...)`/`address(...)` come from files that aren't in the fixture: warned, not dropped silently.
    assert!(
        s.warnings.iter().any(|w| w.contains("tags")),
        "{:?}",
        s.warnings
    );
}

#[test]
fn follows_aliases_imports_tabs_and_polymorphic_relations() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "tsconfig.json",
        r#"{
  // comments and trailing commas are fine
  "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"], }, },
}"#,
    );
    write(
        d.path(),
        "src/blocks/Hero.ts",
        "import type { Block } from 'payload'\nexport const Hero: Block = { slug: 'hero', fields: [{ name: 'heading', type: 'text', required: true }] }\n",
    );
    write(
        d.path(),
        "src/fields/link.ts",
        "export const link = ({ name = 'link', many = false } = {}) => ({ name, type: 'relationship', relationTo: ['pages', 'posts'], hasMany: many })\n",
    );
    write(
        d.path(),
        "src/collections/Pages.ts",
        r#"
import type { CollectionConfig } from 'payload'
import { Hero } from '@/blocks/Hero'
import { link } from '../fields/link'
const shared = [{ name: 'note', type: 'textarea' }] as const
export const Pages = {
  slug: 'pages',
  versions: { drafts: { autosave: true } },
  fields: [
    {
      type: 'tabs',
      tabs: [
        { label: 'Content', fields: [{ name: 'layout', type: 'blocks', blocks: [Hero] }] },
        { name: 'meta', fields: [{ name: 'title', type: 'text' }] },
      ],
    },
    { type: 'row', fields: [link({ name: 'related', many: true }), ...shared] },
  ],
} satisfies CollectionConfig
"#,
    );
    write(d.path(), "src/broken.ts", "export const = ;\n");
    let s = read_payload_config(d.path()).unwrap();
    let pages = &s.collections["pages"];
    assert!(pages.drafts);
    let names: Vec<&str> = pages.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["layout", "meta", "related", "note"]);
    assert_eq!(field(&pages.fields, "meta").kind, "group");
    let rel = field(&pages.fields, "related");
    assert_eq!(rel.relation_to, ["pages", "posts"]);
    assert!(rel.many);
    assert!(field(&field(&pages.fields, "layout").fields, "hero").fields[0].required);
    assert_eq!(s.blocks["hero"][0].name, "heading");
    assert!(
        s.warnings
            .iter()
            .any(|w| w.starts_with("could not read") && w.contains("broken.ts")),
        "{:?}",
        s.warnings
    );
}

#[test]
fn check_flags_missing_field_and_kind() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "Pages.ts", PAGES_WITH_HERO);
    let dest = read_payload_config(d.path()).unwrap();
    let sum = summary(
        "post",
        &[
            ("acf.hero", stats(&["RichText"], &[])),
            ("acf.gone", stats(&["Text"], &[])),
            ("acf.extra", stats(&["Text"], &[])),
        ],
    );
    let map = mapping(
        "post",
        "pages",
        &[("acf.hero", "hero"), ("acf.gone", "missing")],
    );
    let issues = schema_check::check(&sum, &map, &dest);
    let errs = errors(&issues);
    assert!(
        errs.iter()
            .any(|e| e.contains("pages.hero") && e.contains("text") && e.contains("RichText")),
        "{errs:?}"
    );
    assert!(errs.iter().any(|e| e.contains("pages.missing")), "{errs:?}");
    assert!(
        issues.iter().any(|i| i.kind == IssueKind::UnmappedField
            && i.severity == Severity::Warning
            && i.message.contains("acf.extra")),
        "{issues:?}"
    );
    // Missing collection and undefined block slug.
    let mut map2 = mapping("post", "articles", &[]);
    map2.blocks.push(BlockMap {
        r#match: BlockMatch {
            kind: Some("image".into()),
            module: None,
        },
        to: "gallery".into(),
        fields: Default::default(),
    });
    let errs = errors(&schema_check::check(&TypeSummary::default(), &map2, &dest));
    assert!(errs.iter().any(|e| e.contains("articles")), "{errs:?}");
    assert!(errs.iter().any(|e| e.contains("gallery")), "{errs:?}");
}

#[test]
fn check_flags_missing_key_field() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "Pages.ts",
        "export const Pages = { slug: 'pages', admin: {}, fields: [{ name: 'title', type: 'text', required: true }, { name: 'summary', type: 'text', required: true }] }\n",
    );
    let dest = read_payload_config(d.path()).unwrap();
    let errs = errors(&schema_check::check(
        &TypeSummary::default(),
        &mapping("post", "pages", &[]),
        &dest,
    ));
    assert!(errs.iter().any(|e| e.contains("legacyId")), "{errs:?}");
    // Required destination field nothing maps to.
    assert!(errs.iter().any(|e| e.contains("summary")), "{errs:?}");
    assert!(!errs.iter().any(|e| e.contains("pages.title")), "{errs:?}");

    // Present but the wrong kind for key_type.
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "Pages.ts", PAGES_WITH_HERO);
    let dest = read_payload_config(d.path()).unwrap();
    let mut map = mapping("post", "pages", &[]);
    map.types["post"].key_type = KeyType::Number;
    let errs = errors(&schema_check::check(&TypeSummary::default(), &map, &dest));
    assert!(
        errs.iter()
            .any(|e| e.contains("legacyId") && e.contains("number")),
        "{errs:?}"
    );
    let ok = schema_check::check(
        &TypeSummary::default(),
        &mapping("post", "pages", &[]),
        &dest,
    );
    assert!(errors(&ok).is_empty(), "{ok:?}");
}

fn rich_mapping() -> (TypeSummary, MappingConfig) {
    let mut sum = summary(
        "post",
        &[
            ("acf.hero", stats(&["RichText"], &["<p>Hello</p>"])),
            ("acf.count", stats(&["Number"], &["3"])),
            ("acf.flag", stats(&["Bool"], &["true"])),
            ("acf.when", stats(&["Date"], &["2024-01-02T00:00:00Z"])),
            ("acf.image", stats(&["Media"], &[])),
            ("body", stats(&["Layout"], &[])),
            ("acf.subtitle", stats(&["Text"], &["Sub"])),
        ],
    );
    sum.types["post"].fields.insert(
        "acf.related".into(),
        FieldStats {
            variants: ["List".to_string()].into(),
            item_variants: ["Doc".to_string()].into(),
            relation_types: ["post".to_string()].into(),
            ..Default::default()
        },
    );
    let mut map = mapping(
        "post",
        "news-items",
        &[
            ("acf.hero", "hero"),
            ("acf.count", "count"),
            ("acf.flag", "flag"),
            ("acf.when", "when"),
            ("acf.image", "image"),
            ("body", "layout"),
            ("acf.subtitle", "seo.subtitle"),
            ("acf.related", "related"),
        ],
    );
    map.types["post"].key_type = KeyType::Number;
    map.blocks.push(BlockMap {
        r#match: BlockMatch {
            kind: Some("image".into()),
            module: None,
        },
        to: "imageBlock".into(),
        fields: [("caption".to_string(), "caption".to_string())]
            .into_iter()
            .collect(),
    });
    map.raw_block = Some("raw".into());
    (sum, map)
}

#[test]
fn generate_roundtrips() {
    let (sum, map) = rich_mapping();
    let d = tempfile::tempdir().unwrap();
    let files = schema_generate::generate(&sum, &map, d.path()).unwrap();
    assert!(
        files.iter().any(|f| f.ends_with("NewsItems.ts")),
        "{files:?}"
    );
    assert!(files.iter().any(|f| f.ends_with("blocks/ImageBlock.ts")));
    assert!(files.iter().any(|f| f.ends_with("index.ts")));
    let back: DestSchema = read_payload_config(d.path()).unwrap();
    assert!(back.warnings.is_empty(), "{:?}", back.warnings);
    let c = &back.collections["news-items"];
    assert!(c.drafts);
    let got: Vec<(&str, &str)> = c
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.kind.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("legacyId", "number"),
            ("title", "text"),
            ("slug", "text"),
            ("hero", "richText"),
            ("count", "number"),
            ("flag", "checkbox"),
            ("when", "date"),
            ("image", "upload"),
            ("layout", "blocks"),
            ("seo", "group"),
            ("related", "relationship"),
        ]
    );
    assert_eq!(field(&c.fields, "image").relation_to, ["media"]);
    let rel = field(&c.fields, "related");
    assert!(rel.many);
    assert_eq!(rel.relation_to, ["news-items"]);
    assert_eq!(field(&c.fields, "seo").fields[0].name, "subtitle");
    let blocks: Vec<&str> = field(&c.fields, "layout")
        .fields
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(blocks, ["imageBlock", "raw"]);
    assert_eq!(back.blocks["raw"][0].kind, "textarea");
    // A generated schema passes its own check.
    let errs = errors(&schema_check::check(&sum, &map, &back));
    assert!(errs.is_empty(), "{errs:?}");
}

#[test]
fn prompt_contains_check_command_and_samples() {
    let (mut sum, map) = rich_mapping();
    sum.types["post"].fields["acf.subtitle"].samples = vec![
        "<b>Bold</b> words".into(),
        "write to someone@example.test or call +44 20 7946 0000".into(),
        "x".repeat(200),
    ];
    let p = schema_prompt::prompt(&sum, &map, Path::new("site/src"), Path::new("proj"));
    assert!(p.contains("porter schema check --project proj"), "{p}");
    assert!(p.contains("until it exits 0"));
    assert!(p.contains("news-items"));
    assert!(p.contains("Bold words"), "{p}");
    assert!(!p.contains("<b>"));
    assert!(!p.contains("someone@example.test"));
    assert!(!p.contains("7946"));
    assert!(p.contains("[redacted]"));
    assert!(!p.contains(&"x".repeat(81)));
    assert!(p.contains("legacyId"));
    assert!(p.contains("imageBlock"));
    let (md, html) = schema_prompt::spec(&sum, &map, Path::new("site/src"), Path::new("proj"));
    assert!(md.contains("news-items") && md.contains("Bold words"));
    assert!(html.contains("<table") && html.contains("news-items"));
    assert!(!html.contains("someone@example.test"));
}

#[test]
fn summary_from_store_counts_variants_fill_and_samples() {
    let d = tempfile::tempdir().unwrap();
    let p = Project::init(d.path(), "t", "wordpress").unwrap();
    for (n, hero) in [
        ("1", Some(FieldValue::Text("<p>One</p>".into()))),
        ("2", None),
    ] {
        let src = SourceRef {
            connector: "wxr".into(),
            source_id: n.into(),
            url: None,
        };
        let id = ItemId::new(ItemKind::Document, &src);
        let mut fields = indexmap::IndexMap::new();
        if let Some(h) = hero {
            fields.insert("acf.hero".to_string(), h);
        }
        fields.insert(
            "body".to_string(),
            FieldValue::RichText(RichText { blocks: vec![] }),
        );
        let doc = Document {
            id,
            doc_type: "post".into(),
            status: DocStatus::Published,
            locale: None,
            translations: vec![],
            slug: n.into(),
            path: format!("/{n}/"),
            parent: None,
            title: n.into(),
            fields,
            seo: Seo::default(),
            created: None,
            updated: None,
            author: None,
            original: serde_json::Value::Null,
            source: src.clone(),
        };
        p.store()
            .put_item(ItemKind::Document, id, &src, &doc)
            .unwrap();
    }
    let s = TypeSummary::from_store(p.store()).unwrap();
    let t = &s.types["post"];
    assert_eq!(t.count, 2);
    let hero = &t.fields["acf.hero"];
    assert!(hero.variants.contains("Text"));
    assert_eq!(hero.filled, 1);
    assert!((hero.fill_rate - 0.5).abs() < 1e-9);
    assert_eq!(hero.samples, ["<p>One</p>"]);
    assert!(t.fields["body"].variants.contains("RichText"));
}

fn warnings(issues: &[porter_engine::html::PendingIssue]) -> Vec<String> {
    issues
        .iter()
        .filter(|i| i.severity == Severity::Warning)
        .map(|i| i.message.clone())
        .collect()
}

#[test]
fn reader_warns_on_partial_configs() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "src/Pages.ts",
        r#"
import type { CollectionConfig } from 'payload'
import { SOME_TYPE, pkgFields, base, isProd } from 'some-pkg'
const helper = () => {
  const out = []
  if (isProd) { out.push({ name: 'secret', type: 'text', required: true }) }
  return out
}
export const Pages: CollectionConfig = {
  slug: 'pages',
  admin: {},
  fields: [
    { name: 'a', type: isProd ? 'text' : 'textarea' },
    { name: `x${isProd}`, type: 'text' },
    { name: 'b', type: SOME_TYPE },
    { name: 'c', type: 'text', required: isProd, hasMany: isProd },
    ...helper(),
    { name: 'g', type: 'group', fields: pkgFields },
  ],
}
export const Things: CollectionConfig = { ...base, slug: 'things' }
"#,
    );
    let s = read_payload_config(d.path()).unwrap();
    let w = s.warnings.join("\n");
    let has = |needles: &[&str]| {
        assert!(
            s.warnings
                .iter()
                .any(|w| w.contains("Pages.ts") && needles.iter().all(|n| w.contains(n))),
            "{needles:?} not in:\n{w}"
        )
    };
    // (a) type / name unresolved
    has(&["pages.a", "expression"]);
    has(&["pages[1]"]);
    has(&["SOME_TYPE", "pages.b"]);
    // (b) required / hasMany behind an expression
    has(&["pages.c", "hasMany"]);
    let c = field(&s.collections["pages"].fields, "c");
    assert!(c.required_unknown && !c.required);
    // (c) unresolved spread into the collection
    has(&["things", "partly"]);
    // (d) helper with an `if`
    has(&["helper()", "if"]);
    // (e) group with unresolved fields
    has(&["pages.g", "pkgFields"]);

    let issues = schema_check::check(&TypeSummary::default(), &mapping("post", "pages", &[]), &s);
    assert!(
        warnings(&issues)
            .iter()
            .any(|m| m.contains("could not determine if") && m.contains("pages.c")),
        "{issues:?}"
    );
}

#[test]
fn mapping_typo_is_reported() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "Pages.ts", PAGES_WITH_HERO);
    let dest = read_payload_config(d.path()).unwrap();
    let mut sum = summary("post", &[("acf.hero", stats(&["Text"], &[]))]);
    sum.types.insert(
        "empty".into(),
        TypeStats {
            count: 0,
            ..Default::default()
        },
    );
    let mut map = mapping("post", "pages", &[("acf.heor", "hero")]);
    map.types.insert(
        "event".into(),
        TypeMap {
            to: Some("pages".into()),
            ..TypeMap::default()
        },
    );
    let w = warnings(&schema_check::check(&sum, &map, &dest));
    assert!(
        w.iter()
            .any(|m| m.contains("post.acf.heor") && m.contains("none of 3 post documents")),
        "{w:?}"
    );
    assert!(
        w.iter()
            .any(|m| m.contains("type `event` is mapped but has no normalised documents")),
        "{w:?}"
    );
}

#[test]
fn nested_required_fields_checked() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "Pages.ts",
        r#"
export const Pages = { slug: 'pages', admin: {}, fields: [
  { name: 'legacyId', type: 'text' },
  { name: 'hero', type: 'group', fields: [
    { name: 'heading', type: 'text', required: true },
    { name: 'image', type: 'upload', relationTo: 'media' },
  ] },
  { name: 'meta', type: 'group', fields: [{ name: 'keywords', type: 'text', required: true }] },
  { name: 'layout', type: 'blocks', blocks: [
    { slug: 'cta', fields: [{ name: 'label', type: 'text', required: true }, { name: 'url', type: 'text', required: true }] },
  ] },
] }
"#,
    );
    let dest = read_payload_config(d.path()).unwrap();
    let mut map = mapping(
        "post",
        "pages",
        &[("acf.img", "hero.image"), ("acf.meta", "meta")],
    );
    map.blocks.push(BlockMap {
        r#match: BlockMatch {
            kind: Some("button".into()),
            module: None,
        },
        to: "cta".into(),
        fields: [("text".to_string(), "label".to_string())]
            .into_iter()
            .collect(),
    });
    let issues = schema_check::check(&TypeSummary::default(), &map, &dest);
    let errs = errors(&issues);
    assert!(
        errs.iter()
            .any(|e| e.contains("pages.hero.heading") && e.contains("required")),
        "{errs:?}"
    );
    assert!(
        errs.iter().any(|e| e.contains("cta") && e.contains("url")),
        "{errs:?}"
    );
    assert!(!errs.iter().any(|e| e.contains("label")), "{errs:?}");
    assert!(
        warnings(&issues)
            .iter()
            .any(|w| w.contains("meta.keywords") && w.contains("can't be verified")),
        "{issues:?}"
    );
}

#[test]
fn dropped_field_is_not_unmapped_and_pii_is_masked() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "Pages.ts", PAGES_WITH_HERO);
    let dest = read_payload_config(d.path()).unwrap();
    let sum = summary(
        "post",
        &[
            ("acf.old", stats(&["Text"], &[])),
            ("acf.new", stats(&["Text"], &[])),
        ],
    );
    let mut map = mapping("post", "pages", &[]);
    map.types["post"].fields.insert(
        "acf.old".into(),
        FieldMap {
            drop: true,
            reason: Some("retired".into()),
            ..FieldMap::default()
        },
    );
    let issues = schema_check::check(&sum, &map, &dest);
    let unmapped: Vec<_> = issues
        .iter()
        .filter(|i| i.kind == IssueKind::UnmappedField)
        .map(|i| i.message.clone())
        .collect();
    assert!(
        unmapped.iter().any(|m| m.contains("acf.new")),
        "{unmapped:?}"
    );
    assert!(
        !unmapped.iter().any(|m| m.contains("acf.old")),
        "{unmapped:?}"
    );
    assert!(errors(&issues).is_empty(), "{issues:?}");

    for raw in [
        "call 020/7946/0958 now",
        "mail jane&#64;example.com",
        "mail jane&#x40;example.com",
    ] {
        let c = schema_prompt::clean_sample(raw);
        assert!(c.contains("[redacted]"), "{raw} -> {c}");
        assert!(
            !c.contains("7946") && !c.contains("example.com"),
            "{raw} -> {c}"
        );
    }
    assert_eq!(
        schema_prompt::clean_sample("on 2024-01-02"),
        "on 2024-01-02"
    );
}
