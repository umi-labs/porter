//! `porter schema prompt` (instructions for an AI agent) and `porter schema spec` (the same
//! requirements for a human, as Markdown and HTML). Sample values are client data on their
//! way into a prompt: HTML stripped, anything like an email or phone number masked,
//! truncated to 80 characters.

use super::schema_generate::{block_matches, block_slugs, describe};
use crate::mapping::summary::{FieldStats, TypeSummary};
use crate::mapping::{KeyType, MappingConfig};
use maud::{DOCTYPE, html};
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

const SAMPLE_CHARS: usize = 80;

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("valid regex"));
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+").expect("valid regex")
});
// Seven or more digits with phone punctuation between: also catches long ids, which is fine.
static NUM_ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&#([xX][0-9a-fA-F]+|[0-9]+);").expect("valid regex"));
static PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\+?\(?\d[\d\s()./-]{5,}\d").expect("valid regex"));

static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("valid regex"));

/// A sample safe to show: no tags, emails or phone numbers, at most 80 characters.
pub fn clean_sample(s: &str) -> String {
    let s = TAG.replace_all(s, " ");
    let s = s
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    // `&#64;` / `&#x40;` would otherwise hide an `@` from the email pattern.
    let s = NUM_ENTITY.replace_all(&s, |c: &regex::Captures| {
        let n = &c[1];
        let code = match n.strip_prefix(['x', 'X']) {
            Some(h) => u32::from_str_radix(h, 16).ok(),
            None => n.parse().ok(),
        };
        code.and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default()
    });
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = EMAIL.replace_all(&s, "[redacted]");
    let s = PHONE.replace_all(&s, |c: &regex::Captures| {
        let m = &c[0];
        // An ISO date isn't a phone number.
        if DATE.is_match(m) {
            m.to_string()
        } else {
            "[redacted]".to_string()
        }
    });
    if s.chars().count() > SAMPLE_CHARS {
        s.chars().take(SAMPLE_CHARS).collect::<String>() + "…"
    } else {
        s.into_owned()
    }
}

struct Row {
    field: String,
    kind: String,
    required: bool,
    from: String,
    filled: Option<f64>,
    samples: Vec<String>,
}

struct Coll {
    slug: String,
    from: Vec<(String, usize)>,
    rows: Vec<Row>,
}

struct Plan {
    dest_dir: String,
    check: String,
    colls: Vec<Coll>,
    blocks: Vec<(String, Vec<Row>)>,
}

