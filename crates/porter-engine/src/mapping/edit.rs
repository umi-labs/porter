//! Structured edits to `[mapping]` in a hand-edited porter.toml, through `toml_edit`: every
//! byte outside the edited value survives, comments and spacing included. Saves are
//! atomic, re-validated first, and refused when the file changed on disk since it was
//! read, so a hand edit is never clobbered. Platform-free.

use super::{BlockMatch, FieldMap, KeyType, Transform};
use crate::config::ProjectConfig;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

/// What `save` says when porter.toml changed on disk since it was opened.
pub const CHANGED_OUTSIDE: &str = "porter.toml changed outside the editor — reload";

/// A mapped (not dropped) field: its destination path and transforms, in order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldMapEdit {
    pub to: String,
    pub transform: Vec<Transform>,
}

/// A `[[mapping.blocks]]` entry's destination block and its prop -> field map.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockEdit {
    pub to: String,
    pub fields: IndexMap<String, String>,
}

#[derive(Clone)]
pub struct MappingDoc {
    path: PathBuf,
    /// The file's text as last read or written: what must still be on disk to save.
    base: String,
    doc: DocumentMut,
}

fn parse(text: &str) -> Result<ProjectConfig> {
    toml::from_str(text).context("parsing porter.toml")
}

impl MappingDoc {
    /// Reads porter.toml; refuses a file that doesn't parse as a project config (fix it by
    /// hand first), so every table the setters reach is known to be a table.
    pub fn open(path: &Path) -> Result<MappingDoc> {
        let base =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        parse(&base)?;
        let doc = base.parse().context("parsing porter.toml")?;
        Ok(MappingDoc {
            path: path.to_path_buf(),
            base,
            doc,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The text as edited so far.
    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// The config as edited so far.
    pub fn config(&self) -> Result<ProjectConfig> {
        parse(&self.text())
    }

    /// Drops unsaved edits: back to the text last read or written.
    pub fn revert(&mut self) {
        if let Ok(d) = self.base.parse() {
            self.doc = d;
        }
    }

    fn mapping(&mut self) -> &mut dyn TableLike {
        sub(self.doc.as_table_mut(), "mapping")
    }

    fn type_table(&mut self, ty: &str) -> &mut dyn TableLike {
        sub(sub(self.mapping(), "types"), ty)
    }

    fn field_table(&mut self, ty: &str, field: &str) -> &mut dyn TableLike {
        sub(sub(self.type_table(ty), "fields"), field)
    }

    /// Sets (and un-drops) or removes a type's destination collection.
    pub fn set_type_to(&mut self, ty: &str, to: Option<&str>) {
        let t = self.type_table(ty);
        if to.is_some() {
            t.remove("drop_reason");
        }
        set_str(t, "to", to);
    }

    /// Drops a type: no `to`; a reason given replaces the old one, none keeps it.
    pub fn drop_type(&mut self, ty: &str, reason: Option<&str>) {
        let t = self.type_table(ty);
        t.remove("to");
        if reason.is_some() {
            set_str(t, "drop_reason", reason);
        }
    }

    pub fn set_path(&mut self, ty: &str, path: Option<&str>) {
        set_str(self.type_table(ty), "path", path);
    }

    pub fn set_key(&mut self, ty: &str, field: &str, key_type: KeyType) {
        let t = self.type_table(ty);
        set_str(t, "key_field", Some(field));
        let kt = match key_type {
            KeyType::Text => "text",
            KeyType::Number => "number",
        };
        set_str(t, "key_type", Some(kt));
    }

    /// One status rule: source status -> destination status (`"skip"` leaves them out);
    /// None removes the rule.
    pub fn set_status(&mut self, ty: &str, status: &str, to: Option<&str>) {
        set_str(sub(self.type_table(ty), "status"), status, to);
    }

    /// Maps a field: sets `to` and the transforms, clearing any drop. An unchanged
    /// transform list is left exactly as written.
    pub fn set_field(&mut self, ty: &str, field: &str, edit: FieldMapEdit) {
        let same = self
            .current_field(ty, field)
            .is_some_and(|f| f.transform == edit.transform);
        let t = self.field_table(ty, field);
        t.remove("drop");
        t.remove("reason");
        set_str(t, "to", Some(&edit.to));
        if same {
            return;
        }
        match transforms(&edit.transform) {
            Some(v) => set_value(t, "transform", v),
            None => {
                t.remove("transform");
            }
        }
    }

    /// Leaves a field behind on purpose. A reason given replaces the old one; none keeps
    /// whatever reason is written.
    pub fn drop_field(&mut self, ty: &str, field: &str, reason: Option<&str>) {
        let t = self.field_table(ty, field);
        t.remove("to");
        t.remove("transform");
        set_value(t, "drop", true.into());
        if reason.is_some() {
            set_str(t, "reason", reason);
        }
    }

    /// Back to unmapped: the field's entry goes.
    pub fn remove_field(&mut self, ty: &str, field: &str) {
        sub(self.type_table(ty), "fields").remove(field);
    }

    fn current_field(&self, ty: &str, field: &str) -> Option<FieldMap> {
        self.config()
            .ok()?
            .mapping
            .types
            .get(ty)?
            .fields
            .get(field)
            .cloned()
    }

    /// Updates the `[[mapping.blocks]]` entry whose `match` is `m`, or appends one;
    /// None removes it.
    pub fn set_block(&mut self, m: &BlockMatch, edit: Option<BlockEdit>) {
        let current = self.config().ok().map(|c| c.mapping.blocks);
        let at = current
            .as_ref()
            .and_then(|bs| bs.iter().position(|b| &b.r#match == m));
        let same_fields = match (at, &current, &edit) {
            (Some(i), Some(bs), Some(e)) => bs[i].fields == e.fields,
            _ => false,
        };
        let mapping = self.mapping();
        let Some(edit) = edit else {
            if let Some(i) = at {
                match mapping.get_mut("blocks") {
                    Some(Item::ArrayOfTables(a)) => {
                        a.remove(i);
                    }
                    Some(Item::Value(Value::Array(a))) => {
                        a.remove(i);
                    }
                    _ => {}
                }
            }
            return;
        };
        let entry: Option<&mut dyn TableLike> = match (at, mapping.get_mut("blocks")) {
            (Some(i), Some(Item::ArrayOfTables(a))) => {
                a.get_mut(i).map(|t| t as &mut dyn TableLike)
            }
            (Some(i), Some(Item::Value(Value::Array(a)))) => a
                .get_mut(i)
                .and_then(Value::as_inline_table_mut)
                .map(|t| t as &mut dyn TableLike),
            _ => None,
        };
        if let Some(t) = entry {
            set_str(t, "to", Some(&edit.to));
            if !same_fields {
                set_fields(t, &edit.fields);
            }
            return;
        }
        let mut t = Table::new();
        let mut mt = toml_edit::InlineTable::new();
        if let Some(k) = &m.kind {
            mt.insert("kind", k.as_str().into());
        }
        if let Some(md) = &m.module {
            mt.insert("module", md.as_str().into());
        }
        t.insert("match", toml_edit::value(mt));
        t.insert("to", toml_edit::value(edit.to.as_str()));
        set_fields(&mut t, &edit.fields);
        match mapping.get_mut("blocks") {
            Some(Item::ArrayOfTables(a)) => a.push(t),
            Some(Item::Value(Value::Array(a))) => a.push(t.into_inline_table()),
            _ => {
                let mut a = toml_edit::ArrayOfTables::new();
                a.push(t);
                mapping.insert("blocks", Item::ArrayOfTables(a));
            }
        }
    }

    /// The block slug for content with no native shape; None removes it.
    pub fn set_raw_block(&mut self, slug: Option<&str>) {
        set_str(self.mapping(), "raw_block", slug);
    }

    /// Writes the edits. Empty means saved (or nothing to save); otherwise nothing was
    /// written and the messages say why: the file changed on disk since it was read, the
    /// edited text doesn't parse, or the edit makes the config invalid (problems the file
    /// already had elsewhere don't block mapping edits).
    pub fn save(&mut self) -> Vec<String> {
        let disk = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) => return vec![format!("reading {}: {e}", self.path.display())],
        };
        if disk != self.base {
            return vec![CHANGED_OUTSIDE.into()];
        }
        let text = self.text();
        if text == disk {
            return vec![];
        }
        let after = match parse(&text) {
            Ok(c) => c.validate(),
            Err(e) => return vec![format!("{e:#}")],
        };
        let before = parse(&disk).map(|c| c.validate()).unwrap_or_default();
        let new: Vec<String> = after.into_iter().filter(|m| !before.contains(m)).collect();
        if !new.is_empty() {
            return new;
        }
        if let Err(e) = write_atomic(&self.path, &text) {
            return vec![format!("{e:#}")];
        }
        self.base = text;
        vec![]
    }
}

/// Temp file in the same directory, then rename over: a reader sees the old file or the
/// new one, never half. The file's permissions are kept.
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let mut tmp = tempfile::NamedTempFile::new_in(dir).context("temp file")?;
    std::io::Write::write_all(&mut tmp, text.as_bytes())?;
    if let Ok(m) = std::fs::metadata(path) {
        tmp.as_file().set_permissions(m.permissions())?;
    }
    // On disk before the rename, so a crash can't leave an empty or partial file.
    tmp.as_file().sync_all().context("sync temp file")?;
    tmp.persist(path)
        .with_context(|| format!("writing {}", path.display()))?;
    // The rename itself, durable too (best-effort: not every filesystem allows it).
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// The table under `key`, created (as an implicit header) when absent. `MappingDoc::open`
/// checked the file's shape, so an existing value here is a table; anything else would
/// be replaced by one.
fn sub<'a>(parent: &'a mut dyn TableLike, key: &str) -> &'a mut dyn TableLike {
    let item = parent.entry(key).or_insert(Item::None);
    if item.as_table_like().is_none() {
        let mut t = Table::new();
        t.set_implicit(true);
        *item = Item::Table(t);
    }
    item.as_table_like_mut()
        .unwrap_or_else(|| unreachable!("just made a table"))
}

/// Replaces a value keeping its surrounding whitespace and trailing comment, so a
/// one-value edit changes nothing else on its line.
fn set_value(t: &mut dyn TableLike, key: &str, v: Value) {
    match t.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) if same(old, &v) => {}
        Some(old) => {
            let decor = old.decor().clone();
            *old = v;
            *old.decor_mut() = decor;
        }
        None => {
            t.insert(key, Item::Value(v));
        }
    }
}

