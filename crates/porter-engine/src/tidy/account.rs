//! The honesty rule for tidy-ups: per rule, items in and out, and per field of every
//! merged item whether it is carried, renamed or dropped. Problems block load.

use super::compile::DISCRIMINATOR;
use super::{TidyKind, effective_mapping};
use crate::config::ProjectConfig;
use crate::destinations::payload::schema_check::{effective, find};
use crate::destinations::payload::ts::DestSchema;
use crate::mapping::Transform;
use crate::mapping::summary::{FieldStats, TypeSummary};
use crate::stage::StageCtx;
use crate::store::Store;
use anyhow::{Context, Result};
use porter_model::{Block, Document, FieldValue, IssueKind, ItemKind, Severity};
use serde::Serialize;
use serde_json::json;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct RuleOutcome {
    pub rule: String,
    pub items_in: u64,
    pub items_out: u64,
    pub carried: u64,
    pub renamed: u64,
    pub dropped: u64,
    pub problems: Vec<Problem>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Problem {
    /// One destination field fed values of different shapes (`shapes[i]` in `types[i]`).
    ShapeConflict {
        field: String,
        types: Vec<String>,
        shapes: Vec<String>,
    },
    /// A carried field the existing destination `dest` has no field for.
    Unaccounted { field: String, dest: String },
}

impl Problem {
    /// The issue message, naming the rule.
    pub fn message(&self, rule: &str) -> String {
        match self {
            Problem::ShapeConflict {
                field,
                types,
                shapes,
            } => format!(
                "tidy \"{rule}\": {field} is {} in {} but {} in {}",
                shapes[0], types[0], shapes[1], types[1]
            ),
            Problem::Unaccounted { field, dest } => {
                format!("tidy \"{rule}\": {dest} has no field {field}")
            }
        }
    }
}

/// A field's shape after its transforms: `text`, `list`, `doc/text`...; None when never filled.
fn shape(fs: &FieldStats, ts: &[Transform]) -> Option<String> {
    let vs = effective(&fs.variants, ts);
    (!vs.is_empty()).then(|| {
        vs.iter()
            .map(|v| v.to_lowercase())
            .collect::<Vec<_>>()
            .join("/")
    })
}

/// Shape conflicts among `(dest field, type, shape)`, one per field: the first type seen
/// against the first that differs.
fn conflicts(seen: &[(String, String, String)]) -> Vec<Problem> {
    let mut out = vec![];
    let mut done = std::collections::HashSet::new();
    for f in seen.iter().map(|(f, ..)| f.as_str()) {
        if !done.insert(f) {
            continue;
        }
        let mut of = seen.iter().filter(|(x, ..)| x == f);
        let Some((_, t0, s0)) = of.next() else {
            continue;
        };
        if let Some((_, t1, s1)) = of.find(|(_, _, s)| s != s0) {
            out.push(Problem::ShapeConflict {
                field: f.into(),
                types: vec![t0.clone(), t1.clone()],
                shapes: vec![s0.clone(), s1.clone()],
            });
        }
    }
    out
}

fn unaccounted(out: &mut Vec<Problem>, field: &str, dest: &str) {
    let p = Problem::Unaccounted {
        field: field.into(),
        dest: dest.into(),
    };
    if !out.contains(&p) {
        out.push(p);
    }
}

/// Every block in `v`, nested ones included.
fn blocks<'a>(v: &'a FieldValue, out: &mut Vec<&'a Block>) {
    match v {
        FieldValue::Layout(bs) => bs.iter().for_each(|b| block(b, out)),
        FieldValue::List(xs) => xs.iter().for_each(|x| blocks(x, out)),
        FieldValue::Object(m) => m.values().for_each(|x| blocks(x, out)),
        _ => {}
    }
}

fn block<'a>(b: &'a Block, out: &mut Vec<&'a Block>) {
    out.push(b);
    b.children.iter().for_each(|c| block(c, out));
    b.props.values().for_each(|v| blocks(v, out));
}

