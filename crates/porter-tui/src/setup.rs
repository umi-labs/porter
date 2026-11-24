//! The setup form: shown when the folder has no `porter.toml`. The form is data (`FIELDS`);
//! secrets are never typed here, the form only shows which env var each connector reads and
//! whether it is set.
use anyhow::{Context, Result};
use porter_engine::config::{ProjectConfig, is_env_var_name};
use porter_engine::project::Project;
use porter_engine::registry;
use std::path::Path;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Table, value};

/// What a field holds. There is deliberately no secret kind.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Text,
    Path,
    /// Comma-separated.
    List,
    Choice(fn() -> &'static [&'static str]),
}

pub struct Field {
    /// `section.name`; the engine's validation messages start with the same key.
    pub id: &'static str,
    pub section: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub required: bool,
    pub hint: &'static str,
}

pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    /// A connector sub-form, opened by its `use` field.
    pub optional: bool,
}

fn sources() -> &'static [&'static str] {
    registry::source_names()
}
fn destinations() -> &'static [&'static str] {
    registry::destination_names()
}
fn yes_no() -> &'static [&'static str] {
    &["no", "yes"]
}
fn sanity_assets() -> &'static [&'static str] {
    &["", "referenced", "all"]
}
fn auths() -> &'static [&'static str] {
    &["", "basic", "bearer", "apikey"]
}

const fn s(id: &'static str, title: &'static str, optional: bool) -> Section {
    Section {
        id,
        title,
        optional,
    }
}

const fn f(
    id: &'static str,
    section: &'static str,
    label: &'static str,
    kind: Kind,
    required: bool,
    hint: &'static str,
) -> Field {
    Field {
        id,
        section,
        label,
        kind,
        required,
        hint,
    }
}

pub const SECTIONS: &[Section] = &[
    s("project", "Project", false),
    s("source", "Source", false),
    s("crawl", "Connector: crawl a live site", true),
    s("wxr", "Connector: WordPress export (WXR)", true),
    s("wp-rest", "Connector: WordPress REST API", true),
    s("wp-db", "Connector: WordPress database dump", true),
    s("umbraco-json", "Connector: Umbraco JSON export", true),
    s("sanity-api", "Connector: Sanity API", true),
    s("sanity-export", "Connector: Sanity dataset export", true),
    s("destination", "Destination", false),
];

