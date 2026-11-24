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
            ..Default::default()
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

/// A required field with a default is filled by Payload: not a gap. An empty default is.
#[test]
fn required_with_default_is_not_a_gap() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "Pages.ts",
        r#"
import type { CollectionConfig } from 'payload'
export const Pages: CollectionConfig = {
  slug: 'pages',
  fields: [
    { name: 'title', type: 'text', required: true },
    { name: 'legacyId', type: 'text', index: true, unique: true },
    { name: 'ratio', type: 'select', required: true, defaultValue: 'default', options: ['default'] },
    { name: 'label', type: 'text', required: true, defaultValue: '' },
  ],
}
"#,
    );
    let dest = read_payload_config(d.path()).unwrap();
    let pages = &dest.collections["pages"];
    assert!(!field(&pages.fields, "ratio").required);
    assert!(field(&pages.fields, "label").required);
    let errs = errors(&schema_check::check(
        &summary("post", &[]),
        &mapping("post", "pages", &[]),
        &dest,
    ));
    assert!(!errs.iter().any(|e| e.contains("ratio")), "{errs:?}");
    assert!(errs.iter().any(|e| e.contains("label")), "{errs:?}");
}

/// A collection factory whose tabs depend on an option: `...(seo ? [seoTab] : [])`.
#[test]
fn conditional_spread_follows_known_test() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "src/factory.ts",
        r#"
const seoTab = { name: 'meta', fields: [{ name: 'title', type: 'text' }] }
export const make = ({ slug, seo = true, extra = '' }) => ({
  slug,
  fields: [
    { type: 'tabs', tabs: [{ label: 'Details', fields: [{ name: 'body', type: 'text' }] }, ...(seo ? [seoTab] : [])] },
    ...(extra ? [{ name: extra, type: 'text' }] : []),
  ],
})
"#,
    );
    write(
        d.path(),
        "src/collections.ts",
        r#"
import type { CollectionConfig } from 'payload'
import { make } from './factory'
export const Rooms: CollectionConfig = make({ slug: 'rooms' })
export const Slides: CollectionConfig = make({ slug: 'slides', seo: false, extra: 'caption' })
"#,
    );
    let s = read_payload_config(d.path()).unwrap();
    let names = |c: &str| -> Vec<String> {
        s.collections[c]
            .fields
            .iter()
            .map(|f| f.name.clone())
            .collect()
    };
    assert_eq!(names("rooms"), ["body", "meta"]);
    assert_eq!(names("slides"), ["body", "caption"]);
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
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
    let files = schema_generate::generate(&sum, &map, d.path())
        .unwrap()
        .files;
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
            ("path", "text"),
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

/// `seo.*`, `created` and `updated` live on the document, not in `fields`: mapping them is
/// not "mapped but appears in none", they're typed, and never reported unmapped. A radio
/// takes text like a select does.
#[test]
fn doc_level_paths_and_radio_checked() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "Pages.ts",
        r#"
