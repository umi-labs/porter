//! `porter schema generate`: Payload 3 collection and block configs from the mapping and
//! what the documents hold. Written to the project's `generated/payload/`, never into the
//! client's own config.

use super::schema_check::effective;
use crate::mapping::summary::{BlockStats, FieldStats, TypeSummary};
use crate::mapping::{BlockMap, KeyType, MappingConfig};
use crate::stage::StageCtx;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use porter_model::{IssueKind, Severity};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Field {
    name: String,
    kind: String,
    many: bool,
    required: bool,
    relation_to: Vec<String>,
    blocks: Vec<String>,
    fields: Vec<Field>,
}

/// `news-items` -> `NewsItems`.
pub fn pascal(slug: &str) -> String {
    let mut s: String = slug
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut cs = p.chars();
            cs.next()
                .map(|c| c.to_ascii_uppercase().to_string() + cs.as_str())
                .unwrap_or_default()
        })
        .collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// The exported name (and file name) of a block config.
pub fn block_ident(slug: &str) -> String {
    let p = pascal(slug);
    if p.ends_with("Block") { p } else { p + "Block" }
}

fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// Every `[[mapping.blocks]]` slug then the raw block, deduplicated.
pub fn block_slugs(mapping: &MappingConfig) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for s in mapping
        .blocks
        .iter()
        .map(|b| b.to.clone())
        .chain(mapping.raw_block.clone())
    {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Whether a block map's `match` selects blocks with this kind/module (as the loader does).
pub fn block_matches(m: &BlockMap, b: &BlockStats) -> bool {
    let (k, md) = (m.r#match.kind.as_deref(), m.r#match.module.as_deref());
    (k.is_some() || md.is_some())
        && k.is_none_or(|k| k == b.kind)
        && md.is_none_or(|md| Some(md) == b.module.as_deref())
}

/// Collection slug for each source type.
fn collection_of<'a>(mapping: &'a MappingConfig, ty: &str) -> Option<&'a str> {
    mapping.types.get(ty)?.to.as_deref()
}

/// `fs.children` as sub-fields (Sanity's `_type`/`_key` and other `_` keys left out).
fn sub_fields(
    fs: &FieldStats,
    mapping: &MappingConfig,
    own: &str,
    at: &str,
    infos: &mut Vec<String>,
) -> Vec<Field> {
    fs.children
        .iter()
        .filter(|(k, _)| !k.starts_with('_'))
        .map(|(k, c)| Field {
            name: k.clone(),
            ..kind_for(c, &[], mapping, own, &format!("{at}.{k}"), infos)
        })
        .collect()
}

/// The Payload field for `fs`. `at` (`<collection>.<field>`) names it in the Info pushed to
/// `infos` when its shapes are mixed and it falls back to `json`.
fn kind_for(
    fs: &FieldStats,
    ts: &[crate::mapping::Transform],
    mapping: &MappingConfig,
    own: &str,
    at: &str,
    infos: &mut Vec<String>,
) -> Field {
    let vs = effective(&fs.variants, ts);
    let mut f = Field::default();
    let rel = || {
        let mut r: Vec<String> = fs
            .relation_types
            .iter()
            .filter_map(|t| collection_of(mapping, t).map(String::from))
            .collect();
        r.sort();
        r.dedup();
        if r.is_empty() {
            r.push(own.to_string());
        }
        r
    };
    let one = |v: &str, f: &mut Field| match v {
        "Text" => {
            f.kind = if fs.samples.iter().any(|s| s.contains('\n') || s.len() > 120) {
                "textarea".into()
            } else {
                "text".into()
            }
        }
        "Number" => f.kind = "number".into(),
        "Bool" => f.kind = "checkbox".into(),
        "Date" => f.kind = "date".into(),
        "RichText" => f.kind = "richText".into(),
        "Layout" => {
            f.kind = "blocks".into();
            f.blocks = block_slugs(mapping);
        }
        "Media" => {
            f.kind = "upload".into();
            f.relation_to = vec![mapping.media.collection.clone()];
        }
        "Doc" => {
            f.kind = "relationship".into();
            f.relation_to = rel();
        }
        _ => {}
    };
    let v = match vs.len() {
        0 => {
            f.kind = "text".into();
            return f;
        }
        1 => vs.first().map(String::as_str).unwrap_or_default(),
        _ => "",
    };
    let items = &fs.item_variants;
    let nested = match v {
        "Object" => Some("group"),
        "List" if items.len() == 1 && items.contains("Object") => Some("array"),
        "List" => {
            if let Some(i @ ("Doc" | "Media" | "Text" | "Number")) = items
                .first()
                .map(String::as_str)
                .filter(|_| items.len() == 1)
            {
                one(i, &mut f);
                f.many = true;
            }
            None
        }
        v => {
            one(v, &mut f);
            None
        }
    };
    if let Some(kind) = nested {
        f.fields = sub_fields(fs, mapping, own, at, infos);
        if !f.fields.is_empty() {
            f.kind = kind.into();
        }
    }
    if f.kind.is_empty() {
        f.kind = "json".into();
        f.fields.clear();
        infos.push(format!("{at} has mixed shapes; generated as json"));
    }
    f
}