pub const FIELDS: &[Field] = &[
    f("project.name", "project", "name", Kind::Text, true, ""),
    f(
        "source.adapter",
        "source",
        "adapter",
        Kind::Choice(sources),
        true,
        "",
    ),
    f("crawl.use", "crawl", "use", Kind::Choice(yes_no), false, ""),
    f(
        "crawl.url",
        "crawl",
        "url",
        Kind::Text,
        true,
        "https://example.com",
    ),
    f(
        "crawl.include",
        "crawl",
        "include",
        Kind::List,
        false,
        "/blog, /news",
    ),
    f(
        "crawl.exclude",
        "crawl",
        "exclude",
        Kind::List,
        false,
        "/tag",
    ),
    f("wxr.use", "wxr", "use", Kind::Choice(yes_no), false, ""),
    f(
        "wxr.path",
        "wxr",
        "export file",
        Kind::Path,
        true,
        "export.xml",
    ),
    f(
        "wp-rest.use",
        "wp-rest",
        "use",
        Kind::Choice(yes_no),
        false,
        "",
    ),
    f(
        "wp-rest.url",
        "wp-rest",
        "site url",
        Kind::Text,
        true,
        "https://example.com",
    ),
    f(
        "wp-rest.auth",
        "wp-rest",
        "auth type",
        Kind::Choice(auths),
        false,
        "",
    ),
    f("wp-db.use", "wp-db", "use", Kind::Choice(yes_no), false, ""),
    f(
        "wp-db.dump",
        "wp-db",
        "SQL dump",
        Kind::Path,
        true,
        "site.sql.gz",
    ),
    f(
        "wp-db.wp_root",
        "wp-db",
        "wp_root",
        Kind::Path,
        false,
        "wordpress/",
    ),
    f(
        "wp-db.uploads_dir",
        "wp-db",
        "uploads_dir",
        Kind::Path,
        false,
        "uploads/",
    ),
    f(
        "umbraco-json.use",
        "umbraco-json",
        "use",
        Kind::Choice(yes_no),
        false,
        "",
    ),
    f(
        "umbraco-json.paths",
        "umbraco-json",
        "export files",
        Kind::List,
        true,
        "a.json, b.json",
    ),
    f(
        "sanity-api.use",
        "sanity-api",
        "use",
        Kind::Choice(yes_no),
        false,
        "",
    ),
    f(
        "sanity-api.project_id",
        "sanity-api",
        "project id",
        Kind::Text,
        true,
        "abc123",
    ),
    f(
        "sanity-api.dataset",
        "sanity-api",
        "dataset",
        Kind::Text,
        true,
        "production",
    ),
    f(
        "sanity-api.token_env",
        "sanity-api",
        "token env var NAME",
        Kind::Text,
        false,
        "PORTER_SANITY_TOKEN",
    ),
    f(
        "sanity-api.assets",
        "sanity-api",
        "assets",
        Kind::Choice(sanity_assets),
        false,
        "",
    ),
    f(
        "sanity-export.use",
        "sanity-export",
        "use",
        Kind::Choice(yes_no),
        false,
        "",
    ),
    f(
        "sanity-export.path",
        "sanity-export",
        "export archive",
        Kind::Path,
        true,
        "production.tar.gz",
    ),
    f(
        "sanity-export.assets",
        "sanity-export",
        "assets",
        Kind::Choice(sanity_assets),
        false,
        "",
    ),
    f(
        "destination.adapter",
        "destination",
        "adapter",
        Kind::Choice(destinations),
        true,
        "",
    ),
    f(
        "destination.url",
        "destination",
        "url",
        Kind::Text,
        true,
        "https://cms.example.com",
    ),
    f(
        "destination.api_key_env",
        "destination",
        "env var NAME",
        Kind::Text,
        true,
        "",
    ),
    f(
        "destination.config_dir",
        "destination",
        "config_dir",
        Kind::Path,
        false,
        "path to the CMS project",
    ),
];

/// An env var a connector reads, and whether it is set right now. Never the value.
#[derive(Clone, Debug, PartialEq)]
pub struct Secret {
    pub section: &'static str,
    pub var: String,
    pub set: bool,
}

/// Why a save did not happen: problems next to their fields, plus the ones with no field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Failure {
    pub fields: Vec<(usize, String)>,
    pub general: Vec<String>,
}

impl Failure {
    pub fn general(m: impl Into<String>) -> Failure {
        Failure {
            fields: vec![],
            general: vec![m.into()],
        }
    }
}

pub fn field_index(id: &str) -> Option<usize> {
    FIELDS.iter().position(|f| f.id == id)
}

pub struct Setup {
    pub values: Vec<String>,
    /// Index into `FIELDS`.
    pub focus: usize,
    pub editing: bool,
    pub saving: bool,
    pub failure: Failure,
}

impl Setup {
    pub fn new(default_name: &str) -> Setup {
        let values = FIELDS
            .iter()
            .map(|f| match (f.id, f.kind) {
                ("project.name", _) => default_name.to_string(),
                ("destination.api_key_env", _) => "PORTER_PAYLOAD_API_KEY".into(),
                (_, Kind::Choice(o)) => o().first().copied().unwrap_or("").to_string(),
                _ => String::new(),
            })
            .collect();
        Setup::from_values(values)
    }

    pub fn from_values(values: Vec<String>) -> Setup {
        Setup {
            values,
            focus: 0,
            editing: false,
            saving: false,
            failure: Failure::default(),
        }
    }

    pub fn get(&self, id: &str) -> &str {
        field_index(id)
            .and_then(|i| self.values.get(i))
            .map_or("", |v| v.as_str())
    }