/// Equal as TOML values, however each is written (`'a'` and `"a"` are the same).
fn same(a: &Value, b: &Value) -> bool {
    let plain = |v: &Value| {
        let mut v = v.clone();
        v.decor_mut().clear();
        format!("x = {v}").parse::<toml::Table>().ok()
    };
    plain(a).is_some_and(|x| Some(x) == plain(b))
}

fn set_str(t: &mut dyn TableLike, key: &str, v: Option<&str>) {
    match v {
        Some(v) => set_value(t, key, v.into()),
        None => {
            t.remove(key);
        }
    }
}

fn set_fields(t: &mut dyn TableLike, fields: &IndexMap<String, String>) {
    if fields.is_empty() {
        t.remove("fields");
        return;
    }
    let mut it = toml_edit::InlineTable::new();
    for (k, v) in fields {
        it.insert(k, v.as_str().into());
    }
    set_value(t, "fields", Value::InlineTable(it));
}

/// One inline table, or an array of them.
fn transforms(ts: &[Transform]) -> Option<Value> {
    let one = |t: &Transform| -> Option<Value> {
        toml::Value::try_from(t).ok()?.to_string().parse().ok()
    };
    match ts {
        [] => None,
        [t] => one(t),
        ts => Some(Value::Array(ts.iter().filter_map(one).collect())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{BlockMatch, KeyType, Transform};

    /// A hand-edited porter.toml: comments everywhere, odd spacing, inline tables.
    const FIXTURE: &str = r#"# Client site: hand-edited, keep my notes.
[project]
name = "demo"   # the folder name

[source]
adapter = "wordpress"

[destination]
adapter = "payload"
url = "https://cms.example.test"
api_key_env = "PORTER_PAYLOAD_API_KEY"

[mapping]
raw_block = "rawHtml"

# Posts become pages.
[mapping.types.post]
to   = "pages"        # renamed in 2025
path = "/{slug}/"

[mapping.types.post.fields.title]
to = "title"   # keep

[mapping.types.post.fields."meta.subtitle"]
to = "subtitle"
transform = [{ op = "trim" }]

# Members live in the CRM.
[mapping.types.member]
drop_reason = "CRM"

[[mapping.blocks]]
match = { module = "hero_banner" }   # the big one
to = "hero"
fields = { title = "heading" }
"#;

    fn fixture(text: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("porter.toml");
        std::fs::write(&p, text).unwrap();
        (d, p)
    }

    fn changed_lines(a: &str, b: &str) -> Vec<(String, String)> {
        assert_eq!(a.lines().count(), b.lines().count(), "line count:\n{b}");
        a.lines()
            .zip(b.lines())
            .filter(|(x, y)| x != y)
            .map(|(x, y)| (x.to_string(), y.to_string()))
            .collect()
    }

    #[test]
    fn edit_preserves_other_lines() {
        let (_d, p) = fixture(FIXTURE);
        let mut doc = MappingDoc::open(&p).unwrap();
        doc.set_field(
            "post",
            "title",
            FieldMapEdit {
                to: "heading".into(),
                transform: vec![],
            },
        );
        assert_eq!(doc.save(), Vec::<String>::new());
        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            changed_lines(FIXTURE, &after),
            [(
                "to = \"title\"   # keep".to_string(),
                "to = \"heading\"   # keep".to_string()
            )]
        );
        // A type-level edit: one line too, its comment and odd spacing kept.
        doc.set_type_to("post", Some("articles"));
        assert_eq!(doc.save(), Vec::<String>::new());
        let again = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            changed_lines(&after, &again),
            [(
                "to   = \"pages\"        # renamed in 2025".to_string(),
                "to   = \"articles\"        # renamed in 2025".to_string()
            )]
        );
        // A block's destination, in an array of tables with inline `match`.
        doc.set_block(
            &BlockMatch {
                kind: None,
                module: Some("hero_banner".into()),
            },
            Some(BlockEdit {
                to: "banner".into(),
                fields: [("title".to_string(), "heading".to_string())].into(),
            }),
        );
        assert_eq!(doc.save(), Vec::<String>::new());
        let third = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            changed_lines(&again, &third),
            [("to = \"hero\"".to_string(), "to = \"banner\"".to_string())]
        );
        // An unchanged transform list is left exactly as written.
        doc.set_field(
            "post",
            "meta.subtitle",
            FieldMapEdit {
                to: "subtitle".into(),
                transform: vec![Transform::Trim],
            },
        );
        assert_eq!(doc.save(), Vec::<String>::new());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), third);
    }

    #[test]
    fn new_entries_parse_back() {
        let (_d, p) = fixture(FIXTURE);
        let mut doc = MappingDoc::open(&p).unwrap();
        doc.set_field(
            "event",
            "cap",
            FieldMapEdit {
                to: "capacity".into(),
                transform: vec![
                    Transform::Trim,
                    Transform::Regex {
                        pattern: "^x".into(),
                        replace: "y".into(),
                    },
                ],
            },
        );
        doc.set_type_to("event", Some("events"));
        doc.set_key("event", "legacyId", KeyType::Number);
        doc.set_status("event", "private", Some("draft"));
        doc.set_path("event", Some("/events/{slug}/"));
        doc.set_block(
            &BlockMatch {
                kind: Some("gallery".into()),
                module: None,
            },
            Some(BlockEdit {
                to: "gallery".into(),
                fields: Default::default(),
            }),
        );
        assert_eq!(doc.save(), Vec::<String>::new());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("# Client site: hand-edited, keep my notes.\n"));
        let m = crate::config::ProjectConfig::load(&p).unwrap().mapping;
        let ev = &m.types["event"];
        assert_eq!(ev.to.as_deref(), Some("events"));
        assert_eq!(ev.key_type, KeyType::Number);
        assert_eq!(ev.status["private"], "draft");
        assert_eq!(ev.path.as_deref(), Some("/events/{slug}/"));
        assert_eq!(ev.fields["cap"].transform.len(), 2);
        assert_eq!(m.blocks.len(), 2);
        assert_eq!(m.blocks[1].to, "gallery");
        // Removing a block and a type's `to`.
        doc.set_block(&m.blocks[1].r#match, None);
        doc.set_type_to("event", None);
        assert_eq!(doc.save(), Vec::<String>::new());
        let m = crate::config::ProjectConfig::load(&p).unwrap().mapping;
        assert_eq!(m.blocks.len(), 1);
        assert_eq!(m.types["event"].to, None);
    }

    #[test]
    fn drop_requires_reason_optional_but_kept() {
        let (_d, p) = fixture(FIXTURE);
        let mut doc = MappingDoc::open(&p).unwrap();
        // No reason: still valid, and saved.
        doc.drop_field("post", "title", None);
        assert_eq!(doc.save(), Vec::<String>::new());
        let f = |p: &Path| {
            crate::config::ProjectConfig::load(p).unwrap().mapping.types["post"].fields["title"]
                .clone()
        };
        let got = f(&p);
        assert!(got.drop && got.to.is_empty() && got.reason.is_none());
        // A reason given is written...
        doc.drop_field("post", "title", Some("legacy"));
        assert_eq!(doc.save(), Vec::<String>::new());
        assert_eq!(f(&p).reason.as_deref(), Some("legacy"));
        // ...and kept by a later drop that gives none.
        doc.drop_field("post", "title", None);
        assert_eq!(doc.save(), Vec::<String>::new());
        assert_eq!(f(&p).reason.as_deref(), Some("legacy"));
        // Mapping it again clears drop and reason; dropping the transform list too.
        doc.drop_field("post", "meta.subtitle", Some("unused"));
        doc.set_field(
            "post",
            "title",
            FieldMapEdit {
                to: "title".into(),
                transform: vec![],
            },
        );
        assert_eq!(doc.save(), Vec::<String>::new());
        let got = f(&p);
        assert!(!got.drop && got.reason.is_none() && got.to == "title");
        let sub = &crate::config::ProjectConfig::load(&p)
            .unwrap()
            .mapping
            .types["post"]
            .fields["meta.subtitle"];
        assert!(sub.drop && sub.transform.is_empty());
        // A type dropped with a reason loses its `to`.
        doc.drop_type("post", Some("retired"));
        assert_eq!(doc.save(), Vec::<String>::new());
        let t = &crate::config::ProjectConfig::load(&p)
            .unwrap()
            .mapping
            .types["post"];
        assert_eq!(
            (t.to.as_deref(), t.drop_reason.as_deref()),
            (None, Some("retired"))
        );
    }

    #[test]
    fn save_is_atomic() {
        let (d, p) = fixture(FIXTURE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let mut doc = MappingDoc::open(&p).unwrap();
        doc.set_path("post", Some("/news/{slug}/"));
        assert_eq!(doc.save(), Vec::<String>::new());
        // The whole new text, in one file, and no temp file left beside it.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), doc.text());
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["porter.toml"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "permissions kept");
        }
        // Nothing to write: the file is not touched.
        let before = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert_eq!(doc.save(), Vec::<String>::new());
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn validation_messages_returned() {
        let (_d, p) = fixture(FIXTURE);
        let mut doc = MappingDoc::open(&p).unwrap();
        doc.set_field(
            "post",
            "title",
            FieldMapEdit {
                to: "title".into(),
                transform: vec![Transform::Regex {
                    pattern: "(".into(),
                    replace: String::new(),
                }],
            },
        );
        let msgs = doc.save();
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert!(
            msgs[0].starts_with("mapping.types.post.fields.title.transform[0].pattern"),
            "{msgs:?}"
        );
        // Not saved; revert goes back to what is on disk.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), FIXTURE);
        doc.revert();
        assert_eq!(doc.text(), FIXTURE);
        // A problem the file already had elsewhere doesn't block mapping edits.
        let partial = FIXTURE.replace("url = \"https://cms.example.test\"\n", "");
        let (_d2, p2) = fixture(&partial);
        let mut doc = MappingDoc::open(&p2).unwrap();
        doc.set_path("post", Some("/x/{slug}/"));
        assert_eq!(doc.save(), Vec::<String>::new());
        assert!(std::fs::read_to_string(&p2).unwrap().contains("/x/{slug}/"));
    }

    #[test]
    fn external_edit_refused() {
        let (_d, p) = fixture(FIXTURE);
        let mut doc = MappingDoc::open(&p).unwrap();
        let hand = format!("{FIXTURE}\n# Isaac was here\n");
        std::fs::write(&p, &hand).unwrap();
        doc.set_path("post", Some("/y/{slug}/"));
        assert_eq!(doc.save(), [CHANGED_OUTSIDE.to_string()]);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), hand);
        // Reloading picks up the hand edit; edits then save on top of it.
        let mut doc = MappingDoc::open(&p).unwrap();
        doc.set_path("post", Some("/y/{slug}/"));
        assert_eq!(doc.save(), Vec::<String>::new());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# Isaac was here") && text.contains("/y/{slug}/"));
    }

    /// Deletions take their own lines (and the comment that leads them) and nothing else:
    /// the next table's comment stays.
    #[test]
    fn deletions_remove_only_their_own_lines() {
        let commented = FIXTURE.replace(
            "\n[mapping.types.post.fields.\"meta.subtitle\"]",
            "\n# Subtitles are trimmed.\n[mapping.types.post.fields.\"meta.subtitle\"]",
        );
        let text = format!(
            "{commented}\n# Galleries too.\n[[mapping.blocks]]\nmatch = {{ kind = \"gallery\" }}\nto = \"gallery\"\n"
        );
        let save = |f: &dyn Fn(&mut MappingDoc)| {
            let (_d, p) = fixture(&text);
            let mut doc = MappingDoc::open(&p).unwrap();
            f(&mut doc);
            assert_eq!(doc.save(), Vec::<String>::new());
            std::fs::read_to_string(&p).unwrap()
        };
        let field = "\n# Subtitles are trimmed.\n[mapping.types.post.fields.\"meta.subtitle\"]\nto = \"subtitle\"\ntransform = [{ op = \"trim\" }]\n";
        assert!(text.contains(field));
        assert_eq!(
            save(&|d| d.remove_field("post", "meta.subtitle")),
            text.replacen(field, "", 1)
        );
        let hero = "\n[[mapping.blocks]]\nmatch = { module = \"hero_banner\" }   # the big one\nto = \"hero\"\nfields = { title = \"heading\" }\n";
        assert!(text.contains(hero));
        let m = BlockMatch {
            kind: None,
            module: Some("hero_banner".into()),
        };
        assert_eq!(save(&|d| d.set_block(&m, None)), text.replacen(hero, "", 1));
        assert_eq!(
            save(&|d| d.drop_type("post", Some("x"))),
            text.replacen(
                "to   = \"pages\"        # renamed in 2025\npath = \"/{slug}/\"\n",
                "path = \"/{slug}/\"\ndrop_reason = \"x\"\n",
                1
            )
        );
    }

    /// Quotes, backslashes, newlines and non-ASCII survive a write and read; nothing else
    /// is added.
    #[test]
    fn awkward_values_round_trip() {
        let (_d, p) = fixture(FIXTURE);
        let mut doc = MappingDoc::open(&p).unwrap();
        let v = "a \"quoted\" \\path\\ \nline two é 🚀 'single'";
        doc.set_type_to("post", Some(v));
        doc.set_field(
            "post",
            "title",
            FieldMapEdit {
                to: v.into(),
                transform: vec![],
            },
        );
        assert_eq!(doc.save(), Vec::<String>::new());
        let m = crate::config::ProjectConfig::load(&p).unwrap().mapping;
        assert_eq!(m.types["post"].to.as_deref(), Some(v));
        assert_eq!(m.types["post"].fields["title"].to, v);
        let raw: toml::Table = toml::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let post = raw["mapping"]["types"]["post"].as_table().unwrap();
        let keys: Vec<_> = post.keys().map(String::as_str).collect();
        assert_eq!(keys, ["fields", "path", "to"]);
        let title = post["fields"]["title"].as_table().unwrap();
        assert_eq!(title.keys().collect::<Vec<_>>(), ["to"]);
    }

    /// An unchanged value is left as written, quoting style included.
    #[test]
    fn unchanged_value_keeps_its_quoting() {
        let text = FIXTURE.replace("to = \"title\"   # keep", "to = 'title'   # keep");
        let (_d, p) = fixture(&text);
        let mut doc = MappingDoc::open(&p).unwrap();
        doc.set_field(
            "post",
            "title",
            FieldMapEdit {
                to: "title".into(),
                transform: vec![Transform::Trim],
            },
        );
        assert_eq!(doc.save(), Vec::<String>::new());
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(
            after.contains("to = 'title'   # keep\ntransform = { op = \"trim\" }\n"),
            "{after}"
        );
        assert_eq!(after.lines().count(), text.lines().count() + 1);
    }

    #[test]
    fn open_refuses_a_broken_file() {
        let (_d, p) = fixture("[mapping.types.post]\ntoo = \"x\"\n");
        let e = MappingDoc::open(&p).err().unwrap();
        assert!(format!("{e:#}").contains("too"), "{e:#}");
    }

    #[test]
    fn mapping_created_when_absent() {
        let (_d, p) = fixture("# just a name\n[project]\nname = \"x\"\n");
        let mut doc = MappingDoc::open(&p).unwrap();
        doc.set_type_to("post", Some("posts"));
        doc.set_raw_block(Some("raw"));
        assert_eq!(doc.save(), Vec::<String>::new());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.starts_with("# just a name\n[project]\nname = \"x\"\n"),
            "{text}"
        );
        let m = crate::config::ProjectConfig::load(&p).unwrap().mapping;
        assert_eq!(m.types["post"].to.as_deref(), Some("posts"));
        assert_eq!(m.raw_block.as_deref(), Some("raw"));
    }
}