/// The Payload field a mapped source field should become, in words: `relationship -> posts (hasMany)`.
pub fn describe(
    fs: &FieldStats,
    ts: &[crate::mapping::Transform],
    mapping: &MappingConfig,
    own: &str,
) -> String {
    let f = kind_for(fs, ts, mapping, own, "", &mut vec![]);
    let mut s = f.kind.clone();
    if !f.relation_to.is_empty() {
        s.push_str(&format!(" -> {}", f.relation_to.join(" | ")));
    }
    if !f.blocks.is_empty() {
        s.push_str(&format!(": {}", f.blocks.join(", ")));
    }
    if f.many {
        s.push_str(" (hasMany)");
    }
    s
}

/// Puts `f` at its dotted `path`, creating groups on the way.
fn insert(fields: &mut Vec<Field>, path: &str, mut f: Field) {
    match path.split_once('.') {
        None => {
            f.name = path.to_string();
            if !fields.iter().any(|x| x.name == f.name) {
                fields.push(f);
            }
        }
        Some((head, rest)) => {
            let i = match fields.iter().position(|x| x.name == head) {
                Some(i) => i,
                None => {
                    fields.push(Field {
                        name: head.into(),
                        kind: "group".into(),
                        ..Field::default()
                    });
                    fields.len() - 1
                }
            };
            insert(&mut fields[i].fields, rest, f);
        }
    }
}