    #[cfg(test)]
    pub fn set(&mut self, id: &str, v: &str) {
        if let Some(slot) = field_index(id).and_then(|i| self.values.get_mut(i)) {
            *slot = v.to_string();
        }
    }

    fn section_on(&self, section: &str) -> bool {
        SECTIONS
            .iter()
            .find(|s| s.id == section)
            .is_some_and(|s| !s.optional || self.get(&format!("{section}.use")) == "yes")
    }

    /// Fields on screen: a collapsed connector shows only its `use` toggle.
    pub fn shown(&self, i: usize) -> bool {
        FIELDS
            .get(i)
            .is_some_and(|f| f.id.ends_with(".use") || self.section_on(f.section))
    }

    pub fn step(&mut self, d: isize) {
        let n = FIELDS.len() as isize;
        let mut i = self.focus as isize;
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if self.shown(i as usize) {
                self.focus = i as usize;
                return;
            }
        }
    }

    /// Enter: type into a text field, or pick the next choice.
    pub fn enter(&mut self) {
        match FIELDS[self.focus].kind {
            Kind::Choice(_) => self.cycle(1),
            _ => self.editing = true,
        }
    }

    pub fn cycle(&mut self, d: isize) {
        let Kind::Choice(opts) = FIELDS[self.focus].kind else {
            return;
        };
        let opts = opts();
        let cur = opts
            .iter()
            .position(|o| *o == self.values[self.focus])
            .unwrap_or(0) as isize;
        let next = (cur + d).rem_euclid(opts.len().max(1) as isize) as usize;
        self.values[self.focus] = opts.get(next).copied().unwrap_or("").to_string();
    }

    pub fn push(&mut self, c: char) {
        if self.editing && !matches!(FIELDS[self.focus].kind, Kind::Choice(_)) {
            self.values[self.focus].push(c);
        }
    }

    pub fn pop(&mut self) {
        if self.editing {
            self.values[self.focus].pop();
        }
    }

    /// Required fields of the sections in use that are still empty.
    pub fn missing(&self) -> Failure {
        let fields = FIELDS
            .iter()
            .enumerate()
            .filter(|(i, f)| f.required && self.shown(*i) && self.values[*i].trim().is_empty())
            .map(|(i, f)| (i, format!("{} is missing", f.id)))
            .collect();
        Failure {
            fields,
            general: vec![],
        }
    }

    /// The env vars the chosen connectors and destination read. `is_set` is the presence
    /// check (never the value).
    pub fn secrets(&self, is_set: &dyn Fn(&str) -> bool) -> Vec<Secret> {
        let mut out = vec![];
        let mut add = |section: &'static str, var: &str| {
            out.push(Secret {
                section,
                var: var.to_string(),
                set: is_set(var),
            })
        };
        if self.section_on("wp-rest") {
            match self.get("wp-rest.auth") {
                "basic" => {
                    add("wp-rest", "PORTER_WP_USERNAME");
                    add("wp-rest", "PORTER_WP_PASSWORD");
                }
                "bearer" => add("wp-rest", "PORTER_WP_BEARER_TOKEN"),
                "apikey" => add("wp-rest", "PORTER_WP_API_KEY"),
                _ => {}
            }
        }
        if self.section_on("sanity-api") {
            let var = self.get("sanity-api.token_env").trim();
            // Blank means the default; an invalid name may be a pasted token: never shown.
            let var = if var.is_empty() {
                "PORTER_SANITY_TOKEN"
            } else {
                var
            };
            if is_env_var_name(var) {
                add("sanity-api", var);
            }
        }
        let name = self.get("destination.api_key_env").trim();
        // An invalid name may be a pasted key: never shown (see `ui::form`).
        if is_env_var_name(name) {
            add("destination", name);
        }
        out
    }

    /// The connector sections in use, in the order they are written.
    fn connectors(&self) -> Vec<&'static str> {
        SECTIONS
            .iter()
            .filter(|s| s.optional && self.section_on(s.id))
            .map(|s| s.id)
            .collect()
    }

    fn list(&self, id: &str) -> Vec<String> {
        self.get(id)
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    }

    /// Sets `key` to the trimmed value unless it is empty.
    fn opt(&self, t: &mut Table, key: &str, id: &str) {
        let v = self.get(id).trim();
        if !v.is_empty() {
            t[key] = value(v);
        }
    }

    fn connector_table(&self, kind: &str) -> Table {
        let mut t = Table::new();
        t["kind"] = value(kind);
        let id = |k: &str| format!("{kind}.{k}");
        let arr = |items: Vec<String>| items.into_iter().collect::<Array>();
        match kind {
            "crawl" => {
                self.opt(&mut t, "url", &id("url"));
                for k in ["include", "exclude"] {
                    let items = self.list(&id(k));
                    if !items.is_empty() {
                        t[k] = value(arr(items));
                    }
                }
            }
            "wxr" => self.opt(&mut t, "path", &id("path")),
            "wp-rest" => {
                self.opt(&mut t, "url", &id("url"));
                self.opt(&mut t, "auth", &id("auth"));
            }
            "wp-db" => {
                self.opt(&mut t, "dump", &id("dump"));
                self.opt(&mut t, "wp_root", &id("wp_root"));
                self.opt(&mut t, "uploads_dir", &id("uploads_dir"));
            }
            "sanity-api" => {
                for k in ["project_id", "dataset", "token_env", "assets"] {
                    self.opt(&mut t, k, &id(k));
                }
            }
            "sanity-export" => {
                self.opt(&mut t, "path", &id("path"));
                self.opt(&mut t, "assets", &id("assets"));
            }
            _ => t["paths"] = value(arr(self.list(&id("paths")))),
        }
        t
    }

    /// Maps an engine validation message (`key problem`) to the field it is about.
    fn field_for(&self, msg: &str) -> Option<usize> {
        let key = msg.split(' ').next()?;
        if let Some(rest) = key.strip_prefix("connectors[") {
            let (n, field) = rest.split_once("].")?;
            let section = *self.connectors().get(n.parse::<usize>().ok()?)?;
            return field_index(&format!("{section}.{field}"));
        }
        field_index(key)
    }
}

