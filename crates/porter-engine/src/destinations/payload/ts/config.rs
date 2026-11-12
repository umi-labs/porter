//! `read_payload_config`: every collection, global and block config under a directory.
//!
//! Config expressions are evaluated to JSON: object/array literals, spreads, `as const` /
//! `satisfies`, local and imported consts (relative imports and tsconfig path aliases), and
//! calls to the site's own field helpers (params bound, `return`/arrow body evaluated).
//! Anything else (node-module imports, conditionals) becomes an "unresolved" marker, which
//! is warned about only where a field was expected.

use super::import_resolver::{ImportResolver, PathAlias, parse_ts, parse_ts_str};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use swc_common::{FileName, SourceMap, sync::Lrc};
use swc_ecma_ast::*;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DestSchema {
    pub collections: IndexMap<String, CollectionSchema>,
    pub globals: IndexMap<String, CollectionSchema>,
    /// Block slug -> its fields: standalone `Block` configs and every inline block.
    pub blocks: IndexMap<String, Vec<DestField>>,
    /// Unreadable files and unresolvable fields, for schema-mismatch warnings.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct CollectionSchema {
    pub fields: Vec<DestField>,
    pub upload: bool,
    pub drafts: bool,
    /// `folders` is enabled (Payload's `payload-folders`).
    pub folders: bool,
}

/// One field, `kind` being Payload's type name. `row`/`collapsible`/unnamed tabs are
/// flattened into the parent; a named tab is a `group`. A `blocks` field's `fields` are
/// its blocks, each `kind = "block"` named by slug.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct DestField {
    pub name: String,
    pub kind: String,
    pub required: bool,
    /// `required` is an expression the reader couldn't evaluate.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub required_unknown: bool,
    /// `hasMany`.
    pub many: bool,
    pub relation_to: Vec<String>,
    /// `relationTo` was an array: Payload stores `{relationTo, value}`, even with one entry.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub polymorphic: bool,
    pub fields: Vec<DestField>,
}

const UNRESOLVED: &str = "$unresolved";
const MAX_DEPTH: usize = 64;

fn unresolved(what: impl Into<String>) -> Value {
    json!({ UNRESOLVED: what.into() })
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Collection,
    Global,
    Block,
}

enum Binding {
    Expr(Box<Expr>, Option<Role>),
    Fn(Box<Function>),
    Import { src: String, name: String },
}

#[derive(Default)]
struct Mod {
    path: PathBuf,
    decls: HashMap<String, Binding>,
    /// Exported name -> local binding name.
    exports: IndexMap<String, String>,
    star_from: Vec<String>,
}

enum Callable<'a> {
    Arrow(&'a ArrowExpr),
    Fn(&'a Function),
}

type Env = HashMap<String, Value>;

/// Reads every collection, global and block config under `dir` (skipping `node_modules`
/// and dot-directories). `payload.config.ts` goes first, so its inline collections win.
pub fn read_payload_config(dir: &Path) -> anyhow::Result<DestSchema> {
    if !dir.is_dir() {
        anyhow::bail!("{} is not a directory", dir.display());
    }
    let cm: Lrc<SourceMap> = Default::default();
    let (base, aliases) = tsconfig(&cm, dir);
    let mut r = Reader {
        dir: dir.to_path_buf(),
        resolver: ImportResolver::new(base, aliases.as_deref()),
        cm,
        mods: HashMap::new(),
        stack: vec![],
        out: DestSchema::default(),
        cur: String::new(),
    };
    let mut files = vec![];
    walk(dir, &mut files);
    files.sort_by_key(|p| {
        (
            p.file_name().is_none_or(|n| n != "payload.config.ts"),
            p.clone(),
        )
    });
    for f in files {
        let Some(m) = r.load(&f) else { continue };
        r.cur = r.rel(&f);
        if f.file_name().is_some_and(|n| n == "payload.config.ts") {
            let cfg = r.export(&m, "default");
            for b in arr(&cfg["blocks"]) {
                r.add_block(b);
            }
            for c in arr(&cfg["collections"]) {
                r.add_collection(c, Role::Collection);
            }
            for g in arr(&cfg["globals"]) {
                r.add_collection(g, Role::Global);
            }
        }
        let roles: Vec<(String, Role)> = m
            .exports
            .iter()
            .filter_map(|(_, local)| match m.decls.get(local) {
                Some(Binding::Expr(_, Some(role))) => Some((local.clone(), *role)),
                _ => None,
            })
            .collect();
        for (local, role) in roles {
            let v = r.ident(&m, &local);
            match role {
                Role::Block => {
                    r.add_block(&v);
                }
                _ => r.add_collection(&v, role),
            }
        }
    }
    Ok(r.out)
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn truthy(v: &Value) -> bool {
    matches!(v, Value::Bool(true)) || (v.is_object() && v.get(UNRESOLVED).is_none())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if name != "node_modules" && !name.starts_with('.') {
                walk(&p, out);
            }
        } else if name.ends_with(".ts") && !name.ends_with(".d.ts") {
            out.push(p);
        }
    }
}