fn render(fields: &[Field], indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for f in fields {
        out.push_str(&format!(
            "{pad}{{\n{pad}  name: {},\n{pad}  type: {},\n",
            q(&f.name),
            q(&f.kind)
        ));
        if f.required {
            out.push_str(&format!("{pad}  required: true,\n"));
        }
        match f.relation_to.as_slice() {
            [] => {}
            [one] => out.push_str(&format!("{pad}  relationTo: {},\n", q(one))),
            many => out.push_str(&format!(
                "{pad}  relationTo: [{}],\n",
                many.iter().map(|s| q(s)).collect::<Vec<_>>().join(", ")
            )),
        }
        if f.many {
            out.push_str(&format!("{pad}  hasMany: true,\n"));
        }
        if f.kind == "blocks" {
            let names: Vec<String> = f.blocks.iter().map(|b| block_ident(b)).collect();
            out.push_str(&format!("{pad}  blocks: [{}],\n", names.join(", ")));
        }
        if matches!(f.kind.as_str(), "group" | "array") {
            out.push_str(&format!("{pad}  fields: [\n"));
            render(&f.fields, indent + 4, out);
            out.push_str(&format!("{pad}  ],\n"));
        }
        out.push_str(&format!("{pad}}},\n"));
    }
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("create {}", d.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

/// Files `generate` wrote, and an Info per field that fell back to `json`.
pub struct Generated {
    pub files: Vec<PathBuf>,
    pub infos: Vec<String>,
}

/// What `generate_into` wrote, or the slugs that stopped it writing anything.
#[derive(Debug, Default)]
pub struct IntoReport {
    pub written: Vec<PathBuf>,
    pub collisions: Vec<String>,
}

/// The `payload.config.ts` lines that wire in what `--into` wrote.
pub const CONFIG_LINES: &str = "import { porterCollections } from './collections/porter'\n\
collections: [/* your collections */, ...porterCollections],\n";

/// `app/(frontend)/[[...path]]/page.tsx`: a plain page for each document `--into` loaded,
/// `/` included (no segments).
pub const CATCH_ALL_ROUTE: &str = r#"import config from '@payload-config'
import { RichText } from '@payloadcms/richtext-lexical/react'
import { notFound } from 'next/navigation'
import { getPayload, type CollectionSlug } from 'payload'
import { porterCollections } from '@/collections/porter'

const HIDDEN = ['id', 'title', 'slug', 'path', 'legacyId', 'createdAt', 'updatedAt', '_status']

export default async function Page({ params }: { params: Promise<{ path?: string[] }> }) {
  const want = '/' + ((await params).path ?? []).join('/')
  const payload = await getPayload({ config })
  for (const { slug } of porterCollections) {
    const { docs } = await payload.find({
      collection: slug as CollectionSlug,
      where: { path: { in: [want, want + '/'] } },
      limit: 1,
    })
    const doc = docs[0] as Record<string, unknown> | undefined
    if (!doc) continue
    return (
      <main>
        <h1>{String(doc.title)}</h1>
        {Object.entries(doc).map(([k, v]) =>
          HIDDEN.includes(k) ? null
          : typeof v === 'string' || typeof v === 'number' ? <p key={k}>{v}</p>
          : v && typeof v === 'object' && 'root' in v ? <RichText key={k} data={v as never} />
          : null,
        )}
      </main>
    )
  }
  notFound()
}
"#;

/// Generated config file texts, before they are written anywhere.
struct Built {
    collections: Vec<(String, String, String)>, // (slug, ident, text)
    blocks: Vec<(String, String)>,              // (ident, text)
    infos: Vec<String>,
}

/// `blocks_dir`: the blocks folder relative to the collections folder.
fn build(summary: &TypeSummary, mapping: &MappingConfig, blocks_dir: &str) -> Built {
    let mut infos = vec![];
    let mut colls: IndexMap<&str, (KeyType, Vec<Field>)> = IndexMap::new();
    for (ty, tm) in &mapping.types {
        let Some(coll) = tm.to.as_deref() else {
            continue;
        };
        let entry = colls.entry(coll).or_insert_with(|| (tm.key_type, vec![]));
        let stats = summary.types.get(ty);
        for (src, fm) in tm.fields.iter().filter(|(_, fm)| !fm.drop) {
            if fm.to == tm.key_field || ["title", "slug", "path"].contains(&fm.to.as_str()) {
                continue;
            }
            let fs = stats
                .and_then(|s| s.field(src))
                .cloned()
                .unwrap_or_default();
            let at = format!("{coll}.{}", fm.to);
            let f = kind_for(&fs, &fm.transform, mapping, coll, &at, &mut infos);
            insert(&mut entry.1, &fm.to, f);
        }
    }
    let mut out = Built {
        collections: vec![],
        blocks: vec![],
        infos: vec![],
    };
    for (coll, (key_type, fields)) in &colls {
        let ident = pascal(coll);
        let key = mapping
            .types
            .values()
            .find(|t| t.to.as_deref() == Some(coll))
            .map(|t| t.key_field.clone())
            .unwrap_or_else(|| "legacyId".into());
        let key_kind = match key_type {
            KeyType::Text => "text",
            KeyType::Number => "number",
        };
        let mut body = String::new();
        render(fields, 4, &mut body);
        let mut imports = String::from("import type { CollectionConfig } from 'payload'\n");
        if fields_use_blocks(fields) {
            for b in block_slugs(mapping) {
                let id = block_ident(&b);
                imports.push_str(&format!("import {{ {id} }} from '{blocks_dir}/{id}'\n"));
            }
        }
        let text = format!(
            "// Generated by `porter schema generate`. Review before you rely on it.\n\
             {imports}\n\
             export const {ident}: CollectionConfig = {{\n\
             \x20 slug: {slug},\n\
             \x20 admin: {{ useAsTitle: 'title' }},\n\
             \x20 versions: {{ drafts: true }},\n\
             \x20 fields: [\n\
             \x20   {{\n\
             \x20     name: {key},\n\
             \x20     type: '{key_kind}',\n\
             \x20     index: true,\n\
             \x20     unique: true,\n\
             \x20     admin: {{ hidden: true, readOnly: true }},\n\
             \x20   }},\n\
             \x20   {{ name: 'title', type: 'text', required: true }},\n\
             \x20   {{ name: 'slug', type: 'text', index: true }},\n\
             \x20   {{ name: 'path', type: 'text', index: true, unique: true, admin: {{ readOnly: true }} }},\n\
             {body}\
             \x20 ],\n\
             }}\n",
            slug = q(coll),
            key = q(&key),
        );
        out.collections.push((coll.to_string(), ident, text));
    }
    for slug in block_slugs(mapping) {
        let id = block_ident(&slug);
        let mut fields = vec![];
        if mapping.raw_block.as_deref() == Some(slug.as_str()) {
            fields.push(Field {
                name: "html".into(),
                kind: "textarea".into(),
                ..Field::default()
            });
        }
        for m in mapping.blocks.iter().filter(|m| m.to == slug) {
            for (src, dest) in &m.fields {
                let mut fs = FieldStats::default();
                for b in summary.blocks.iter().filter(|b| block_matches(m, b)) {
                    if let Some(p) = b.props.get(src) {
                        fs.merge(p);
                    }
                }
                let own = mapping
                    .types
                    .values()
                    .find_map(|t| t.to.as_deref())
                    .unwrap_or("pages");
                let at = format!("{slug}.{dest}");
                insert(
                    &mut fields,
                    dest,
                    kind_for(&fs, &[], mapping, own, &at, &mut infos),
                );
            }
        }
        let mut body = String::new();
        render(&fields, 4, &mut body);
        let text = format!(
            "// Generated by `porter schema generate`.\n\
             import type {{ Block }} from 'payload'\n\n\
             export const {id}: Block = {{\n\
             \x20 slug: {slug},\n\
             \x20 interfaceName: '{id}',\n\
             \x20 fields: [\n\
             {body}\
             \x20 ],\n\
             }}\n",
            slug = q(&slug),
        );
        out.blocks.push((id, text));
    }
    out.infos = infos;
    out
}

/// Writes collection and block configs to `out_dir` (blocks under `blocks/`) with an
/// `index.ts` re-exporting them.
pub fn generate(
    summary: &TypeSummary,
    mapping: &MappingConfig,
    out_dir: &Path,
) -> Result<Generated> {
    let b = build(summary, mapping, "./blocks");
    let mut files = vec![];
    let mut index = String::new();
    for (_, ident, text) in &b.collections {
        let p = out_dir.join(format!("{ident}.ts"));
        write(&p, text)?;
        index.push_str(&format!("export {{ {ident} }} from './{ident}'\n"));
        files.push(p);
    }
    for (id, text) in &b.blocks {
        let p = out_dir.join("blocks").join(format!("{id}.ts"));
        write(&p, text)?;
        index.push_str(&format!("export {{ {id} }} from './blocks/{id}'\n"));
        files.push(p);
    }
    let p = out_dir.join("index.ts");
    write(&p, &index)?;
    files.push(p);
    Ok(Generated {
        files,
        infos: b.infos,
    })
}

/// Collection slugs declared in `<repo>/src/collections/*.ts` (not under `porter/`, which
/// is ours), with the file that declares each.
fn declared_slugs(repo: &Path) -> Result<Vec<(String, String)>> {
    let dir = repo.join("src").join("collections");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(vec![]);
    };
    let re = regex::Regex::new(r#"slug:\s*['"]([^'"]+)['"]"#)?;
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "ts"))
        .collect();
    files.sort();
    let mut out = vec![];
    for f in files {
        let text = std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?;
        let rel = f.strip_prefix(repo).unwrap_or(&f).display().to_string();
        for c in re.captures_iter(&text) {
            out.push((c[1].to_string(), rel.clone()));
        }
    }
    Ok(out)
}