import type { CollectionConfig } from 'payload'
export const Pages: CollectionConfig = {
  slug: 'pages',
  fields: [
    { name: 'title', type: 'text', required: true },
    { name: 'legacyId', type: 'text', index: true, unique: true },
    { name: 'publishedAt', type: 'date' },
    { name: 'metaTitle', type: 'text' },
    { name: 'kind', type: 'radio', options: ['a', 'b'] },
  ],
}
"#,
    );
    let dest = read_payload_config(d.path()).unwrap();
    let p = Project::init(d.path().join("p").as_path(), "t", "wordpress").unwrap();
    let src = SourceRef {
        connector: "wxr".into(),
        source_id: "1".into(),
        url: None,
    };
    let id = ItemId::new(ItemKind::Document, &src);
    let mut fields = indexmap::IndexMap::new();
    fields.insert("acf.kind".to_string(), FieldValue::Text("a".into()));
    let doc = Document {
        id,
        doc_type: "post".into(),
        status: DocStatus::Published,
        locale: None,
        translations: vec![],
        slug: "one".into(),
        path: "/one/".into(),
        parent: None,
        title: "One".into(),
        fields,
        seo: Seo {
            title: Some("T".into()),
            ..Seo::default()
        },
        created: Some(chrono::Utc::now()),
        updated: None,
        author: None,
        original: serde_json::Value::Null,
        source: src.clone(),
    };
    p.store()
        .put_item(ItemKind::Document, id, &src, &doc)
        .unwrap();
    let sum = TypeSummary::from_store(p.store()).unwrap();
    assert!(!sum.types["post"].fields.contains_key("seo.title"));
    assert_eq!(sum.types["post"].field("seo.title").unwrap().filled, 1);
    let map = mapping(
        "post",
        "pages",
        &[
            ("seo.title", "metaTitle"),
            ("created", "publishedAt"),
            ("acf.kind", "kind"),
        ],
    );
    let issues = schema_check::check(&sum, &map, &dest);
    assert!(errors(&issues).is_empty(), "{issues:?}");
    assert!(
        !issues.iter().any(|i| i.message.contains("appears in none")
            || i.message.contains("seo.")
            || i.message.contains("`post.created`")
            || i.message.contains("`post.updated`")),
        "{issues:?}"
    );
    // A doc-level value of the wrong kind is still caught.
    let bad = mapping("post", "pages", &[("created", "metaTitle")]);
    let errs = errors(&schema_check::check(&sum, &bad, &dest));
    assert!(errs.iter().any(|e| e.contains("holds Date")), "{errs:?}");
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
    has(&["pages.a", "condition"]);
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

#[test]
fn round2_tabs_groups_relation_to_never_pass_silently() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "Pages.ts",
        r#"
import { seoTab, OTHER } from 'some-pkg'
export const Pages = { slug: 'pages', admin: {}, fields: [
  { name: 'legacyId', type: 'text' },
  { type: 'tabs', tabs: [seoTab, { label: 'Empty' }] },
  { name: 'meta', type: 'group', fields: [{ name: 'keywords', type: 'text', required: true }] },
  { name: 'link', type: 'relationship', relationTo: ['media', OTHER] },
  { name: 'one', type: 'relationship', relationTo: ['posts'] },
] }
"#,
    );
    let s = read_payload_config(d.path()).unwrap();
    let w = s.warnings.join("\n");
    assert!(w.contains("could not resolve tab `seoTab`"), "{w}");
    assert!(w.contains("tab 1 of pages[1] has no fields"), "{w}");
    assert!(
        w.contains("could not resolve every `relationTo` entry of pages.link (got media)"),
        "{w}"
    );
    let f = &s.collections["pages"].fields;
    let link = field(f, "link");
    assert_eq!(link.relation_to, ["media"]);
    assert!(link.polymorphic);
    assert!(field(f, "one").polymorphic);

    let errs = errors(&schema_check::check(
        &TypeSummary::default(),
        &mapping("post", "pages", &[]),
        &s,
    ));
    assert!(
        errs.iter()
            .any(|e| e.contains("pages.meta.keywords") && e.contains("required")),
        "{errs:?}"
    );
}

// ---- nested data, the path field, `--into` ----

fn sref(sid: &str) -> SourceRef {
    SourceRef {
        connector: "wxr".into(),
        source_id: sid.into(),
        url: None,
    }
}