/// The text of `porter.toml`: `Project::init`'s template, then the connectors and the
/// destination edited in, comments kept.
fn render(s: &Setup) -> Result<(String, ProjectConfig)> {
    let scratch = tempfile::tempdir().context("scratch folder")?;
    Project::init(
        scratch.path(),
        s.get("project.name"),
        s.get("source.adapter"),
    )?;
    let path = scratch.path().join("porter.toml");
    let mut doc: DocumentMut = std::fs::read_to_string(&path)?.parse()?;
    let mut aot = ArrayOfTables::new();
    for kind in s.connectors() {
        aot.push(s.connector_table(kind));
    }
    if !aot.is_empty() {
        doc["connectors"] = toml_edit::Item::ArrayOfTables(aot);
    }
    let Some(dest) = doc["destination"].as_table_mut() else {
        anyhow::bail!("the template has no [destination]");
    };
    dest["adapter"] = value(s.get("destination.adapter"));
    s.opt(dest, "url", "destination.url");
    s.opt(dest, "api_key_env", "destination.api_key_env");
    s.opt(dest, "config_dir", "destination.config_dir");
    let text = doc.to_string();
    std::fs::write(&path, &text)?;
    let config = ProjectConfig::load(&path).context("the generated porter.toml does not load")?;
    Ok((text, config))
}

/// Builds `porter.toml` the way `porter init` does and edits the connectors and destination
/// in; the result must load and validate before anything is written to `dir`, so a form
/// with problems leaves the folder untouched. The write is atomic and never replaces an
/// existing file (private temp file in `dir`, then a no-clobber persist).
pub fn save(dir: &Path, values: &[String]) -> Result<(), Failure> {
    save_with(dir, values, &|| {})
}