/// The nearest tsconfig.json at or above `dir`: (base dir for aliases, its `paths`).
/// Parsed as a JS expression, so comments and trailing commas are fine.
fn tsconfig(cm: &Lrc<SourceMap>, dir: &Path) -> (PathBuf, Option<Vec<PathAlias>>) {
    let found = dir
        .ancestors()
        .map(|d| d.join("tsconfig.json"))
        .find(|p| p.is_file());
    let Some(path) = found else {
        return (dir.to_path_buf(), None);
    };
    let root = path.parent().unwrap_or(dir).to_path_buf();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let Ok(module) = parse_ts_str(cm, FileName::Anon, format!("({text})"), false) else {
        return (root, None);
    };
    let Some(ModuleItem::Stmt(Stmt::Expr(e))) = module.body.first() else {
        return (root, None);
    };
    let mut r = Reader {
        dir: root.clone(),
        resolver: ImportResolver::new(root.clone(), Some(&[])),
        cm: cm.clone(),
        mods: HashMap::new(),
        stack: vec![],
        out: DestSchema::default(),
        cur: String::new(),
    };
    let v = r.eval(&Rc::new(Mod::default()), &Env::new(), &e.expr);
    let opts = &v["compilerOptions"];
    let base = match opts["baseUrl"].as_str() {
        Some(b) => root.join(b),
        None => root,
    };
    let aliases = opts["paths"].as_object().map(|m| {
        m.iter()
            .map(|(alias, to)| PathAlias {
                alias: alias.clone(),
                paths: arr(to)
                    .iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect(),
            })
            .collect()
    });
    (base, aliases)
}

fn stmt_kind(s: &Stmt) -> &'static str {
    match s {
        Stmt::If(_) => "if",
        Stmt::Expr(_) => "expression",
        Stmt::For(_) | Stmt::ForIn(_) | Stmt::ForOf(_) | Stmt::While(_) | Stmt::DoWhile(_) => {
            "loop"
        }
        Stmt::Switch(_) => "switch",
        Stmt::Try(_) => "try",
        Stmt::Decl(_) => "declaration",
        _ => "other",
    }
}

fn unres(v: &Value) -> Option<&str> {
    v.get(UNRESOLVED).and_then(Value::as_str)
}

/// `true`/`false`, None when it's an expression we couldn't evaluate (absent = false).
fn flag(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Null => Some(false),
        _ => None,
    }
}

struct Reader {
    dir: PathBuf,
    resolver: ImportResolver,
    cm: Lrc<SourceMap>,
    mods: HashMap<PathBuf, Option<Rc<Mod>>>,
    /// (file, name) being evaluated: cycle guard.
    stack: Vec<(PathBuf, String)>,
    out: DestSchema,
    /// The file whose configs are being read, for warnings.
    cur: String,
}

fn atom(w: &swc_atoms::Wtf8Atom) -> String {
    w.to_atom_lossy().to_string()
}

fn export_name(n: &ModuleExportName) -> String {
    match n {
        ModuleExportName::Ident(i) => i.sym.to_string(),
        ModuleExportName::Str(s) => atom(&s.value),
    }
}