/// A project holding one `post` per entry of `docs` (source id, fields).
fn store_with(docs: Vec<(&str, Vec<(&str, FieldValue)>)>) -> (tempfile::TempDir, Project) {
    let d = tempfile::tempdir().unwrap();
    let p = Project::init(d.path(), "t", "wordpress").unwrap();
    for (sid, fields) in docs {
        let src = sref(sid);
        let id = ItemId::new(ItemKind::Document, &src);
        let doc = Document {
            id,
            doc_type: "post".into(),
            status: DocStatus::Published,
            locale: None,
            translations: vec![],
            slug: sid.into(),
            path: format!("/{sid}/"),
            parent: None,
            title: sid.into(),
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
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
    (d, p)
}

fn obj(kv: Vec<(&str, FieldValue)>) -> FieldValue {
    FieldValue::Object(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn text(s: &str) -> FieldValue {
    FieldValue::Text(s.into())
}

/// Generates `post -> posts` mapping `field` to `field`; returns Posts.ts and the infos.
fn gen_posts(p: &Project, field: &str) -> (String, Vec<String>) {
    let sum = TypeSummary::from_store(p.store()).unwrap();
    let map = mapping("post", "posts", &[(field, field)]);
    let out = tempfile::tempdir().unwrap();
    let g = schema_generate::generate(&sum, &map, out.path()).unwrap();
    let ts = std::fs::read_to_string(out.path().join("Posts.ts")).unwrap();
    (ts, g.infos)
}

#[test]
fn object_becomes_group() {
    let (_d, p) = store_with(vec![(
        "1",
        vec![(
            "seo",
            obj(vec![("metaTitle", text("T")), ("_type", text("seoGroup"))]),
        )],
    )]);
    let (ts, infos) = gen_posts(&p, "seo");
    assert!(
        ts.contains(
            "    {\n      name: \"seo\",\n      type: \"group\",\n      fields: [\n        {\n          name: \"metaTitle\",\n          type: \"text\",\n        },\n      ],\n    },\n"
        ),
        "{ts}"
    );
    assert!(!ts.contains("_type"), "{ts}");
    assert!(infos.is_empty(), "{infos:?}");
}

#[test]
fn list_of_objects_becomes_array() {
    let row = |l: &str| obj(vec![("label", text(l)), ("score", FieldValue::Number(1.0))]);
    let (_d, p) = store_with(vec![(
        "1",
        vec![("rows", FieldValue::List(vec![row("a"), row("b")]))],
    )]);
    let (ts, infos) = gen_posts(&p, "rows");
    assert!(
        ts.contains(
            "    {\n      name: \"rows\",\n      type: \"array\",\n      fields: [\n        {\n          name: \"label\",\n          type: \"text\",\n        },\n        {\n          name: \"score\",\n          type: \"number\",\n        },\n      ],\n    },\n"
        ),
        "{ts}"
    );
    assert!(infos.is_empty(), "{infos:?}");
}

#[test]
fn doc_inside_object_is_relationship() {
    let target = ItemId::new(ItemKind::Document, &sref("2"));
    let (_d, p) = store_with(vec![
        (
            "1",
            vec![(
                "hero",
                obj(vec![
                    ("link", FieldValue::Doc(target)),
                    ("label", text("Go")),
                ]),
            )],
        ),
        ("2", vec![]),
    ]);
    let sum = TypeSummary::from_store(p.store()).unwrap();
    let link = &sum.types["post"].fields["hero"].children["link"];
    assert!(link.relation_types.contains("post"), "{link:?}");
    let (ts, _) = gen_posts(&p, "hero");
    assert!(
        ts.contains(
            "        {\n          name: \"link\",\n          type: \"relationship\",\n          relationTo: \"posts\",\n        },\n"
        ),
        "{ts}"
    );
}

#[test]
fn mixed_shapes_fall_back_to_json_with_info() {
    let (_d, p) = store_with(vec![
        ("1", vec![("extra", obj(vec![("a", text("x"))]))]),
        ("2", vec![("extra", text("plain"))]),
    ]);
    let (ts, infos) = gen_posts(&p, "extra");
    assert!(
        ts.contains("    {\n      name: \"extra\",\n      type: \"json\",\n    },\n"),
        "{ts}"
    );
    assert_eq!(infos, ["posts.extra has mixed shapes; generated as json"]);
}

#[test]
fn every_collection_has_path_field() {
    let (sum, map) = rich_mapping();
    let d = tempfile::tempdir().unwrap();
    schema_generate::generate(&sum, &map, d.path()).unwrap();
    let ts = std::fs::read_to_string(d.path().join("NewsItems.ts")).unwrap();
    assert!(
        ts.contains(
            "    { name: 'path', type: 'text', index: true, unique: true, admin: { readOnly: true } },\n"
        ),
        "{ts}"
    );
    let back = read_payload_config(d.path()).unwrap();
    let path = field(&back.collections["news-items"].fields, "path");
    assert_eq!(path.kind, "text");
}

/// Runs `generate_into` for `post -> <to>` over a one-doc project.
async fn into(
    repo: &Path,
    to: &str,
) -> (
    porter_engine::events::Summary,
    Option<schema_generate::IntoReport>,
    Vec<porter_model::Issue>,
) {
    use porter_engine::events::EventSink;
    use porter_engine::stage::run_stage;
    use std::sync::{Arc, Mutex};
    let (_d, p) = store_with(vec![("1", vec![("seo", obj(vec![("t", text("x"))]))])]);
    let p = Arc::new(p);
    let mut cfg = p.config().clone();
    cfg.mapping = mapping("post", to, &[("seo", "seo")]);
    let report = Arc::new(Mutex::new(None));
    let (r, repo) = (report.clone(), repo.to_path_buf());
    let summary = run_stage(
        p.clone(),
        Arc::new(cfg),
        EventSink::new(64),
        Default::default(),
        porter_model::Stage::Schema,
        |ctx| async move {
            *r.lock().unwrap() = Some(schema_generate::generate_into(&ctx, &repo)?);
            Ok(())
        },
    )
    .await
    .unwrap();
    let issues = p.store().issues(&Default::default()).unwrap();
    let out = report.lock().unwrap().take();
    (summary, out, issues)
}

/// `post` maps onto the app's `pages` on purpose (R20): `pages` is left to the app, with a
/// warning; nothing collides. (The accidental case, `pages` -> `pages`, is the CLI test.)
#[tokio::test]
async fn into_skips_a_collection_mapped_onto_the_app() {
    let repo = tempfile::tempdir().unwrap();
    write(
        repo.path(),
        "src/collections/Pages.ts",
        "export const Pages = {\n  slug: 'pages',\n  fields: [],\n}\n",
    );
    let (summary, report, issues) = into(repo.path(), "pages").await;
    let report = report.unwrap();
    assert!(report.collisions.is_empty());
    assert_eq!(report.skipped, ["pages"]);
    assert!(!repo.path().join("src/collections/porter/Pages.ts").exists());
    assert_eq!(summary.errors, 0);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Warning
            && i.message
                .contains("`post` maps onto existing collection `pages`")),
        "{issues:?}"
    );
}

#[tokio::test]
async fn into_writes_index() {
    let repo = tempfile::tempdir().unwrap();
    // A slug under porter/ is ours, from an earlier run: not a collision.
    write(
        repo.path(),
        "src/collections/porter/Articles.ts",
        "export const Articles = { slug: 'articles' }\n",
    );
    let (summary, report, _) = into(repo.path(), "articles").await;
    let report = report.unwrap();
    assert!(report.collisions.is_empty());
    assert_eq!(porter_engine::events::exit_code(&summary), 0);
    let base = repo.path().join("src/collections/porter");
    assert!(report.written.contains(&base.join("Articles.ts")));
    assert!(report.written.contains(&base.join("index.ts")));
    let index = std::fs::read_to_string(base.join("index.ts")).unwrap();
    assert!(
        index.contains("import { Articles } from './Articles'"),
        "{index}"
    );
    assert!(
        index.contains("export const porterCollections: CollectionConfig[] = [Articles]"),
        "{index}"
    );
    assert!(
        index.contains("export const porterBlocks: Block[] = []"),
        "{index}"
    );
    let back = read_payload_config(&base).unwrap();
    assert!(back.collections.contains_key("articles"));
}

#[test]
fn test_app_reads_generated_porter_collections() {
    let s = read_payload_config(&repo().join("tests/payload-app/src")).unwrap();
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    let hb = &s.collections["hotelBreak"];
    assert_eq!(field(&hb.fields, "path").kind, "text");
    assert_eq!(field(&hb.fields, "seo").kind, "group");
    let hl = field(&hb.fields, "highlights");
    assert_eq!(hl.kind, "array");
    assert_eq!(field(&hl.fields, "city").relation_to, ["cityPage"]);
}