fn row(field: &str, kind: String, from: String, fs: Option<&FieldStats>) -> Row {
    Row {
        field: field.into(),
        kind,
        required: false,
        from,
        filled: fs.map(|f| f.fill_rate),
        samples: fs
            .map(|f| {
                f.samples
                    .iter()
                    .map(|s| clean_sample(s))
                    .filter(|s| !s.is_empty())
                    .take(3)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn plan(
    summary: &TypeSummary,
    mapping: &MappingConfig,
    dest_dir: &Path,
    project_dir: &Path,
) -> Plan {
    let mut colls: Vec<Coll> = vec![];
    for (ty, tm) in &mapping.types {
        let Some(slug) = &tm.to else { continue };
        let stats = summary.types.get(ty);
        let i = match colls.iter().position(|c| &c.slug == slug) {
            Some(i) => i,
            None => {
                let key_kind = match tm.key_type {
                    KeyType::Text => "text",
                    KeyType::Number => "number",
                };
                let fixed = |f: &str, kind: &str, from: &str, required| Row {
                    required,
                    ..row(f, kind.into(), from.into(), None)
                };
                colls.push(Coll {
                    slug: slug.clone(),
                    from: vec![],
                    rows: vec![
                        fixed(
                            &tm.key_field,
                            &format!("{key_kind}, index: true, unique: true, admin: {{ hidden: true, readOnly: true }}"),
                            "source id (upsert key)",
                            true,
                        ),
                        fixed("title", "text", "document title", true),
                        fixed("slug", "text, index: true", "document slug", false),
                    ],
                });
                colls.len() - 1
            }
        };
        let c = &mut colls[i];
        c.from
            .push((ty.clone(), stats.map(|s| s.count).unwrap_or(0)));
        for (src, fm) in tm.fields.iter().filter(|(_, fm)| !fm.drop) {
            if c.rows.iter().any(|r| r.field == fm.to) {
                continue;
            }
            let fs = stats.and_then(|s| s.fields.get(src));
            let kind = describe(
                &fs.cloned().unwrap_or_default(),
                &fm.transform,
                mapping,
                slug,
            );
            c.rows.push(row(&fm.to, kind, format!("{ty}.{src}"), fs));
        }
    }
    let mut blocks = vec![];
    for slug in block_slugs(mapping) {
        let mut rows = vec![];
        if mapping.raw_block.as_deref() == Some(slug.as_str()) {
            rows.push(row(
                "html",
                "textarea".into(),
                "HTML with no native block".into(),
                None,
            ));
        }
        for m in mapping.blocks.iter().filter(|m| m.to == slug) {
            for (src, dest) in &m.fields {
                let mut fs = FieldStats::default();
                for b in summary.blocks.iter().filter(|b| block_matches(m, b)) {
                    if let Some(p) = b.props.get(src) {
                        fs.variants.extend(p.variants.iter().cloned());
                        fs.item_variants.extend(p.item_variants.iter().cloned());
                        fs.samples.extend(p.samples.iter().cloned());
                    }
                }
                let own = colls.first().map(|c| c.slug.as_str()).unwrap_or("pages");
                let kind = describe(&fs, &[], mapping, own);
                let from = m
                    .r#match
                    .module
                    .clone()
                    .or(m.r#match.kind.clone())
                    .unwrap_or_default();
                let mut r = row(dest, kind, format!("{from} block: {src}"), Some(&fs));
                r.filled = None;
                rows.push(r);
            }
        }
        blocks.push((slug, rows));
    }
    Plan {
        dest_dir: dest_dir.display().to_string(),
        check: format!("porter schema check --project {}", project_dir.display()),
        colls,
        blocks,
    }
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn md_body(p: &Plan) -> String {
    let mut s = String::new();
    s.push_str("## Collections\n\n");
    for c in &p.colls {
        let from: Vec<String> = c
            .from
            .iter()
            .map(|(t, n)| format!("`{t}` ({n} documents)"))
            .collect();
        s.push_str(&format!(
            "### `{}`\n\nFrom source types {}.\n\n",
            c.slug,
            from.join(", ")
        ));
        s.push_str("| Field | Payload type | Required | From | Filled | Samples |\n|---|---|---|---|---|---|\n");
        for r in &c.rows {
            s.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} |\n",
                cell(&r.field),
                cell(&r.kind),
                if r.required { "yes" } else { "" },
                cell(&r.from),
                r.filled
                    .map(|f| format!("{:.0}%", f * 100.0))
                    .unwrap_or_default(),
                cell(&r.samples.join(" · ")),
            ));
        }
        s.push('\n');
    }
    s.push_str("## Blocks\n\n");
    if p.blocks.is_empty() {
        s.push_str("No layout blocks are mapped.\n\n");
    } else {
        s.push_str("Every `blocks` field above lists these blocks. Define each as a Payload `Block` with this slug and fields.\n\n");
    }
    for (slug, rows) in &p.blocks {
        s.push_str(&format!("### `{slug}`\n\n"));
        if rows.is_empty() {
            s.push_str("No fields mapped.\n\n");
            continue;
        }
        s.push_str("| Field | Payload type | From | Samples |\n|---|---|---|---|\n");
        for r in rows {
            s.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                cell(&r.field),
                cell(&r.kind),
                cell(&r.from),
                cell(&r.samples.join(" · "))
            ));
        }
        s.push('\n');
    }
    s.push_str(
        "## Upsert key\n\nEvery collection above needs its key field (first row of its table) exactly as shown: \
         `index: true, unique: true`, hidden and read-only in the admin. porter finds documents it already \
         loaded by this field, so re-runs update instead of duplicating.\n\n",
    );
    s
}

/// Instructions for an AI agent editing the Payload config. Written to `reports/schema-prompt.md`.
pub fn prompt(
    summary: &TypeSummary,
    mapping: &MappingConfig,
    dest_dir: &Path,
    project_dir: &Path,
) -> String {
    let p = plan(summary, mapping, dest_dir, project_dir);
    format!(
        "# Prepare the Payload schema for a porter migration\n\n\
         ## Goal\n\n\
         Edit the Payload CMS config in `{dir}` so it can receive the content porter will load. \
         Add the collections, fields and blocks below that are missing; keep everything that exists. \
         Names and types must match exactly, since porter writes to these paths. Where a type is \
         marked `json`, the source data is mixed: choose a better Payload type only if it accepts \
         every sample. Sample values are real (masked) content, for judging field types only.\n\n\
         {body}\
         ## Done when\n\n\
         Do not stop until it passes: verify by running `{check}` until it exits 0. \
         It prints every mismatch it finds; fix each and run it again.\n",
        dir = p.dest_dir,
        body = md_body(&p),
        check = p.check,
    )
}

/// The same requirements for a person: `(markdown, html)` for `reports/schema-spec.{md,html}`.
pub fn spec(
    summary: &TypeSummary,
    mapping: &MappingConfig,
    dest_dir: &Path,
    project_dir: &Path,
) -> (String, String) {
    let p = plan(summary, mapping, dest_dir, project_dir);
    let intro = format!(
        "What the Payload site in `{}` needs before porter can load the migrated content. \
         Add anything missing; existing fields can stay. When you're done, run `{}`: it exits 0 \
         once the schema matches.",
        p.dest_dir, p.check
    );
    let md = format!("# Payload schema spec\n\n{intro}\n\n{}", md_body(&p));
    let rows = |rows: &[Row], full: bool| {
        html! {
            div.scroll { table {
                thead { tr {
                    th { "Field" } th { "Payload type" }
                    @if full { th { "Required" } }
                    th { "From" }
                    @if full { th { "Filled" } }
                    th { "Samples" }
                } }
                tbody { @for r in rows { tr {
                    td { code { (r.field) } } td { (r.kind) }
                    @if full { td { @if r.required { "yes" } } }
                    td { (r.from) }
                    @if full { td { (r.filled.map(|f| format!("{:.0}%", f * 100.0)).unwrap_or_default()) } }
                    td { (r.samples.join(" · ")) }
                } } }
            } }
        }
    };
    let html = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Payload schema spec" }
                style { (maud::PreEscaped(crate::report::CSS)) }
            }
            body { main {
                header { h1 { "Payload schema spec" } p.muted { (intro) } }
                h2 { "Collections" }
                @for c in &p.colls {
                    h3 { code { (c.slug) } }
                    p.muted { "From " (c.from.iter().map(|(t, n)| format!("{t} ({n} documents)")).collect::<Vec<_>>().join(", ")) }
                    (rows(&c.rows, true))
                }
                h2 { "Blocks" }
                @for (slug, rs) in &p.blocks {
                    h3 { code { (slug) } }
                    @if rs.is_empty() { p.muted { "No fields mapped." } } @else { (rows(rs, false)) }
                }
                h2 { "Upsert key" }
                p { "Every collection needs its key field (first row) exactly as shown: indexed, unique, hidden and read-only. porter finds documents it already loaded by it, so re-runs update instead of duplicating." }
            } }
        }
    }
    .into_string();
    (md, html)
}