/// `porter schema generate --into <repo>`: collections to `src/collections/porter/`, blocks
/// to `src/blocks/porter/`, and `src/collections/porter/index.ts` exporting
/// `porterCollections` and `porterBlocks`. A slug the repo already declares is an Error
/// issue, and then nothing is written.
pub fn generate_into(ctx: &StageCtx, repo: &Path) -> Result<IntoReport> {
    let summary = TypeSummary::from_store(ctx.project.store())?;
    let b = build(&summary, &ctx.config.mapping, "../../blocks/porter");
    for i in &b.infos {
        ctx.issue(
            IssueKind::SchemaMismatch,
            Severity::Info,
            None,
            i.clone(),
            json!({}),
        )?;
    }
    let declared = declared_slugs(repo)?;
    let mut report = IntoReport::default();
    for (slug, ..) in &b.collections {
        if let Some((_, file)) = declared.iter().find(|(s, _)| s == slug) {
            ctx.issue(
                IssueKind::SchemaMismatch,
                Severity::Error,
                None,
                format!("collection `{slug}` is already declared in {file}; nothing written"),
                json!({"slug": slug, "file": file}),
            )?;
            report.collisions.push(slug.clone());
        }
    }
    if !report.collisions.is_empty() {
        return Ok(report);
    }
    let colls = repo.join("src").join("collections").join("porter");
    let blocks = repo.join("src").join("blocks").join("porter");
    let mut imports = String::new();
    for (_, ident, text) in &b.collections {
        let p = colls.join(format!("{ident}.ts"));
        write(&p, text)?;
        imports.push_str(&format!("import {{ {ident} }} from './{ident}'\n"));
        report.written.push(p);
    }
    for (id, text) in &b.blocks {
        let p = blocks.join(format!("{id}.ts"));
        write(&p, text)?;
        imports.push_str(&format!(
            "import {{ {id} }} from '../../blocks/porter/{id}'\n"
        ));
        report.written.push(p);
    }
    let list = |xs: Vec<&str>| xs.join(", ");
    let index = format!(
        "// Generated by `porter schema generate --into`. Spread these into your Payload config.\n\
         import type {{ Block, CollectionConfig }} from 'payload'\n\
         {imports}\n\
         export const porterCollections: CollectionConfig[] = [{}]\n\
         export const porterBlocks: Block[] = [{}]\n",
        list(b.collections.iter().map(|c| c.1.as_str()).collect()),
        list(b.blocks.iter().map(|x| x.0.as_str()).collect()),
    );
    let p = colls.join("index.ts");
    write(&p, &index)?;
    report.written.push(p);
    let n = report.written.len() as u64;
    for (i, p) in report.written.iter().enumerate() {
        ctx.progress(i as u64 + 1, Some(n), format!("wrote {}", p.display()));
    }
    Ok(report)
}

fn fields_use_blocks(fields: &[Field]) -> bool {
    fields
        .iter()
        .any(|f| f.kind == "blocks" || fields_use_blocks(&f.fields))
}
