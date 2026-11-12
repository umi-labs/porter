//! Mapping editor: the types, fields and blocks tables over porter.toml's `[mapping]`,
//! destination pickers from the parsed Payload config, and the schema check's issues
//! placed on the rows that cause them. Every edit goes through `MappingDoc` (toml_edit,
//! so hand comments survive) and is saved at once; the bridge saves and re-checks.

use indexmap::IndexMap;
use porter_engine::destinations::payload::schema_check::{compatible, effective, find};
use porter_engine::destinations::payload::schema_prompt::clean_sample;
use porter_engine::destinations::payload::ts::{DestField, DestSchema};
use porter_engine::html::PendingIssue;
use porter_engine::mapping::edit::{CHANGED_OUTSIDE, MappingDoc};
use porter_engine::mapping::summary::TypeSummary;
use porter_engine::mapping::{BlockMatch, MappingConfig, Transform};
use porter_model::Severity;
use serde_json::Value;

/// Shown instead of a picker when there is no destination schema to pick from.
pub const NO_SCHEMA: &str = "no destination schema (set destination.config_dir)";

/// What the bridge read: the document (as saved), what the documents hold, the
/// destination schema (None: `config_dir` not set) and the schema check's issues.
pub struct Loaded {
    pub doc: MappingDoc,
    pub summary: TypeSummary,
    pub dest: Result<Option<DestSchema>, String>,
    pub issues: Vec<PendingIssue>,
}