fn save_with(dir: &Path, values: &[String], before_persist: &dyn Fn()) -> Result<(), Failure> {
    let s = Setup::from_values(values.to_vec());
    let missing = s.missing();
    if !missing.fields.is_empty() {
        return Err(missing);
    }
    let io = |e: anyhow::Error| Failure::general(format!("{e:#}"));
    let (text, config) = render(&s).map_err(io)?;
    let mut failure = Failure::default();
    for m in config.validate() {
        match s.field_for(&m) {
            Some(i) => failure.fields.push((i, m)),
            None => failure.general.push(m),
        }
    }
    if !failure.fields.is_empty() || !failure.general.is_empty() {
        return Err(failure);
    }
    let target = dir.join("porter.toml");
    porter_engine::mapping::edit::write_atomic_noclobber(&target, &text, before_persist).map_err(
        |e| {
            let exists = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists);
            if exists {
                io(anyhow::anyhow!(
                    "porter.toml was created meanwhile — not overwritten"
                ))
            } else {
                io(e)
            }
        },
    )
}

/// Env presence, never the value.
pub fn env_is_set(var: &str) -> bool {
    std::env::var_os(var).is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> Setup {
        let mut s = Setup::new("demo");
        s.set("source.adapter", "wordpress");
        s.set("crawl.use", "yes");
        s.set("crawl.url", "https://example.com");
        s.set("crawl.include", "/blog, /news");
        s.set("wp-rest.use", "yes");
        s.set("wp-rest.url", "https://example.com");
        s.set("wp-rest.auth", "basic");
        s.set("destination.url", "https://cms.example.com");
        s
    }

    #[test]
    fn setup_writes_valid_config() {
        let d = tempfile::tempdir().unwrap();
        save(d.path(), &filled().values).unwrap();
        let path = d.path().join("porter.toml");
        let c = ProjectConfig::load(&path).unwrap();
        assert!(c.validate().is_empty(), "{:?}", c.validate());
        assert_eq!(c.project.name, "demo");
        assert_eq!(c.destination.api_key_env, "PORTER_PAYLOAD_API_KEY");
        assert_eq!(c.connectors.len(), 2);
        assert!(matches!(
            &c.connectors[0],
            porter_engine::config::ConnectorConfig::Crawl { include, .. } if include == &["/blog", "/news"]
        ));
        assert!(
            matches!(&c.connectors[1], porter_engine::config::ConnectorConfig::WpRest { auth: Some(a), .. } if a == "basic")
        );
        // Same file `porter init` writes: its comments survive the edit.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Credentials never go here"), "{text}");
        // And the project opens, which the app does next.
        porter_engine::session::Session::open(d.path()).unwrap();
    }

    #[test]
    fn invalid_form_writes_nothing_and_points_at_fields() {
        let d = tempfile::tempdir().unwrap();
        let mut s = filled();
        s.set("destination.url", "not a url");
        let e = save(d.path(), &s.values).unwrap_err();
        let i = field_index("destination.url").unwrap();
        assert!(
            e.fields
                .iter()
                .any(|(f, m)| *f == i && m.contains("destination.url")),
            "{e:?}"
        );
        assert!(!d.path().join("porter.toml").exists());
        // A connector's own problem lands on its field too.
        let mut s = filled();
        s.set("wp-rest.url", "https://u:p@example.com");
        let e = save(d.path(), &s.values).unwrap_err();
        let i = field_index("wp-rest.url").unwrap();
        assert!(e.fields.iter().any(|(f, _)| *f == i), "{e:?}");
        // Required fields are checked before anything else.
        let mut s = filled();
        s.set("crawl.url", " ");
        let e = save(d.path(), &s.values).unwrap_err();
        assert_eq!(e.fields.len(), 1, "{e:?}");
    }

    #[test]
    fn existing_porter_toml_is_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("porter.toml"), "mine").unwrap();
        let e = save(d.path(), &filled().values).unwrap_err();
        assert!(e.general[0].contains("not overwritten"), "{e:?}");
        assert_eq!(
            std::fs::read_to_string(d.path().join("porter.toml")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn setup_never_has_secret_fields() {
        for f in FIELDS {
            // `token_env` holds the NAME of an env var, and its label says so.
            if f.id.ends_with("_env") {
                assert!(f.label.contains("NAME"), "{}", f.id);
                continue;
            }
            let name = format!("{} {}", f.id, f.label).to_lowercase();
            for bad in ["password", "token", "secret", "key-value"] {
                assert!(!name.contains(bad), "{name}");
            }
        }
        // Every kind is one of the plain ones; the enum has no secret variant to pick.
        assert!(FIELDS.iter().all(|f| matches!(
            f.kind,
            Kind::Text | Kind::Path | Kind::List | Kind::Choice(_)
        )));
        // api_key_env is the NAME of the env var, and says so.
        let f = &FIELDS[field_index("destination.api_key_env").unwrap()];
        assert!(f.label.contains("NAME"));
    }

    #[test]
    fn secret_indicators_report_presence_per_chosen_auth() {
        let mut s = filled();
        let set = |v: &str| v == "PORTER_WP_USERNAME" || v == "PORTER_PAYLOAD_API_KEY";
        let got = s.secrets(&set);
        let vars: Vec<_> = got.iter().map(|x| (x.var.as_str(), x.set)).collect();
        assert_eq!(
            vars,
            [
                ("PORTER_WP_USERNAME", true),
                ("PORTER_WP_PASSWORD", false),
                ("PORTER_PAYLOAD_API_KEY", true)
            ]
        );
        s.set("wp-rest.auth", "bearer");
        assert_eq!(s.secrets(&set)[0].var, "PORTER_WP_BEARER_TOKEN");
        s.set("wp-rest.use", "no");
        assert_eq!(
            s.secrets(&set).len(),
            1,
            "a collapsed connector reads nothing"
        );
    }

    #[test]
    fn collapsed_connectors_are_skipped_by_focus() {
        let mut s = Setup::new("demo");
        s.set("source.adapter", "wordpress");
        let mut seen = vec![];
        for _ in 0..8 {
            s.step(1);
            seen.push(FIELDS[s.focus].id);
        }
        assert!(!seen.contains(&"crawl.url"), "{seen:?}");
        assert!(seen.contains(&"crawl.use"), "{seen:?}");
        s.focus = field_index("crawl.use").unwrap();
        s.enter();
        s.step(1);
        assert_eq!(FIELDS[s.focus].id, "crawl.url");
    }

    #[test]
    fn porter_toml_created_meanwhile_is_not_clobbered() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("porter.toml");
        let e = save_with(d.path(), &filled().values, &|| {
            std::fs::write(&target, "theirs").unwrap()
        })
        .unwrap_err();
        assert!(e.general[0].contains("created meanwhile"), "{e:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "theirs");
        let left: Vec<_> = std::fs::read_dir(d.path()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "no temp file left behind: {left:?}");
    }

    fn round_trip(s: &Setup) -> ProjectConfig {
        let d = tempfile::tempdir().unwrap();
        save(d.path(), &s.values).unwrap_or_else(|e| panic!("{e:?}"));
        let c = ProjectConfig::load(&d.path().join("porter.toml")).unwrap();
        assert!(c.validate().is_empty(), "{:?}", c.validate());
        c
    }

    #[test]
    fn every_connector_kind_round_trips_alone() {
        use porter_engine::config::ConnectorConfig as C;
        let minimal: &[(&str, &[(&str, &str)])] = &[
            (
                "crawl",
                &[("url", "https://example.com"), ("exclude", "/tag, /x")],
            ),
            ("wxr", &[("path", "export.xml")]),
            (
                "wp-rest",
                &[("url", "https://example.com"), ("auth", "bearer")],
            ),
            (
                "wp-db",
                &[
                    ("dump", "a.sql.gz"),
                    ("wp_root", "wp"),
                    ("uploads_dir", "up"),
                ],
            ),
            ("umbraco-json", &[("paths", "a.json, b.json")]),
            (
                "sanity-api",
                &[
                    ("project_id", "abc123"),
                    ("dataset", "production"),
                    ("token_env", "MY_SANITY_TOKEN"),
                    ("assets", "all"),
                ],
            ),
            (
                "sanity-export",
                &[("path", "export.tar.gz"), ("assets", "all")],
            ),
        ];
        let offered: Vec<_> = SECTIONS
            .iter()
            .filter(|s| s.optional)
            .map(|s| s.id)
            .collect();
        assert_eq!(
            offered.len(),
            minimal.len(),
            "a new connector needs a case here"
        );
        for (kind, vals) in minimal {
            let mut s = Setup::new("demo");
            s.set(&format!("{kind}.use"), "yes");
            for (k, v) in *vals {
                s.set(&format!("{kind}.{k}"), v);
            }
            s.set("destination.url", "https://cms.example.com");
            let c = round_trip(&s);
            assert_eq!(c.connectors.len(), 1, "{kind}");
            match (&c.connectors[0], *kind) {
                (C::Crawl { exclude, .. }, "crawl") => assert_eq!(exclude, &["/tag", "/x"]),
                (C::Wxr { path }, "wxr") => assert_eq!(path.to_str(), Some("export.xml")),
                (C::WpRest { auth, .. }, "wp-rest") => assert_eq!(auth.as_deref(), Some("bearer")),
                (
                    C::WpDb {
                        wp_root,
                        uploads_dir,
                        ..
                    },
                    "wp-db",
                ) => {
                    assert_eq!(wp_root.as_deref().and_then(|p| p.to_str()), Some("wp"));
                    assert_eq!(uploads_dir.as_deref().and_then(|p| p.to_str()), Some("up"));
                }
                (C::UmbracoJson { paths }, "umbraco-json") => assert_eq!(paths.len(), 2),
                (
                    C::SanityApi {
                        project_id,
                        dataset,
                        token_env,
                        assets,
                        ..
                    },
                    "sanity-api",
                ) => {
                    assert_eq!(
                        (project_id.as_str(), dataset.as_str()),
                        ("abc123", "production")
                    );
                    assert_eq!(token_env, "MY_SANITY_TOKEN");
                    assert_eq!(*assets, porter_engine::config::SanityAssets::All);
                }
                (C::SanityExport { path, assets }, "sanity-export") => {
                    assert_eq!(path.to_str(), Some("export.tar.gz"));
                    assert_eq!(*assets, porter_engine::config::SanityAssets::All);
                }
                (other, _) => panic!("{kind} wrote {other:?}"),
            }
        }
    }

    #[test]
    fn every_source_and_destination_choice_round_trips() {
        for src in registry::source_names() {
            for dst in registry::destination_names() {
                let mut s = Setup::new("demo");
                s.set("source.adapter", src);
                s.set("destination.adapter", dst);
                s.set("destination.url", "https://cms.example.com");
                let c = round_trip(&s);
                assert_eq!(
                    (c.source.adapter.as_str(), c.destination.adapter.as_str()),
                    (*src, *dst)
                );
            }
        }
    }

    #[test]
    fn pasted_secret_as_env_var_name_is_refused_without_echo() {
        let d = tempfile::tempdir().unwrap();
        let mut s = filled();
        s.set("destination.api_key_env", "sk-live-abc123");
        let e = save(d.path(), &s.values).unwrap_err();
        let i = field_index("destination.api_key_env").unwrap();
        assert!(e.fields.iter().any(|(f, _)| *f == i), "{e:?}");
        assert!(!format!("{e:?}").contains("sk-live"), "{e:?}");
        assert!(!d.path().join("porter.toml").exists());
        assert!(
            s.secrets(&|_| true)
                .iter()
                .all(|x| !x.var.contains("sk-live"))
        );
    }
}
