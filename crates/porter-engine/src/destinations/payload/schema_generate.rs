//! `porter schema generate`: Payload 3 collection and block configs from the mapping and
//! what the documents hold. Written to the project's `generated/payload/`, never into the
//! client's own config.

use super::schema_check::effective;
use crate::mapping::summary::{BlockStats, FieldStats, TypeSummary};
use crate::mapping::{BlockMap, KeyType, MappingConfig};
use anyhow::{Context, Result};
use indexmap::IndexMap;
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

fn kind_for(
    fs: &FieldStats,
    ts: &[crate::mapping::Transform],
    mapping: &MappingConfig,
    own: &str,
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
        _ => f.kind = "json".into(),
    };
    match vs.len() {
        0 => f.kind = "text".into(),
        1 => {
            let v = vs.first().map(String::as_str).unwrap_or_default();
            if v == "List" {
                match fs.item_variants.iter().next().map(String::as_str) {
                    Some(i @ ("Doc" | "Media" | "Text" | "Number"))
                        if fs.item_variants.len() == 1 =>
                    {
                        one(i, &mut f);
                        f.many = true;
                    }
                    _ => f.kind = "json".into(),
                }
            } else {
                one(v, &mut f);
            }
        }
        _ => f.kind = "json".into(),
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
    let f = kind_for(fs, ts, mapping, own);
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

pub fn generate(
    summary: &TypeSummary,
    mapping: &MappingConfig,
    out_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let mut colls: IndexMap<&str, (KeyType, Vec<Field>)> = IndexMap::new();
    for (ty, tm) in &mapping.types {
        let Some(coll) = tm.to.as_deref() else {
            continue;
        };
        let entry = colls.entry(coll).or_insert_with(|| (tm.key_type, vec![]));
        let stats = summary.types.get(ty);
        for (src, fm) in tm.fields.iter().filter(|(_, fm)| !fm.drop) {
            if fm.to == tm.key_field || ["title", "slug"].contains(&fm.to.as_str()) {
                continue;
            }
            let fs = stats
                .and_then(|s| s.fields.get(src))
                .cloned()
                .unwrap_or_default();
            insert(
                &mut entry.1,
                &fm.to,
                kind_for(&fs, &fm.transform, mapping, coll),
            );
        }
    }
    let mut files = vec![];
    let mut index = String::new();
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
                imports.push_str(&format!("import {{ {id} }} from './blocks/{id}'\n"));
            }
        }
        let text = format!(
            "// Generated by `porter schema generate`. Review, then copy into your Payload config.\n\
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
             {body}\
             \x20 ],\n\
             }}\n",
            slug = q(coll),
            key = q(&key),
        );
        let p = out_dir.join(format!("{ident}.ts"));
        write(&p, &text)?;
        index.push_str(&format!("export {{ {ident} }} from './{ident}'\n"));
        files.push(p);
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
                        fs.variants.extend(p.variants.iter().cloned());
                        fs.item_variants.extend(p.item_variants.iter().cloned());
                        fs.relation_types.extend(p.relation_types.iter().cloned());
                        fs.samples.extend(p.samples.iter().cloned());
                    }
                }
                let own = mapping
                    .types
                    .values()
                    .find_map(|t| t.to.as_deref())
                    .unwrap_or("pages");
                insert(&mut fields, dest, kind_for(&fs, &[], mapping, own));
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
        let p = out_dir.join("blocks").join(format!("{id}.ts"));
        write(&p, &text)?;
        index.push_str(&format!("export {{ {id} }} from './blocks/{id}'\n"));
        files.push(p);
    }
    let p = out_dir.join("index.ts");
    write(&p, &index)?;
    files.push(p);
    Ok(files)
}

fn fields_use_blocks(fields: &[Field]) -> bool {
    fields
        .iter()
        .any(|f| f.kind == "blocks" || fields_use_blocks(&f.fields))
}