fn type_role(t: &TsType) -> Option<Role> {
    let TsType::TsTypeRef(r) = t else { return None };
    let name = match &r.type_name {
        TsEntityName::Ident(i) => i.sym.to_string(),
        TsEntityName::TsQualifiedName(q) => q.right.sym.to_string(),
        #[allow(unreachable_patterns)]
        _ => return None,
    };
    match name.as_str() {
        "CollectionConfig" => Some(Role::Collection),
        "GlobalConfig" => Some(Role::Global),
        "Block" => Some(Role::Block),
        _ => None,
    }
}

/// The role from an annotation, `as`/`satisfies`, or (unannotated) the object's keys.
fn role(ann: Option<&TsTypeAnn>, init: &Expr) -> Option<Role> {
    if let Some(r) = ann.and_then(|a| type_role(&a.type_ann)) {
        return Some(r);
    }
    match init {
        Expr::TsAs(e) => type_role(&e.type_ann).or_else(|| role(None, &e.expr)),
        Expr::TsSatisfies(e) => type_role(&e.type_ann).or_else(|| role(None, &e.expr)),
        Expr::TsConstAssertion(e) => role(None, &e.expr),
        Expr::Paren(e) => role(None, &e.expr),
        Expr::Object(o) => {
            let keys: Vec<String> = o
                .props
                .iter()
                .filter_map(|p| match p {
                    PropOrSpread::Prop(p) => match &**p {
                        Prop::KeyValue(kv) => prop_key(&kv.key),
                        Prop::Shorthand(i) => Some(i.sym.to_string()),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            let has = |k: &str| keys.iter().any(|x| x == k);
            if !(has("slug") && has("fields")) {
                return None;
            }
            let collection = ["upload", "auth", "versions", "access", "hooks", "admin"];
            if collection.iter().any(|k| has(k)) {
                Some(Role::Collection)
            } else if has("labels") || has("interfaceName") {
                Some(Role::Block)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn prop_key(k: &PropName) -> Option<String> {
    match k {
        PropName::Ident(i) => Some(i.sym.to_string()),
        PropName::Str(s) => Some(atom(&s.value)),
        PropName::Num(n) => Some(n.value.to_string()),
        _ => None,
    }
}

fn collect(m: &mut Mod, module: &Module) {
    let var = |m: &mut Mod, v: &VarDecl, exported: bool| {
        for d in &v.decls {
            if let (Pat::Ident(b), Some(init)) = (&d.name, &d.init) {
                let name = b.id.sym.to_string();
                let r = role(b.type_ann.as_deref(), init);
                m.decls.insert(name.clone(), Binding::Expr(init.clone(), r));
                if exported {
                    m.exports.insert(name.clone(), name);
                }
            }
        }
    };
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(v))) => var(m, v, false),
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(f))) => {
                m.decls
                    .insert(f.ident.sym.to_string(), Binding::Fn(f.function.clone()));
            }
            ModuleItem::ModuleDecl(d) => match d {
                ModuleDecl::Import(i) => {
                    let src = atom(&i.src.value);
                    for s in &i.specifiers {
                        let (local, name) = match s {
                            ImportSpecifier::Named(n) => (
                                n.local.sym.to_string(),
                                n.imported
                                    .as_ref()
                                    .map(export_name)
                                    .unwrap_or_else(|| n.local.sym.to_string()),
                            ),
                            ImportSpecifier::Default(d) => {
                                (d.local.sym.to_string(), "default".into())
                            }
                            ImportSpecifier::Namespace(n) => (n.local.sym.to_string(), "*".into()),
                        };
                        m.decls.insert(
                            local,
                            Binding::Import {
                                src: src.clone(),
                                name,
                            },
                        );
                    }
                }
                ModuleDecl::ExportDecl(e) => match &e.decl {
                    Decl::Var(v) => var(m, v, true),
                    Decl::Fn(f) => {
                        let name = f.ident.sym.to_string();
                        m.decls
                            .insert(name.clone(), Binding::Fn(f.function.clone()));
                        m.exports.insert(name.clone(), name);
                    }
                    _ => {}
                },
                ModuleDecl::ExportNamed(n) => {
                    for s in &n.specifiers {
                        let ExportSpecifier::Named(s) = s else {
                            continue;
                        };
                        let orig = export_name(&s.orig);
                        let exported = s.exported.as_ref().map(export_name).unwrap_or(orig.clone());
                        let local = match &n.src {
                            Some(src) => {
                                let local = format!("\0reexport:{exported}");
                                m.decls.insert(
                                    local.clone(),
                                    Binding::Import {
                                        src: atom(&src.value),
                                        name: orig,
                                    },
                                );
                                local
                            }
                            None => orig,
                        };
                        m.exports.insert(exported, local);
                    }
                }
                ModuleDecl::ExportAll(a) => m.star_from.push(atom(&a.src.value)),
                ModuleDecl::ExportDefaultExpr(e) => {
                    let r = role(None, &e.expr);
                    m.decls
                        .insert("default".into(), Binding::Expr(e.expr.clone(), r));
                    m.exports.insert("default".into(), "default".into());
                }
                ModuleDecl::ExportDefaultDecl(e) => {
                    if let DefaultDecl::Fn(f) = &e.decl {
                        m.decls
                            .insert("default".into(), Binding::Fn(f.function.clone()));
                        m.exports.insert("default".into(), "default".into());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

impl Reader {
    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.dir).unwrap_or(p).display().to_string()
    }

    /// A warning about the file whose configs are being read.
    fn warn_at(&mut self, w: String) {
        let w = if self.cur.is_empty() {
            w
        } else {
            format!("{}: {w}", self.cur)
        };
        self.warn(w);
    }

    fn warn(&mut self, w: String) {
        if !self.out.warnings.contains(&w) {
            self.out.warnings.push(w);
        }
    }

    fn load(&mut self, path: &Path) -> Option<Rc<Mod>> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(m) = self.mods.get(&path) {
            return m.clone();
        }
        let m = match parse_ts(&self.cm, &path) {
            Ok(module) => {
                let mut m = Mod {
                    path: path.clone(),
                    ..Mod::default()
                };
                collect(&mut m, &module);
                Some(Rc::new(m))
            }
            Err(e) => {
                let reason = match e {
                    super::SchemaParseError::ParseError { message, .. } => message,
                    e => e.to_string(),
                };
                self.warn(format!("could not read {}: {reason}", self.rel(&path)));
                None
            }
        };
        self.mods.insert(path, m.clone());
        m
    }

    /// The module an import source names, if it's a file we can read.
    fn import(&mut self, from: &Mod, src: &str) -> Option<Rc<Mod>> {
        let p = self.resolver.resolve_import_path(src, &from.path).ok()?;
        self.load(&p)
    }

    fn export(&mut self, m: &Rc<Mod>, name: &str) -> Value {
        if let Some(local) = m.exports.get(name).cloned() {
            return self.ident(m, &local);
        }
        for src in m.star_from.clone() {
            if let Some(t) = self.import(m, &src)
                && (t.exports.contains_key(name) || !t.star_from.is_empty())
            {
                let v = self.export(&t, name);
                if v.get(UNRESOLVED).is_none() {
                    return v;
                }
            }
        }
        unresolved(name)
    }

    fn namespace(&mut self, m: &Rc<Mod>) -> Value {
        let names: Vec<String> = m.exports.keys().cloned().collect();
        let mut o = Map::new();
        for n in names {
            let v = self.export(m, &n);
            o.insert(n, v);
        }
        Value::Object(o)
    }

    /// A module-level name's value.
    fn ident(&mut self, m: &Rc<Mod>, name: &str) -> Value {
        let key = (m.path.clone(), name.to_string());
        if self.stack.contains(&key) || self.stack.len() > MAX_DEPTH {
            return unresolved(name);
        }
        self.stack.push(key);
        let v = match m.decls.get(name) {
            Some(Binding::Expr(e, _)) => self.eval(m, &Env::new(), e),
            Some(Binding::Fn(_)) => Value::Null,
            Some(Binding::Import {
                src,
                name: imported,
            }) => {
                let (src, imported) = (src.clone(), imported.clone());
                match self.import(m, &src) {
                    Some(t) if imported == "*" => self.namespace(&t),
                    Some(t) => self.export(&t, &imported),
                    None => unresolved(name),
                }
            }
            None => unresolved(name),
        };
        self.stack.pop();
        v
    }

    /// The function a name refers to: the module defining it and its local name there.
    fn callable(&mut self, m: &Rc<Mod>, name: &str) -> Option<(Rc<Mod>, String)> {
        if self.stack.len() > MAX_DEPTH {
            return None;
        }
        match m.decls.get(name)? {
            Binding::Fn(_) => Some((m.clone(), name.to_string())),
            Binding::Expr(e, _) if matches!(**e, Expr::Arrow(_) | Expr::Fn(_)) => {
                Some((m.clone(), name.to_string()))
            }
            Binding::Expr(..) => None,
            Binding::Import {
                src,
                name: imported,
            } => {
                let (src, imported) = (src.clone(), imported.clone());
                let t = self.import(m, &src)?;
                let local = t.exports.get(&imported)?.clone();
                self.stack
                    .push((t.path.clone(), format!("\0callable:{local}")));
                let r = self.callable(&t, &local);
                self.stack.pop();
                r
            }
        }
    }

    fn eval(&mut self, m: &Rc<Mod>, env: &Env, e: &Expr) -> Value {
        match e {
            Expr::Paren(p) => self.eval(m, env, &p.expr),
            Expr::TsAs(x) => self.eval(m, env, &x.expr),
            Expr::TsConstAssertion(x) => self.eval(m, env, &x.expr),
            Expr::TsSatisfies(x) => self.eval(m, env, &x.expr),
            Expr::TsTypeAssertion(x) => self.eval(m, env, &x.expr),
            Expr::TsNonNull(x) => self.eval(m, env, &x.expr),
            Expr::TsInstantiation(x) => self.eval(m, env, &x.expr),
            Expr::Lit(Lit::Str(s)) => Value::String(atom(&s.value)),
            Expr::Lit(Lit::Num(n)) => json!(n.value),
            Expr::Lit(Lit::Bool(b)) => Value::Bool(b.value),
            Expr::Lit(Lit::Null(_)) => Value::Null,
            Expr::Tpl(t) if t.exprs.is_empty() => Value::String(
                t.quasis
                    .iter()
                    .filter_map(|q| q.cooked.as_ref().map(atom))
                    .collect(),
            ),
            Expr::Ident(i) => {
                let name = i.sym.to_string();
                match env.get(&name) {
                    Some(v) => v.clone(),
                    None if name == "undefined" => Value::Null,
                    None => self.ident(m, &name),
                }
            }
            Expr::Array(a) => {
                let mut out = vec![];
                for el in a.elems.iter().flatten() {
                    let v = self.eval(m, env, &el.expr);
                    match (el.spread.is_some(), v) {
                        (true, Value::Array(vs)) => out.extend(vs),
                        (_, v) => out.push(v),
                    }
                }
                Value::Array(out)
            }
            Expr::Object(o) => {
                let mut out = Map::new();
                for p in &o.props {
                    match p {
                        PropOrSpread::Spread(s) => {
                            if let Value::Object(src) = self.eval(m, env, &s.expr) {
                                out.extend(src);
                            }
                        }
                        PropOrSpread::Prop(p) => match &**p {
                            Prop::KeyValue(kv) => {
                                if let Some(k) = prop_key(&kv.key) {
                                    let v = self.eval(m, env, &kv.value);
                                    out.insert(k, v);
                                }
                            }
                            Prop::Shorthand(i) => {
                                let v = self.eval(m, env, &Expr::Ident(i.clone()));
                                out.insert(i.sym.to_string(), v);
                            }
                            _ => {}
                        },
                    }
                }
                Value::Object(out)
            }
            Expr::Member(me) => {
                let obj = self.eval(m, env, &me.obj);
                let key = match &me.prop {
                    MemberProp::Ident(i) => Some(i.sym.to_string()),
                    MemberProp::Computed(c) => {
                        self.eval(m, env, &c.expr).as_str().map(String::from)
                    }
                    _ => None,
                };
                match (obj.as_object(), key) {
                    (Some(o), Some(k)) if obj.get(UNRESOLVED).is_none() => {
                        o.get(&k).cloned().unwrap_or(Value::Null)
                    }
                    (_, k) => unresolved(k.unwrap_or_default()),
                }
            }
            Expr::Call(c) => self.call(m, env, c),
            Expr::Arrow(_) | Expr::Fn(_) => Value::Null,
            _ => unresolved("expression"),
        }
    }

    fn call(&mut self, m: &Rc<Mod>, env: &Env, c: &CallExpr) -> Value {
        let Callee::Expr(callee) = &c.callee else {
            return Value::Null;
        };
        let Expr::Ident(id) = &**callee else {
            return unresolved("call()");
        };
        let name = id.sym.to_string();
        let args: Vec<Value> = c.args.iter().map(|a| self.eval(m, env, &a.expr)).collect();
        if let Some((d, local)) = self.callable(m, &name) {
            return self.invoke(&d, &local, args);
        }
        let arg = args.first().cloned().unwrap_or(Value::Null);
        // Well-known helpers from node modules (payload, @payloadcms/plugin-seo).
        match name.as_str() {
            "buildConfig" => arg,
            "slugField" => json!({"name": "slug", "type": "text"}),
            "MetaTitleField" => json!({"name": "title", "type": "text"}),
            "MetaDescriptionField" => json!({"name": "description", "type": "textarea"}),
            "MetaImageField" => json!({
                "name": "image",
                "type": "upload",
                "relationTo": arg.get("relationTo").cloned().unwrap_or(json!("media")),
            }),
            "OverviewField" | "PreviewField" => json!({"type": "ui"}),
            _ => unresolved(format!("{name}()")),
        }
    }

    /// Calls `name` as defined in `d` with evaluated `args`.
    fn invoke(&mut self, d: &Rc<Mod>, name: &str, args: Vec<Value>) -> Value {
        let key = (d.path.clone(), format!("\0invoke:{name}"));
        if self.stack.iter().filter(|k| **k == key).count() > 3 || self.stack.len() > MAX_DEPTH {
            return unresolved(format!("{name}()"));
        }
        self.stack.push(key);
        let f = match d.decls.get(name) {
            Some(Binding::Fn(f)) => Some(Callable::Fn(f)),
            Some(Binding::Expr(e, _)) => match &**e {
                Expr::Arrow(a) => Some(Callable::Arrow(a)),
                Expr::Fn(f) => Some(Callable::Fn(&f.function)),
                _ => None,
            },
            _ => None,
        };
        // Clone the body out so `self` can be borrowed mutably while evaluating it.
        let v = match f {
            Some(Callable::Arrow(a)) => {
                let (params, body) = (a.params.clone(), a.body.clone());
                let mut env = Env::new();
                for (i, p) in params.iter().enumerate() {
                    let arg = args.get(i).cloned().unwrap_or(Value::Null);
                    self.bind(d, &mut env, p, arg);
                }
                match &*body {
                    ArrowFunctionBody::Expr(e) => self.eval(d, &env, e),
                    ArrowFunctionBody::FunctionBody(b) => self.block(d, env, &b.stmts),
                    #[allow(unreachable_patterns)]
                    _ => Value::Null,
                }
            }
            Some(Callable::Fn(f)) => {
                let (params, body) = (f.params.clone(), f.body.clone());
                let mut env = Env::new();
                for (i, p) in params.iter().enumerate() {
                    let arg = args.get(i).cloned().unwrap_or(Value::Null);
                    self.bind(d, &mut env, &p.pat, arg);
                }
                body.map(|b| self.block(d, env, &b.stmts))
                    .unwrap_or(Value::Null)
            }
            None => unresolved(format!("{name}()")),
        };
        self.stack.pop();
        match v.get(UNRESOLVED).and_then(Value::as_str) {
            Some(w) if w.starts_with("statement") => {
                unresolved(format!("{name}(): unsupported {w}"))
            }
            _ => v,
        }
    }

    fn block(&mut self, m: &Rc<Mod>, mut env: Env, stmts: &[Stmt]) -> Value {
        for s in stmts {
            match s {
                Stmt::Decl(Decl::Var(v)) => {
                    for d in &v.decls {
                        let val = d
                            .init
                            .as_ref()
                            .map(|e| self.eval(m, &env, e))
                            .unwrap_or(Value::Null);
                        self.bind(m, &mut env, &d.name, val);
                    }
                }
                Stmt::Return(r) => {
                    return r
                        .arg
                        .as_ref()
                        .map(|e| self.eval(m, &env, e))
                        .unwrap_or(Value::Null);
                }
                Stmt::Empty(_) => {}
                // An `if`, loop or `push` could change what's returned: don't guess.
                s => return unresolved(format!("statement {}", stmt_kind(s))),
            }
        }
        Value::Null
    }

    fn bind(&mut self, m: &Rc<Mod>, env: &mut Env, p: &Pat, v: Value) {
        match p {
            Pat::Ident(b) => {
                env.insert(b.id.sym.to_string(), v);
            }
            Pat::Assign(a) => {
                let v = if v.is_null() {
                    self.eval(m, env, &a.right)
                } else {
                    v
                };
                self.bind(m, env, &a.left, v);
            }
            Pat::Object(o) => {
                for prop in &o.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => {
                            let k = prop_key(&kv.key).unwrap_or_default();
                            let field = v.get(&k).cloned().unwrap_or(Value::Null);
                            self.bind(m, env, &kv.value, field);
                        }
                        ObjectPatProp::Assign(a) => {
                            let k = a.key.id.sym.to_string();
                            let mut field = v.get(&k).cloned().unwrap_or(Value::Null);
                            if field.is_null()
                                && let Some(d) = &a.value
                            {
                                field = self.eval(m, env, d);
                            }
                            env.insert(k, field);
                        }
                        ObjectPatProp::Rest(_) => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn add_collection(&mut self, v: &Value, role: Role) {
        let Some(slug) = v["slug"].as_str().map(String::from) else {
            return;
        };
        let map = if role == Role::Global {
            &self.out.globals
        } else {
            &self.out.collections
        };
        if map.contains_key(&slug) {
            return;
        }
        if let Some(what) = unres(v) {
            self.warn_at(format!(
                "collection `{slug}` is only partly resolved (`{what}`): fields from it may be missing"
            ));
        }
        let fields = self.fields(&v["fields"], &slug);
        let c = CollectionSchema {
            fields,
            upload: truthy(&v["upload"]),
            drafts: truthy(&v["versions"]["drafts"]),
            folders: truthy(&v["folders"]),
        };
        if role == Role::Global {
            self.out.globals.insert(slug, c);
        } else {
            self.out.collections.insert(slug, c);
        }
    }

    /// A block config: its `DestField` (`kind = "block"`), recorded in `blocks` on first sight.
    fn add_block(&mut self, v: &Value) -> Option<DestField> {
        let slug = v["slug"].as_str()?.to_string();
        if let Some(what) = unres(v) {
            self.warn_at(format!(
                "block `{slug}` is only partly resolved (`{what}`): fields from it may be missing"
            ));
        }
        let fields = self.fields(&v["fields"], &format!("block {slug}"));
        self.out
            .blocks
            .entry(slug.clone())
            .or_insert_with(|| fields.clone());
        Some(DestField {
            name: slug,
            kind: "block".into(),
            fields,
            ..DestField::default()
        })
    }

    fn fields(&mut self, v: &Value, ctx: &str) -> Vec<DestField> {
        if let Some(what) = unres(v) {
            self.warn_at(format!("could not resolve the fields of {ctx} (`{what}`)"));
            return vec![];
        }
        if !v.is_null() && !v.is_array() {
            self.warn_at(format!("the fields of {ctx} are not a list"));
            return vec![];
        }
        let mut out = vec![];
        for (i, f) in arr(v).iter().enumerate() {
            let label = f["name"]
                .as_str()
                .map(|n| format!("{ctx}.{n}"))
                .unwrap_or_else(|| format!("{ctx}[{i}]"));
            let Some(kind) = f["type"].as_str() else {
                match unres(f).or_else(|| unres(&f["type"])) {
                    Some(what) => {
                        self.warn_at(format!("could not resolve `{what}` (field {label})"))
                    }
                    None => self.warn_at(format!("could not read the type of field {label}")),
                }
                continue;
            };
            if let Some(what) = unres(f) {
                self.warn_at(format!(
                    "field {label} is only partly resolved (`{what}`): some of its settings may be missing"
                ));
            }
            match kind {
                "row" | "collapsible" => out.extend(self.fields(&f["fields"], ctx)),
                "tabs" => {
                    if let Some(what) = unres(&f["tabs"]) {
                        self.warn_at(format!("could not resolve the tabs of {label} (`{what}`)"));
                    }
                    for (i, tab) in arr(&f["tabs"]).iter().enumerate() {
                        if let Some(what) = unres(tab) {
                            self.warn_at(format!("could not resolve tab `{what}` in {label}"));
                            if tab["fields"].is_null() {
                                continue;
                            }
                        }
                        if tab["fields"].is_null() {
                            self.warn_at(format!("tab {i} of {label} has no fields"));
                        }
                        if let Some(what) = unres(&tab["name"]) {
                            self.warn_at(format!(
                                "could not resolve a tab name in {ctx} (`{what}`)"
                            ));
                        }
                        let inner = self.fields(&tab["fields"], ctx);
                        match tab["name"].as_str() {
                            Some(n) => out.push(DestField {
                                name: n.into(),
                                kind: "group".into(),
                                fields: inner,
                                ..DestField::default()
                            }),
                            None => out.extend(inner),
                        }
                    }
                }
                "ui" | "join" => {}
                _ => {
                    let Some(name) = f["name"].as_str() else {
                        self.warn_at(format!(
                            "could not read the name of the {kind} field {label}"
                        ));
                        continue;
                    };
                    let path = format!("{ctx}.{name}");
                    let fields = match kind {
                        "group" | "array" => self.fields(&f["fields"], &path),
                        "blocks" => {
                            let mut bs = vec![];
                            for b in arr(&f["blocks"]) {
                                if let Some(what) = unres(b) {
                                    self.warn_at(format!(
                                        "could not resolve block `{what}` ({path})"
                                    ));
                                } else if let Some(d) = self.add_block(b) {
                                    bs.push(d);
                                }
                            }
                            for slug in arr(&f["blockReferences"]).iter().filter_map(Value::as_str)
                            {
                                bs.push(DestField {
                                    name: slug.into(),
                                    kind: "block".into(),
                                    fields: self.out.blocks.get(slug).cloned().unwrap_or_default(),
                                    ..DestField::default()
                                });
                            }
                            bs
                        }
                        _ => vec![],
                    };
                    let relation_to: Vec<String> =
                        match &f["relationTo"] {
                            Value::String(s) => vec![s.clone()],
                            Value::Array(a) => {
                                let ok: Vec<String> = a
                                    .iter()
                                    .filter_map(|s| s.as_str().map(String::from))
                                    .collect();
                                if ok.len() < a.len() {
                                    self.warn_at(format!(
                                    "could not resolve every `relationTo` entry of {path} (got {})",
                                    if ok.is_empty() { "none".into() } else { ok.join(", ") }
                                ));
                                }
                                ok
                            }
                            _ => vec![],
                        };
                    let polymorphic = f["relationTo"].is_array();
                    let required = flag(&f["required"]);
                    let many = flag(&f["hasMany"]);
                    if many.is_none() {
                        self.warn_at(format!("could not determine if {path} has many values (`hasMany` is an expression)"));
                    }
                    if !f["relationTo"].is_null() && !polymorphic && relation_to.is_empty() {
                        self.warn_at(format!("could not resolve `relationTo` of {path}"));
                    }
                    if let Some(what) = unres(&f["blocks"]) {
                        self.warn_at(format!("could not resolve the blocks of {path} (`{what}`)"));
                    }
                    out.push(DestField {
                        name: name.into(),
                        kind: kind.into(),
                        required: required.unwrap_or(false),
                        required_unknown: required.is_none(),
                        many: many.unwrap_or(false),
                        relation_to,
                        polymorphic,
                        fields,
                    });
                }
            }
        }
        out
    }
}