/// The status as `[mapping.types.*.status]` keys name it.
fn wire_status(d: &Document) -> String {
    serde_json::to_value(d.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// One outcome per enabled rule, over the normalised documents of `cfg.source.adapter`.
/// `dest` is the existing destination's schema when there is one: then a carried field
/// it has no field for is `Unaccounted`.
pub fn account(
    store: &Store,
    cfg: &ProjectConfig,
    dest: Option<&DestSchema>,
) -> Result<Vec<RuleOutcome>> {
    let rules: Vec<_> = cfg.tidy.iter().filter(|r| r.enabled).collect();
    if rules.is_empty() {
        return Ok(vec![]);
    }
    let m = effective_mapping(cfg)?;
    let summary = TypeSummary::from_store(store)?;
    let docs: Vec<Document> = store
        .items_decodable::<Document>(ItemKind::Document)?
        .into_iter()
        .filter(|(_, s, _)| s.connector == cfg.source.adapter)
        .map(|(.., d)| d)
        .collect();
    let mut out = vec![];
    for rule in rules {
        let mut o = RuleOutcome {
            rule: rule.id.clone(),
            ..RuleOutcome::default()
        };
        match &rule.kind {
            TidyKind::MergeTypes { from, into, .. } => {
                let coll = dest.and_then(|d| d.collections.get(into));
                let mut seen = vec![];
                let mut missing = vec![];
                for ty in from {
                    let Some(tm) = m.types.get(ty) else { continue };
                    let stats = summary.types.get(ty);
                    for (src, fm) in tm.fields.iter().filter(|(_, fm)| !fm.drop) {
                        let fs = stats.and_then(|s| s.field(src));
                        if let Some(sh) = fs.and_then(|fs| shape(fs, &fm.transform)) {
                            seen.push((fm.to.clone(), ty.clone(), sh));
                        }
                        if let Some(c) = coll
                            && (fs.is_some() || src == DISCRIMINATOR)
                            && find(&c.fields, &fm.to).is_none()
                        {
                            unaccounted(&mut missing, &fm.to, into);
                        }
                    }
                }
                o.problems = conflicts(&seen);
                o.problems.extend(missing);
                for d in docs.iter().filter(|d| from.contains(&d.doc_type)) {
                    let Some(tm) = m.types.get(&d.doc_type) else {
                        continue;
                    };
                    o.items_in += 1;
                    let skipped = tm.status.get(&wire_status(d)).is_some_and(|s| s == "skip");
                    if tm.to.as_deref() == Some(into.as_str()) && !skipped {
                        o.items_out += 1;
                    }
                    for (k, v) in &d.fields {
                        if matches!(v, FieldValue::Null) {
                            continue;
                        }
                        match tm.fields.get(k) {
                            Some(fm) if fm.drop => o.dropped += 1,
                            Some(fm) if &fm.to == k => o.carried += 1,
                            Some(_) => o.renamed += 1,
                            None => {}
                        }
                    }
                }
            }
            TidyKind::MergeBlocks {
                from, into, drop, ..
            } => {
                let map_of = |module: &str| {
                    m.blocks
                        .iter()
                        .find(|b| b.r#match.module.as_deref() == Some(module))
                };
                let dest_block = dest.and_then(|d| d.blocks.get(into));
                let mut seen = vec![];
                let mut missing = vec![];
                for bs in summary
                    .blocks
                    .iter()
                    .filter(|b| b.module.as_ref().is_some_and(|md| from.contains(md)))
                {
                    let (Some(md), Some(map)) = (&bs.module, bs.module.as_deref().and_then(map_of))
                    else {
                        continue;
                    };
                    for (prop, fs) in &bs.props {
                        let Some(to) = map.fields.get(prop) else {
                            continue;
                        };
                        if let Some(sh) = shape(fs, &[]) {
                            seen.push((to.clone(), md.clone(), sh));
                        }
                        if let Some(fields) = dest_block
                            && find(fields, to).is_none()
                        {
                            unaccounted(&mut missing, to, into);
                        }
                    }
                }
                o.problems = conflicts(&seen);
                o.problems.extend(missing);
                let mut all = vec![];
                for d in &docs {
                    d.fields.values().for_each(|v| blocks(v, &mut all));
                }
                for b in all {
                    let Some(md) = b.original.as_ref().map(|x| x.module.as_str()) else {
                        continue;
                    };
                    if !from.iter().any(|f| f == md) {
                        continue;
                    }
                    o.items_in += 1;
                    let map = m.blocks.iter().find(|x| x.matches(b));
                    if map.is_some_and(|x| &x.to == into) {
                        o.items_out += 1;
                    }
                    for (k, v) in &b.props {
                        if matches!(v, FieldValue::Null) {
                            continue;
                        }
                        match map.and_then(|x| x.fields.get(k)) {
                            Some(to) if to == k => o.carried += 1,
                            Some(_) => o.renamed += 1,
                            None if drop.contains_key(&format!("{md}.{k}")) => o.dropped += 1,
                            None => {}
                        }
                    }
                }
            }
        }
        out.push(o);
    }
    Ok(out)
}

/// Runs `account`, writes `reports/tidy.json` and opens an error per problem. False: load
/// must not go on.
pub fn check_before_load(ctx: &StageCtx, dest: Option<&DestSchema>) -> Result<bool> {
    let out = account(ctx.project.store(), &ctx.config, dest)?;
    let p = ctx.project.reports_dir().join(REPORT);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(&out)?)
        .with_context(|| format!("write {}", p.display()))?;
    let mut clean = true;
    for o in &out {
        for problem in &o.problems {
            clean = false;
            ctx.issue(
                IssueKind::SchemaMismatch,
                Severity::Error,
                None,
                problem.message(&o.rule),
                json!({"rule": o.rule, "problem": problem}),
            )?;
        }
    }
    Ok(clean)
}

/// Under `reports/`: the last load's rule outcomes.
pub const REPORT: &str = "tidy.json";