pub enum Reply {
    /// Opened, or saved and re-checked. The error is kept verbatim.
    Loaded(Result<Box<Loaded>, String>),
    /// The save was refused: nothing was written, and these say why.
    Refused(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowIssue {
    pub error: bool,
    pub message: String,
}

/// How bad a row is: its worst issue. None when it has none.
pub fn level(issues: &[RowIssue]) -> Option<Severity> {
    if issues.iter().any(|i| i.error) {
        Some(Severity::Error)
    } else if issues.is_empty() {
        None
    } else {
        Some(Severity::Warning)
    }
}

/// The row a problem sits on: what clicking it selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Type(String),
    Field(String, String),
    /// An index into `block_rows()`.
    Block(usize),
}

/// One problem and the row that causes it, for "Next problem" and the list of what
/// blocks Apply. `why` is the schema check's own message.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub target: Target,
    /// The row's name: `post`, `post.price`, `hero`.
    pub what: String,
    pub why: String,
    pub error: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypeRow {
    pub name: String,
    /// Normalised documents of this type.
    pub count: usize,
    /// No `[mapping.types.<name>]` entry at all.
    pub unmapped: bool,
    pub to: Option<String>,
    pub drop_reason: Option<String>,
    pub path: Option<String>,
    pub key: String,
    /// `private → draft, trash → skip`.
    pub status: String,
    pub issues: Vec<RowIssue>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldRow {
    pub path: String,
    pub samples: String,
    /// `62%`, or empty when the field was never seen.
    pub fill: String,
    /// No entry under the type's `fields`.
    pub unmapped: bool,
    pub to: String,
    pub transforms: Vec<Transform>,
    pub drop: bool,
    pub reason: Option<String>,
    pub issues: Vec<RowIssue>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockRow {
    /// What a new `[[mapping.blocks]]` entry for this row matches on, or the existing
    /// entry's own `match`.
    pub r#match: BlockMatch,
    pub kind: String,
    pub module: Option<String>,
    /// Blocks seen; 0 for a mapping entry nothing matched.
    pub count: usize,
    pub to: Option<String>,
    /// `title=heading, body=text`.
    pub fields: String,
    pub issues: Vec<RowIssue>,
}

/// A destination picker: choices from the schema, or free text and why.
#[derive(Clone, Debug, PartialEq)]
pub enum Picker {
    Choices(Vec<String>),
    FreeText(String),
}

#[derive(Default)]
pub struct Mapping {
    asked: bool,
    /// A save (and its re-check) is in flight; edits wait.
    saving: bool,
    loaded: Option<Result<Box<Loaded>, String>>,
    /// Parsed from the document as edited.
    config: MappingConfig,
    placed: Placed,
    pub selected_type: Option<String>,
    pub selected_field: Option<String>,
    pub selected_block: Option<usize>,
    /// Why the last save was refused.
    messages: Vec<String>,
}

impl Mapping {
    /// The section was shown: reads porter.toml and checks it, once. True: ask the bridge.
    pub fn open(&mut self) -> bool {
        if self.asked {
            return false;
        }
        self.asked = true;
        true
    }

    /// Re-reads porter.toml from disk (after it changed outside the editor), dropping
    /// unsaved edits. True: ask the bridge.
    pub fn reload(&mut self) -> bool {
        if self.saving {
            return false;
        }
        self.messages.clear();
        self.asked = true;
        true
    }

    /// The store changed (a stage ran): re-read and re-check, if open and not saving.
    pub fn invalidate(&mut self) -> bool {
        self.asked && self.reload()
    }

    pub fn land(&mut self, r: Reply) {
        self.saving = false;
        match r {
            Reply::Loaded(l) => {
                self.messages.clear();
                self.loaded = Some(l);
                self.reparse();
            }
            Reply::Refused(msgs) => {
                if let Some(Ok(l)) = &mut self.loaded {
                    l.doc.revert();
                }
                self.messages = msgs;
                self.reparse();
            }
        }
    }

    fn reparse(&mut self) {
        let Some(Ok(l)) = &self.loaded else { return };
        if let Ok(c) = l.doc.config() {
            self.config = c.mapping;
        }
        self.placed = place(&l.issues, &self.config);
    }

    pub fn loading(&self) -> bool {
        self.asked && self.loaded.is_none()
    }

    pub fn saving(&self) -> bool {
        self.saving
    }

    pub fn error(&self) -> Option<&str> {
        match &self.loaded {
            Some(Err(e)) => Some(e),
            _ => None,
        }
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    /// The last save was refused because porter.toml changed on disk: offer a reload.
    pub fn needs_reload(&self) -> bool {
        self.messages.iter().any(|m| m == CHANGED_OUTSIDE)
    }

    /// Applies an edit and returns the document to save, or None while a save is in
    /// flight or nothing is loaded. On refusal the edit is reverted (`land`).
    pub fn edit(&mut self, f: impl FnOnce(&mut MappingDoc)) -> Option<MappingDoc> {
        if self.saving {
            return None;
        }
        let Some(Ok(l)) = &mut self.loaded else {
            return None;
        };
        f(&mut l.doc);
        let doc = l.doc.clone();
        self.saving = true;
        self.reparse();
        Some(doc)
    }

    /// Shows a message where save refusals go (input the editor couldn't read).
    pub fn note(&mut self, msg: String) {
        self.messages = vec![msg];
    }

    /// The source statuses `ty` has rules for.
    pub fn status_keys(&self, ty: &str) -> Vec<String> {
        self.config
            .types
            .get(ty)
            .map(|t| t.status.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// `ty.field`'s transforms as mapped now.
    pub fn field_transforms(&self, ty: &str, field: &str) -> Vec<Transform> {
        self.config
            .types
            .get(ty)
            .and_then(|t| t.fields.get(field))
            .map(|f| f.transform.clone())
            .unwrap_or_default()
    }

    fn loaded(&self) -> Option<&Loaded> {
        match &self.loaded {
            Some(Ok(l)) => Some(l),
            _ => None,
        }
    }

    fn summary(&self) -> Option<&TypeSummary> {
        self.loaded().map(|l| &l.summary)
    }

    /// Issues that belong to no row (destination warnings and the like).
    pub fn general_issues(&self) -> &[RowIssue] {
        &self.placed.general
    }

    /// Every problem that sits on a row, in the order the page reads: each type, then its
    /// fields, then the blocks. Problems with no row (`general_issues`) are not here.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = vec![];
        let mut add = |target: Target, what: String, issues: &[RowIssue]| {
            out.extend(issues.iter().map(|i| Problem {
                target: target.clone(),
                what: what.clone(),
                why: i.message.clone(),
                error: i.error,
            }));
        };
        for t in self.type_rows() {
            add(Target::Type(t.name.clone()), t.name.clone(), &t.issues);
            for f in self.field_rows(&t.name) {
                let what = format!("{}.{}", t.name, f.path);
                add(Target::Field(t.name.clone(), f.path), what, &f.issues);
            }
        }
        for (i, b) in self.block_rows().into_iter().enumerate() {
            let what = b.module.clone().unwrap_or(b.kind.clone());
            add(Target::Block(i), what, &b.issues);
        }
        out
    }

    /// `counts()` once the mapping has been read, for the sidebar.
    pub fn problem_counts(&self) -> Option<(usize, usize)> {
        self.loaded().map(|_| self.counts())
    }

    /// (errors, warnings) over `problems()`: the numbers in the header.
    pub fn counts(&self) -> (usize, usize) {
        let p = self.problems();
        let e = p.iter().filter(|p| p.error).count();
        (e, p.len() - e)
    }

    /// The row the editor is on.
    pub fn selection(&self) -> Option<Target> {
        match (
            &self.selected_type,
            &self.selected_field,
            self.selected_block,
        ) {
            (_, _, Some(i)) => Some(Target::Block(i)),
            (Some(t), Some(f), None) => Some(Target::Field(t.clone(), f.clone())),
            (Some(t), None, None) => Some(Target::Type(t.clone())),
            _ => None,
        }
    }

    /// The first problem on a row after `after`, wrapping round; a row with several
    /// problems is one stop. With a single problem row it is that row again.
    pub fn next_problem(&self, after: Option<&Target>) -> Option<Problem> {
        let all = self.problems();
        let start = after
            .and_then(|a| all.iter().rposition(|p| &p.target == a))
            .map_or(0, |i| i + 1);
        all.iter()
            .cycle()
            .skip(start)
            .take(all.len())
            .find(|p| Some(&p.target) != after)
            .or(all.first())
            .cloned()
    }

    /// Every source type seen or mapped: seen ones first, in the store's order.
    pub fn type_rows(&self) -> Vec<TypeRow> {
        let seen = self.summary().map(|s| &s.types);
        let mut names: Vec<&String> = seen.map(|t| t.keys().collect()).unwrap_or_default();
        let only_mapped: Vec<&String> = self
            .config
            .types
            .keys()
            .filter(|k| !names.contains(k))
            .collect();
        names.extend(only_mapped);
        names
            .into_iter()
            .map(|name| {
                let count = seen.and_then(|t| t.get(name)).map_or(0, |s| s.count);
                let Some(tm) = self.config.types.get(name) else {
                    return TypeRow {
                        name: name.clone(),
                        count,
                        unmapped: true,
                        issues: self.placed.types.get(name).cloned().unwrap_or_default(),
                        ..TypeRow::default()
                    };
                };
                let kt = match tm.key_type {
                    porter_engine::mapping::KeyType::Text => "text",
                    porter_engine::mapping::KeyType::Number => "number",
                };
                TypeRow {
                    name: name.clone(),
                    count,
                    unmapped: false,
                    to: tm.to.clone(),
                    drop_reason: tm.drop_reason.clone(),
                    path: tm.path.clone(),
                    key: format!("{} ({kt})", tm.key_field),
                    status: tm
                        .status
                        .iter()
                        .map(|(a, b)| format!("{a} → {b}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    issues: self.placed.types.get(name).cloned().unwrap_or_default(),
                }
            })
            .collect()
    }

    /// The fields of `ty`: observed ones first, then ones only the mapping names.
    pub fn field_rows(&self, ty: &str) -> Vec<FieldRow> {
        let stats = self.summary().and_then(|s| s.types.get(ty));
        let mapped = self.config.types.get(ty).map(|t| &t.fields);
        let mut paths: Vec<&String> = stats.map(|s| s.fields.keys().collect()).unwrap_or_default();
        if let Some(m) = mapped {
            let only_mapped: Vec<&String> = m.keys().filter(|k| !paths.contains(k)).collect();
            paths.extend(only_mapped);
        }
        paths
            .into_iter()
            .map(|path| {
                let fs = stats.and_then(|s| s.field(path));
                let fm = mapped.and_then(|m| m.get(path));
                FieldRow {
                    path: path.clone(),
                    samples: fs
                        .map(|f| {
                            f.samples
                                .iter()
                                .map(|s| short(&clean_sample(s)))
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join(" · ")
                        })
                        .unwrap_or_default(),
                    fill: fs.map_or(String::new(), |f| format!("{:.0}%", f.fill_rate * 100.0)),
                    unmapped: fm.is_none(),
                    to: fm.map(|f| f.to.clone()).unwrap_or_default(),
                    transforms: fm.map(|f| f.transform.clone()).unwrap_or_default(),
                    drop: fm.is_some_and(|f| f.drop),
                    reason: fm.and_then(|f| f.reason.clone()),
                    issues: self
                        .placed
                        .fields
                        .get(&(ty.to_string(), path.clone()))
                        .cloned()
                        .unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Observed block kinds/modules with their counts and the entry that maps them, then
    /// mapping entries nothing matched.
    pub fn block_rows(&self) -> Vec<BlockRow> {
        let blocks = &self.config.blocks;
        let mut used = vec![false; blocks.len()];
        let mut rows: Vec<BlockRow> = vec![];
        for b in self
            .summary()
            .map(|s| s.blocks.as_slice())
            .unwrap_or_default()
        {
            let hit = blocks.iter().position(|m| {
                let (k, md) = (m.r#match.kind.as_deref(), m.r#match.module.as_deref());
                (k.is_some() || md.is_some())
                    && k.is_none_or(|k| k == b.kind)
                    && md.is_none_or(|md| Some(md) == b.module.as_deref())
            });
            if let Some(i) = hit {
                used[i] = true;
            }
            let m = match hit {
                Some(i) => blocks[i].r#match.clone(),
                None if b.module.is_some() => BlockMatch {
                    kind: None,
                    module: b.module.clone(),
                },
                None => BlockMatch {
                    kind: Some(b.kind.clone()),
                    module: None,
                },
            };
            rows.push(self.block_row(m, b.kind.clone(), b.module.clone(), b.count, hit));
        }
        for (i, m) in blocks.iter().enumerate().filter(|(i, _)| !used[*i]) {
            let r = &m.r#match;
            rows.push(self.block_row(
                r.clone(),
                r.kind.clone().unwrap_or_default(),
                r.module.clone(),
                0,
                Some(i),
            ));
        }
        rows
    }

    fn block_row(
        &self,
        m: BlockMatch,
        kind: String,
        module: Option<String>,
        count: usize,
        map: Option<usize>,
    ) -> BlockRow {
        let map = map.map(|i| &self.config.blocks[i]);
        let to = map.map(|b| b.to.clone());
        BlockRow {
            r#match: m,
            kind,
            module,
            count,
            fields: map.map(|b| pairs(&b.fields)).unwrap_or_default(),
            issues: to
                .as_ref()
                .and_then(|t| self.placed.blocks.get(t).cloned())
                .unwrap_or_default(),
            to,
        }
    }

    pub fn raw_block(&self) -> Option<&str> {
        self.config.raw_block.as_deref()
    }

    pub fn raw_block_issues(&self) -> Vec<RowIssue> {
        self.raw_block()
            .and_then(|r| self.placed.blocks.get(r).cloned())
            .unwrap_or_default()
    }

    fn schema(&self) -> Result<&DestSchema, String> {
        match self.loaded().map(|l| &l.dest) {
            Some(Ok(Some(d))) => Ok(d),
            Some(Err(e)) => Err(format!("destination schema unreadable: {e}")),
            _ => Err(NO_SCHEMA.into()),
        }
    }

    /// Destination collections for a type's `to`.
    pub fn collection_picker(&self) -> Picker {
        match self.schema() {
            Ok(d) => Picker::Choices(d.collections.keys().cloned().collect()),
            Err(why) => Picker::FreeText(why),
        }
    }

    /// Destination fields for `ty.field`: every path in the type's collection whose kind
    /// takes what the field holds after its transforms (all of them if never seen).
    pub fn field_picker(&self, ty: &str, field: &str) -> Picker {
        let d = match self.schema() {
            Ok(d) => d,
            Err(why) => return Picker::FreeText(why),
        };
        let tm = self.config.types.get(ty);
        let Some(coll) = tm.and_then(|t| t.to.as_deref()) else {
            return Picker::FreeText(format!("type `{ty}` has no destination collection"));
        };
        let Some(c) = d.collections.get(coll) else {
            return Picker::FreeText(format!(
                "collection `{coll}` is not in the destination schema"
            ));
        };
        let ts = tm
            .and_then(|t| t.fields.get(field))
            .map(|f| f.transform.as_slice())
            .unwrap_or_default();
        let variants = self
            .summary()
            .and_then(|s| s.types.get(ty))
            .and_then(|s| s.field(field))
            .map(|f| effective(&f.variants, ts))
            .unwrap_or_default();
        let mut out = vec![];
        paths(&c.fields, "", &mut out);
        out.retain(|p| {
            find(&c.fields, p).is_some_and(|f| variants.iter().all(|v| compatible(v, f)))
        });
        Picker::Choices(out)
    }

    /// Destination block slugs.
    pub fn block_picker(&self) -> Picker {
        match self.schema() {
            Ok(d) => Picker::Choices(d.blocks.keys().cloned().collect()),
            Err(why) => Picker::FreeText(why),
        }
    }
}

/// Every field path, dotted through groups and arrays.
fn paths(fields: &[DestField], prefix: &str, out: &mut Vec<String>) {
    for f in fields {
        let p = format!("{prefix}{}", f.name);
        out.push(p.clone());
        if matches!(f.kind.as_str(), "group" | "array") {
            paths(&f.fields, &format!("{p}."), out);
        }
    }
}

fn short(s: &str) -> String {
    const MAX: usize = 60;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    format!("{}…", s.chars().take(MAX).collect::<String>())
}

/// `a=b, c=d`.
pub fn pairs(m: &IndexMap<String, String>) -> String {
    m.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parses `a=b, c=d` (what `pairs` writes).
pub fn parse_pairs(s: &str) -> Result<IndexMap<String, String>, String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) if !k.trim().is_empty() && !v.trim().is_empty() => {
                Ok((k.trim().to_string(), v.trim().to_string()))
            }
            _ => Err(format!("`{p}` is not prop=field")),
        })
        .collect()
}

/// One transform as written in porter.toml: `trim`, or an inline table such as
/// `{ op = "regex", pattern = "^x", replace = "y" }`.
pub fn parse_transform(s: &str) -> Result<Transform, String> {
    let s = s.trim();
    let table = if s.starts_with('{') {
        s.to_string()
    } else {
        format!("{{ op = \"{s}\" }}")
    };
    #[derive(serde::Deserialize)]
    struct W {
        t: Transform,
    }
    toml::from_str::<W>(&format!("t = {table}"))
        .map(|w| w.t)
        .map_err(|e| e.message().to_string())
}

/// A transform as `parse_transform` reads it back.
pub fn transform_label(t: &Transform) -> String {
    let v = serde_json::to_value(t).unwrap_or_default();
    let op = v["op"].as_str().unwrap_or_default().to_string();
    let rest: Vec<String> = v
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| *k != "op")
        .map(|(k, v)| format!("{k} = {}", toml_inline(v)))
        .collect();
    if rest.is_empty() {
        op
    } else {
        format!("{{ op = \"{op}\", {} }}", rest.join(", "))
    }
}

fn toml_inline(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{s:?}"),
        Value::Object(m) => format!(
            "{{ {} }}",
            m.iter()
                .map(|(k, v)| format!("{k:?} = {}", toml_inline(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => other.to_string(),
    }
}

/// `ts` with item `i` moved one place up (or down); out-of-range moves do nothing.
pub fn move_transform(ts: &[Transform], i: usize, up: bool) -> Vec<Transform> {
    let mut out = ts.to_vec();
    let j = if up { i.checked_sub(1) } else { Some(i + 1) };
    if let Some(j) = j.filter(|&j| j < out.len() && i < out.len()) {
        out.swap(i, j);
    }
    out
}

/// Issues by the row that causes them; anything else is general.
#[derive(Default)]
struct Placed {
    types: IndexMap<String, Vec<RowIssue>>,
    fields: IndexMap<(String, String), Vec<RowIssue>>,
    /// By destination block slug.
    blocks: IndexMap<String, Vec<RowIssue>>,
    general: Vec<RowIssue>,
}

fn place(issues: &[PendingIssue], m: &MappingConfig) -> Placed {
    let mut p = Placed::default();
    for i in issues {
        let ri = RowIssue {
            error: i.severity == Severity::Error,
            message: i.message.clone(),
        };
        let s = |k: &str| i.details.get(k).and_then(Value::as_str);
        let block_msg = i.message.starts_with("block `");
        if let Some(ty) = s("type") {
            match s("source").or(s("field")) {
                Some(f) => p.fields.entry((ty.into(), f.into())).or_default().push(ri),
                None => p.types.entry(ty.into()).or_default().push(ri),
            }
            continue;
        }
        if let Some(b) = s("block").or(s("owner").filter(|_| block_msg)) {
            p.blocks.entry(b.into()).or_default().push(ri);
            continue;
        }
        // A collection's own problem (key field, required fields): on every type that
        // maps to it.
        if let Some(c) = s("collection").or(s("owner")) {
            let tys: Vec<&String> = m
                .types
                .iter()
                .filter(|(_, t)| t.to.as_deref() == Some(c))
                .map(|(k, _)| k)
                .collect();
            if !tys.is_empty() {
                for ty in tys {
                    p.types.entry(ty.clone()).or_default().push(ri.clone());
                }
                continue;
            }
        }
        p.general.push(ri);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_engine::mapping::summary::{BlockStats, FieldStats, TypeStats};
    use porter_model::IssueKind;
    use serde_json::json;

    const TOML: &str = r#"# hand notes
[project]
name = "demo"

[destination]
adapter = "payload"

[mapping]
raw_block = "rawHtml"

[mapping.types.post]
to = "pages"   # renamed
path = "/{slug}/"

[mapping.types.post.fields.title]
to = "title"

[mapping.types.post.fields.legacy]
drop = true
reason = "unused"

[[mapping.blocks]]
match = { module = "hero_banner" }
to = "hero"
fields = { title = "heading" }
"#;

    fn stats(variants: &[&str], fill: f64, samples: &[&str]) -> FieldStats {
        FieldStats {
            variants: variants.iter().map(|s| s.to_string()).collect(),
            fill_rate: fill,
            samples: samples.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn summary() -> TypeSummary {
        let mut post = TypeStats {
            count: 12,
            ..Default::default()
        };
        post.fields.insert(
            "title".into(),
            stats(&["Text"], 1.0, &["<b>Hello</b> world"]),
        );
        post.fields
            .insert("price".into(), stats(&["Text"], 0.5, &["12"]));
        let page = TypeStats {
            count: 3,
            ..Default::default()
        };
        TypeSummary {
            types: [("post".to_string(), post), ("page".to_string(), page)].into(),
            blocks: vec![
                BlockStats {
                    kind: "builder".into(),
                    module: Some("hero_banner".into()),
                    count: 7,
                    ..Default::default()
                },
                BlockStats {
                    kind: "gallery".into(),
                    module: None,
                    count: 2,
                    ..Default::default()
                },
            ],
        }
    }

    fn field(name: &str, kind: &str) -> DestField {
        DestField {
            name: name.into(),
            kind: kind.into(),
            ..Default::default()
        }
    }

    fn dest() -> DestSchema {
        let mut d = DestSchema::default();
        d.collections.insert(
            "pages".into(),
            porter_engine::destinations::payload::ts::CollectionSchema {
                fields: vec![
                    field("title", "text"),
                    field("price", "number"),
                    DestField {
                        name: "meta".into(),
                        kind: "group".into(),
                        fields: vec![field("subtitle", "textarea"), field("when", "date")],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
        );
        d.blocks.insert("hero".into(), vec![]);
        d
    }

    fn issue(sev: Severity, msg: &str, details: Value) -> PendingIssue {
        PendingIssue {
            kind: IssueKind::SchemaMismatch,
            severity: sev,
            message: msg.into(),
            details,
        }
    }

    fn loaded(dir: &std::path::Path, dest: Result<Option<DestSchema>, String>) -> Mapping {
        let p = dir.join("porter.toml");
        std::fs::write(&p, TOML).unwrap();
        let mut m = Mapping::default();
        assert!(m.open());
        assert!(!m.open(), "opens once");
        assert!(m.loading());
        m.land(Reply::Loaded(Ok(Box::new(Loaded {
            doc: MappingDoc::open(&p).unwrap(),
            summary: summary(),
            dest,
            issues: vec![
                issue(
                    Severity::Error,
                    "field `pages.subtitle` (mapped from `post.title`) does not exist",
                    json!({"collection": "pages", "field": "subtitle", "source": "title", "type": "post"}),
                ),
                issue(
                    Severity::Warning,
                    "`post.price` has no mapping (filled in 50% of post)",
                    json!({"type": "post", "field": "price"}),
                ),
                issue(
                    Severity::Warning,
                    "source type `page` has no mapping",
                    json!({"type": "page"}),
                ),
                issue(
                    Severity::Error,
                    "collection `pages` has no `legacyId` field",
                    json!({"collection": "pages", "field": "legacyId", "key_type": "text"}),
                ),
                issue(
                    Severity::Error,
                    "block `hero` field `cta` is required but no [[mapping.blocks]] field maps to it",
                    json!({"owner": "hero", "field": "cta"}),
                ),
                issue(
                    Severity::Error,
                    "block `rawHtml` (mapping.raw_block) is not defined",
                    json!({"block": "rawHtml"}),
                ),
                issue(Severity::Warning, "could not read x.ts", json!({})),
            ],
        }))));
        m
    }

    #[test]
    fn rows_carry_their_issues() {
        let d = tempfile::tempdir().unwrap();
        let m = loaded(d.path(), Ok(Some(dest())));
        let types = m.type_rows();
        let names: Vec<_> = types.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["post", "page"]);
        let post = &types[0];
        assert_eq!(post.count, 12);
        assert_eq!(post.to.as_deref(), Some("pages"));
        assert_eq!(post.key, "legacyId (text)");
        // The collection's key-field problem sits on the type mapped to it.
        assert_eq!(post.issues.len(), 1);
        assert!(post.issues[0].error && post.issues[0].message.contains("legacyId"));
        assert!(types[1].unmapped);
        assert_eq!(
            types[1].issues[0].message,
            "source type `page` has no mapping"
        );

        let fields = m.field_rows("post");
        let names: Vec<_> = fields.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["title", "price", "legacy"]);
        assert_eq!(fields[0].samples, "Hello world", "tags stripped");
        assert_eq!(fields[0].fill, "100%");
        assert!(fields[0].issues[0].message.contains("does not exist"));
        assert!(fields[1].unmapped && !fields[1].issues[0].error);
        assert!(fields[2].drop && fields[2].reason.as_deref() == Some("unused"));
        assert_eq!(fields[2].fill, "", "never seen");

        let blocks = m.block_rows();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].to.as_deref(), Some("hero"));
        assert_eq!(blocks[0].count, 7);
        assert_eq!(blocks[0].fields, "title=heading");
        assert!(blocks[0].issues[0].message.contains("`cta`"));
        assert_eq!(blocks[1].to, None);
        assert_eq!(blocks[1].r#match.kind.as_deref(), Some("gallery"));
        assert!(m.raw_block_issues()[0].message.contains("rawHtml"));
        assert_eq!(m.general_issues().len(), 1);
    }

    #[test]
    fn problems_name_the_row_and_say_why() {
        let d = tempfile::tempdir().unwrap();
        let m = loaded(d.path(), Ok(Some(dest())));
        let p = m.problems();
        let rows: Vec<_> = p.iter().map(|p| (p.what.as_str(), p.error)).collect();
        assert_eq!(
            rows,
            [
                ("post", true),
                ("post.title", true),
                ("post.price", false),
                ("page", false),
                ("hero_banner", true),
            ]
        );
        assert!(p[1].why.contains("does not exist"));
        assert_eq!(p[1].target, Target::Field("post".into(), "title".into()));
        assert_eq!(p[4].target, Target::Block(0));
        assert_eq!(m.counts(), (3, 2));
    }

    #[test]
    fn next_problem_walks_the_rows_and_wraps() {
        let d = tempfile::tempdir().unwrap();
        let mut m = loaded(d.path(), Ok(Some(dest())));
        let first = m.next_problem(None).unwrap();
        assert_eq!(first.target, Target::Type("post".into()));
        let second = m.next_problem(Some(&first.target)).unwrap();
        assert_eq!(second.what, "post.title");
        let last = Target::Block(0);
        assert_eq!(m.next_problem(Some(&last)).unwrap().what, "post", "wraps");
        // The editor's own row is where the walk continues from.
        m.selected_type = Some("post".into());
        m.selected_field = Some("price".into());
        assert_eq!(
            m.selection(),
            Some(Target::Field("post".into(), "price".into()))
        );
        assert_eq!(m.next_problem(m.selection().as_ref()).unwrap().what, "page");
        m.selected_block = Some(0);
        assert_eq!(m.selection(), Some(Target::Block(0)));
    }

    #[test]
    fn a_row_is_as_bad_as_its_worst_issue() {
        let m = |error| RowIssue {
            error,
            message: "x".into(),
        };
        assert_eq!(level(&[]), None);
        assert_eq!(level(&[m(false)]), Some(Severity::Warning));
        assert_eq!(level(&[m(false), m(true)]), Some(Severity::Error));
    }

    #[test]
    fn no_problems_means_no_next() {
        let m = Mapping::default();
        assert_eq!(m.counts(), (0, 0));
        assert_eq!(m.next_problem(None), None);
        assert_eq!(m.selection(), None);
    }

    #[test]
    fn pickers_from_schema_or_free_text() {
        let d = tempfile::tempdir().unwrap();
        let m = loaded(d.path(), Ok(Some(dest())));
        assert_eq!(m.collection_picker(), Picker::Choices(vec!["pages".into()]));
        // Text goes to text-like fields only, through groups.
        assert_eq!(
            m.field_picker("post", "title"),
            Picker::Choices(vec!["title".into(), "meta.subtitle".into()])
        );
        assert_eq!(m.block_picker(), Picker::Choices(vec!["hero".into()]));
        assert!(matches!(m.field_picker("page", "x"), Picker::FreeText(_)));

        let d2 = tempfile::tempdir().unwrap();
        let none = loaded(d2.path(), Ok(None));
        assert_eq!(none.collection_picker(), Picker::FreeText(NO_SCHEMA.into()));
        assert_eq!(
            none.field_picker("post", "title"),
            Picker::FreeText(NO_SCHEMA.into())
        );
        let d3 = tempfile::tempdir().unwrap();
        let bad = loaded(d3.path(), Err("x is not a directory".into()));
        assert_eq!(
            bad.block_picker(),
            Picker::FreeText("destination schema unreadable: x is not a directory".into())
        );
    }

    /// The editor's save path: an edit produces the expected TOML with comments kept;
    /// a refusal reverts it and says why; a change on disk asks for a reload.
    #[test]
    fn edit_save_and_refusal() {
        let d = tempfile::tempdir().unwrap();
        let mut m = loaded(d.path(), Ok(None));
        let mut doc = m.edit(|d| d.set_type_to("post", Some("articles"))).unwrap();
        assert!(m.saving());
        assert!(
            m.edit(|d| d.set_path("post", None)).is_none(),
            "one save at a time"
        );
        assert_eq!(m.type_rows()[0].to.as_deref(), Some("articles"));
        assert_eq!(doc.save(), Vec::<String>::new());
        let text = std::fs::read_to_string(d.path().join("porter.toml")).unwrap();
        assert!(text.contains("to = \"articles\"   # renamed"), "{text}");
        assert!(text.starts_with("# hand notes\n"));
        m.land(Reply::Loaded(Ok(Box::new(Loaded {
            doc,
            summary: summary(),
            dest: Ok(None),
            issues: vec![],
        }))));
        assert!(!m.saving());

        // Refused: the edit is undone in the editor and the reason shown.
        m.edit(|d| d.drop_type("post", Some("gone"))).unwrap();
        assert_eq!(m.type_rows()[0].to, None);
        m.land(Reply::Refused(vec!["mapping.types.post: bad".into()]));
        assert_eq!(m.type_rows()[0].to.as_deref(), Some("articles"));
        assert_eq!(m.messages(), ["mapping.types.post: bad"]);
        assert!(!m.needs_reload());
        m.edit(|d| d.set_path("post", None)).unwrap();
        m.land(Reply::Refused(vec![CHANGED_OUTSIDE.into()]));
        assert!(m.needs_reload());
        assert!(m.reload());
        assert!(m.messages().is_empty());
    }

    #[test]
    fn transforms_list_editing() {
        let ts = vec![
            parse_transform("trim").unwrap(),
            parse_transform(r#"{ op = "regex", pattern = "^x", replace = "y" }"#).unwrap(),
            parse_transform(r#"{ op = "lookup", table = { a = "b" } }"#).unwrap(),
        ];
        for t in &ts {
            assert_eq!(&parse_transform(&transform_label(t)).unwrap(), t);
        }
        assert_eq!(transform_label(&ts[0]), "trim");
        let moved = move_transform(&ts, 1, true);
        assert_eq!(moved[0], ts[1]);
        assert_eq!(move_transform(&ts, 0, true), ts);
        assert_eq!(move_transform(&ts, 2, false), ts);
        let e = parse_transform("frobnicate").unwrap_err();
        assert!(e.contains("frobnicate"), "{e}");
        assert!(parse_transform(r#"{ op = "regex", patern = "a", replace = "b" }"#).is_err());
        assert_eq!(
            parse_pairs("title=heading, body = text").unwrap(),
            IndexMap::from([
                ("title".to_string(), "heading".to_string()),
                ("body".to_string(), "text".to_string())
            ])
        );
        assert!(parse_pairs("title").is_err());
        assert!(parse_pairs("").unwrap().is_empty());
    }
}
