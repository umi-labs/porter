//! The flow graph: how content moves from the source, through any tidy-ups, into the
//! destination. Built here once; the hub and the desktop app both draw it, and
//! `porter flow` prints it. Normalise and load write it to `reports/flow.json`.

use crate::config::ProjectConfig;
use crate::destinations::payload::DRY_RUN;
use crate::mapping::{MappingConfig, Transform, TypeMap};
use crate::project::Project;
use crate::store::Store;
use crate::tidy::suggest::{humanise, list, plural_name};
use crate::tidy::{DISCRIMINATOR, TidyKind, TidyRule, effective_mapping};
use anyhow::{Context, Result};
use indexmap::IndexMap;
use porter_model::{
    Block, Document, FieldValue, Form, ItemId, ItemKind, ItemStatus, Stage, Submission,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FlowGraph {
    pub v: u8,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
    /// When the load the counts come from finished (RFC 3339); None: no load since the
    /// last normalise, so nothing is counted.
    pub counted_at: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FlowNode {
    pub id: String,
    pub column: Column,
    pub kind: NodeKind,
    pub title: String,
    pub summary: String,
    pub enabled: bool,
    pub counts: Option<Counts>,
    pub example: Option<Example>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Column {
    Source,
    Tidy,
    Destination,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    DocType,
    BlockType,
    Submissions,
    MergeTypes,
    MergeBlocks,
    Collection,
    Block,
    Form,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub items_in: u64,
    pub ok: u64,
    pub skipped: u64,
    pub failed: u64,
    pub dropped: u64,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.items_in += o.items_in;
        self.ok += o.ok;
        self.skipped += o.skipped;
        self.failed += o.failed;
        self.dropped += o.dropped;
    }

    fn tally(&mut self, s: Option<&ItemStatus>) {
        self.items_in += 1;
        match s {
            Some(ItemStatus::Done | ItemStatus::Partial { .. }) => self.ok += 1,
            // A dry run "skips" what it would have written.
            Some(ItemStatus::Skipped { reason }) if reason == DRY_RUN => self.ok += 1,
            Some(ItemStatus::Skipped { .. }) => self.skipped += 1,
            Some(ItemStatus::Failed { .. }) => self.failed += 1,
            None => {}
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Example {
    pub title: String,
    /// `(field, before, after)`: after is where the field goes, or `(dropped)`.
    pub changed: Vec<(String, String, String)>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub count: Option<u64>,
}

/// Under `reports/`.
pub const REPORT: &str = "flow.json";

/// Builds the graph and writes `reports/flow.json`.
pub fn write(project: &Project, cfg: &ProjectConfig) -> Result<FlowGraph> {
    let g = build(project, cfg)?;
    let p = project.reports_dir().join(REPORT);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(&g)?)
        .with_context(|| format!("write {}", p.display()))?;
    Ok(g)
}

/// The finish time of the last load that ran after the last finished normalise.
fn counted_at(store: &Store) -> Result<Option<String>> {
    let Some(norm) = store
        .runs(Stage::Normalise)?
        .into_iter()
        .rfind(|r| r.ended_at.is_some())
    else {
        return Ok(None);
    };
    Ok(store
        .runs(Stage::Load)?
        .into_iter()
        .filter(|r| r.id > norm.id)
        .filter_map(|r| r.ended_at)
        .next_back())
}

/// A field value as a short display string.
fn show(v: Option<&FieldValue>, titles: &HashMap<ItemId, String>) -> String {
    let s = match v {
        None | Some(FieldValue::Null) => String::new(),
        Some(FieldValue::Text(s)) => s.clone(),
        Some(FieldValue::Number(n)) => n.to_string(),
        Some(FieldValue::Bool(b)) => b.to_string(),
        Some(FieldValue::Date(d)) => d.to_rfc3339(),
        Some(FieldValue::RichText(_)) => "(rich text)".into(),
        Some(FieldValue::Layout(bs)) => format!("({} blocks)", bs.len()),
        Some(FieldValue::Media(_)) => "(image)".into(),
        Some(FieldValue::Doc(id)) => titles.get(id).cloned().unwrap_or_else(|| "(link)".into()),
        Some(FieldValue::List(xs)) => format!("({} items)", xs.len()),
        Some(FieldValue::Object(m)) => format!("({} fields)", m.len()),
    };
    if s.chars().count() > 80 {
        s.chars().take(79).collect::<String>() + "…"
    } else {
        s
    }
}

fn doc_example(d: &Document, tm: &TypeMap, titles: &HashMap<ItemId, String>) -> Option<Example> {
    let mut changed = vec![];
    if let Some(fm) = tm.fields.get(DISCRIMINATOR)
        && let Some(Transform::Default { value }) = fm.transform.first()
    {
        changed.push((fm.to.clone(), "(none)".into(), value.clone()));
    }
    if let Some(p) = &tm.parent {
        changed.push((p.clone(), show(d.fields.get(p), titles), "parent".into()));
    }
    for (k, v) in d
        .fields
        .iter()
        .filter(|(_, v)| !matches!(v, FieldValue::Null))
    {
        match tm.fields.get(k) {
            Some(fm) if fm.drop => {
                changed.push((k.clone(), show(Some(v), titles), "(dropped)".into()))
            }
            Some(fm) if &fm.to != k => {
                changed.push((k.clone(), show(Some(v), titles), fm.to.clone()))
            }
            _ => {}
        }
    }
    changed.truncate(5);
    (!changed.is_empty()).then(|| Example {
        title: d.title.clone(),
        changed,
    })
}

fn block_example(
    d: &Document,
    b: &Block,
    m: &MappingConfig,
    drop: Option<&IndexMap<String, String>>,
    titles: &HashMap<ItemId, String>,
) -> Option<Example> {
    let md = b
        .original
        .as_ref()
        .map(|o| o.module.as_str())
        .unwrap_or_default();
    let map = m.blocks.iter().find(|x| x.matches(b));
    let mut changed = vec![];
    for (k, v) in b
        .props
        .iter()
        .filter(|(_, v)| !matches!(v, FieldValue::Null))
    {
        match map.and_then(|x| x.fields.get(k)) {
            Some(to) if to != k => changed.push((k.clone(), show(Some(v), titles), to.clone())),
            None if drop.is_some_and(|dr| dr.contains_key(&format!("{md}.{k}"))) => {
                changed.push((k.clone(), show(Some(v), titles), "(dropped)".into()))
            }
            _ => {}
        }
    }
    changed.truncate(5);
    (!changed.is_empty()).then(|| Example {
        title: d.title.clone(),
        changed,
    })
}

fn from_into(r: &TidyRule) -> (&[String], &str) {
    match &r.kind {
        TidyKind::MergeTypes { from, into, .. } | TidyKind::MergeBlocks { from, into, .. } => {
            (from, into)
        }
    }
}

/// "City pages", "Hotel breaks"; "Site settings" stays as it is.
fn type_title(ty: &str) -> String {
    let h = humanise(ty);
    if h.ends_with('s') {
        h
    } else {
        humanise(&plural_name(ty))
    }
}

#[derive(Default)]
struct Builder {
    nodes: Vec<FlowNode>,
    dests: BTreeMap<String, FlowNode>,
    edges: Vec<FlowEdge>,
    counts: HashMap<String, Counts>,
}

impl Builder {
    fn node(&mut self, id: String, column: Column, kind: NodeKind, title: String, summary: String) {
        self.nodes.push(FlowNode {
            id,
            column,
            kind,
            title,
            summary,
            enabled: true,
            counts: None,
            example: None,
        });
    }

    fn edge(&mut self, from: &str, to: &str) {
        if !self.edges.iter().any(|e| e.from == from && e.to == to) {
            self.edges.push(FlowEdge {
                from: from.into(),
                to: to.into(),
                count: None,
            });
        }
    }

    fn dest(&mut self, id: String, kind: NodeKind, title: String, summary: String) {
        self.dests.entry(id.clone()).or_insert(FlowNode {
            id,
            column: Column::Destination,
            kind,
            title,
            summary,
            enabled: true,
            counts: None,
            example: None,
        });
    }

    fn collection(&mut self, coll: &str) -> String {
        let id = format!("dst:{coll}");
        let t = humanise(coll);
        self.dest(
            id.clone(),
            NodeKind::Collection,
            t.clone(),
            format!("The {t} collection."),
        );
        id
    }

    fn block(&mut self, slug: &str) -> String {
        let id = format!("dstblk:{slug}");
        let t = humanise(slug);
        self.dest(
            id.clone(),
            NodeKind::Block,
            t.clone(),
            format!("The {t} block."),
        );
        id
    }
}

pub fn build(project: &Project, cfg: &ProjectConfig) -> Result<FlowGraph> {
    let store = project.store();
    let m = effective_mapping(cfg)?;
    let counted_at = counted_at(store)?;
    let statuses: HashMap<ItemId, ItemStatus> = store.statuses(Stage::Load)?.into_iter().collect();
    let mut docs: Vec<Document> = store
        .items_decodable::<Document>(ItemKind::Document)?
        .into_iter()
        .filter(|(_, s, _)| s.connector == cfg.source.adapter)
        .map(|(.., d)| d)
        .collect();
    docs.sort_by(|a, b| (&a.title, a.id.to_string()).cmp(&(&b.title, b.id.to_string())));
    let titles: HashMap<ItemId, String> = docs.iter().map(|d| (d.id, d.title.clone())).collect();
    let enabled_rule = |name: &str, types: bool| {
        cfg.tidy.iter().find(|r| {
            r.enabled
                && matches!(r.kind, TidyKind::MergeTypes { .. }) == types
                && from_into(r).0.iter().any(|f| f == name)
        })
    };
    let mut g = Builder::default();

    // Document types.
    let mut by_type: BTreeMap<&str, Vec<&Document>> = BTreeMap::new();
    for d in &docs {
        by_type.entry(&d.doc_type).or_default().push(d);
    }
    for (ty, ds) in &by_type {
        let id = format!("src:{ty}");
        let title = type_title(ty);
        let tm = m.types.get(*ty);
        let summary = if let Some(r) = enabled_rule(ty, true) {
            g.edge(&id, &format!("tidy:{}", r.id));
            format!(
                "Goes through the tidy-up \"{}\" into {}.",
                r.id,
                humanise(from_into(r).1)
            )
        } else {
            match tm {
                Some(TypeMap { to: Some(c), .. }) => {
                    let dst = g.collection(c);
                    g.edge(&id, &dst);
                    let same =
                        tm.is_none_or(|t| t.fields.iter().all(|(k, fm)| !fm.drop && &fm.to == k));
                    let how = if same {
                        " unchanged"
                    } else {
                        ", renaming or leaving out some fields"
                    };
                    format!("Moves {title} into {}{how}.", humanise(c))
                }
                Some(TypeMap {
                    drop_reason: Some(why),
                    ..
                }) => format!("Left behind: {why}."),
                Some(_) => "Left behind: no collection is mapped.".into(),
                None => "Not mapped, so it is not loaded.".into(),
            }
        };
        let mut c = Counts::default();
        ds.iter().for_each(|d| c.tally(statuses.get(&d.id)));
        g.counts.insert(id.clone(), c);
        g.node(id, Column::Source, NodeKind::DocType, title, summary);
        if let (Some(n), Some(tm)) = (g.nodes.last_mut(), tm) {
            n.example = doc_example(ds[0], tm, &titles);
        }
    }

    // Block types, by module, in documents ordered by title.
    let mut by_module: BTreeMap<String, Vec<(&Document, &Block)>> = BTreeMap::new();
    for d in &docs {
        let mut all = vec![];
        d.fields
            .values()
            .for_each(|v| crate::tidy::blocks(v, &mut all));
        for b in all {
            if let Some(o) = &b.original {
                by_module.entry(o.module.clone()).or_default().push((d, b));
            }
        }
    }
    for (md, occ) in &by_module {
        let id = format!("blk:{md}");
        let title = format!("{} blocks", humanise(md));
        let (d0, b0) = occ[0];
        let map = m.blocks.iter().find(|x| x.matches(b0));
        let summary = if let Some(r) = enabled_rule(md, false) {
            g.edge(&id, &format!("tidy:{}", r.id));
            format!(
                "Goes through the tidy-up \"{}\" into the {} block.",
                r.id,
                humanise(from_into(r).1)
            )
        } else if let Some(map) = map {
            let dst = g.block(&map.to);
            g.edge(&id, &dst);
            format!("Moves {title} into the {} block.", humanise(&map.to))
        } else if let Some(raw) = &m.raw_block {
            let dst = g.block(raw);
            g.edge(&id, &dst);
            format!(
                "Has no block of its own, so it is kept as raw data in the {} block.",
                humanise(raw)
            )
        } else {
            "Has no block mapped, so it is not loaded.".into()
        };
        let mut c = Counts::default();
        occ.iter().for_each(|(d, _)| c.tally(statuses.get(&d.id)));
        g.counts.insert(id.clone(), c);
        g.node(id, Column::Source, NodeKind::BlockType, title, summary);
        if let Some(n) = g.nodes.last_mut() {
            n.example = block_example(d0, b0, &m, None, &titles);
        }
    }

    // Form submissions: counts only, never an example (they hold personal data).
    let subs = store.items_decodable::<Submission>(ItemKind::Submission)?;
    for (fid, _, form) in store.items_decodable::<Form>(ItemKind::Form)? {
        let id = format!("src:{}", form.title);
        let title = format!("{} submissions", humanise(&form.title));
        let summary = match (&m.forms.collection, &m.forms.submissions) {
            (Some(_), Some(coll)) => {
                let dst = format!("form:{}", form.title);
                let t = humanise(&form.title);
                g.dest(
                    dst.clone(),
                    NodeKind::Form,
                    t.clone(),
                    format!("The {t} form, with its past submissions in {}.", humanise(coll)),
                );
                g.edge(&id, &dst);
                format!("Moves {title} into {}. Shown as counts only.", humanise(coll))
            }
            _ => "Not loaded: set mapping.forms.collection and mapping.forms.submissions to keep them.".into(),
        };
        let mut c = Counts::default();
        subs.iter()
            .filter(|(.., s)| s.form == fid)
            .for_each(|(sid, ..)| c.tally(statuses.get(sid)));
        g.counts.insert(id.clone(), c);
        g.node(id, Column::Source, NodeKind::Submissions, title, summary);
    }

    // Tidy-ups, disabled ones included (drawn, but bypassed).
    let report: serde_json::Value =
        std::fs::read_to_string(project.reports_dir().join(crate::tidy::REPORT))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
    for r in &cfg.tidy {
        let (from, into) = from_into(r);
        let id = format!("tidy:{}", r.id);
        let mut names: Vec<String> = from.to_vec();
        names.sort();
        // What the rule does (or would do, when off).
        let mut on = cfg.clone();
        on.tidy
            .iter_mut()
            .filter(|x| x.id == r.id)
            .for_each(|x| x.enabled = true);
        let mo = effective_mapping(&on).ok();
        let (kind, mut summary, dst, example) = match &r.kind {
            TidyKind::MergeTypes { discriminator, .. } => {
                let ts: Vec<String> = names.iter().map(|t| type_title(t)).collect();
                let mut s = format!("Merges {} into {}.", list(&ts), humanise(into));
                if let Some(d) = discriminator {
                    s += &format!(" Each keeps what it was in {}.", humanise(&d.field));
                }
                let ex = mo.as_ref().and_then(|mo| {
                    let d = docs.iter().find(|d| from.contains(&d.doc_type))?;
                    doc_example(d, mo.types.get(&d.doc_type)?, &titles)
                });
                (NodeKind::MergeTypes, s, format!("dst:{into}"), ex)
            }
            TidyKind::MergeBlocks { drop, .. } => {
                let bs: Vec<String> = names.iter().map(|b| humanise(b)).collect();
                let s = format!(
                    "Merges {} blocks into the {} block.",
                    list(&bs),
                    humanise(into)
                );
                let ex = mo.as_ref().and_then(|mo| {
                    let (d, b) = by_module
                        .iter()
                        .filter(|(md, _)| from.contains(md))
                        .flat_map(|(_, occ)| occ.iter())
                        .min_by(|a, b| {
                            (&a.0.title, a.0.id.to_string()).cmp(&(&b.0.title, b.0.id.to_string()))
                        })?;
                    block_example(d, b, mo, Some(drop), &titles)
                });
                (NodeKind::MergeBlocks, s, format!("dstblk:{into}"), ex)
            }
        };
        if r.enabled {
            if kind == NodeKind::MergeTypes {
                g.collection(into);
            } else {
                g.block(into);
            }
            g.edge(&id, &dst);
            let mut c = Counts::default();
            let src = if kind == NodeKind::MergeTypes {
                "src"
            } else {
                "blk"
            };
            for f in from {
                if let Some(x) = g.counts.get(&format!("{src}:{f}")) {
                    c.add(x);
                }
            }
            c.dropped = report
                .as_array()
                .and_then(|a| a.iter().find(|o| o["rule"] == r.id.as_str()))
                .and_then(|o| o["dropped"].as_u64())
                .unwrap_or(0);
            g.counts.insert(id.clone(), c);
        } else {
            summary += " Off: passes straight through.";
        }
        let title = r.label.clone().unwrap_or_else(|| humanise(&r.id));
        g.node(id, Column::Tidy, kind, title, summary);
        if let Some(n) = g.nodes.last_mut() {
            n.enabled = r.enabled;
            n.example = example;
        }
    }

    // Destinations count what their edges bring in.
    for dst in g.dests.keys() {
        let mut c = Counts::default();
        for e in g.edges.iter().filter(|e| &e.to == dst) {
            if let Some(x) = g.counts.get(&e.from) {
                c.add(x);
            }
        }
        g.counts.insert(dst.clone(), c);
    }
    let Builder {
        mut nodes,
        dests,
        mut edges,
        counts,
    } = g;
    nodes.extend(dests.into_values());
    if counted_at.is_some() {
        for n in &mut nodes {
            n.counts = counts.get(&n.id).cloned();
        }
        for e in &mut edges {
            e.count = counts.get(&e.from).map(|c| c.ok);
        }
    }
    Ok(FlowGraph {
        v: 1,
        nodes,
        edges,
        counted_at,
    })
}
