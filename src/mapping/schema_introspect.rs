use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use crate::config::PorterConfig;
use crate::dlog;
use swc_common::sync::Lrc;
use swc_common::{errors::ColorConfig, errors::Handler, SourceMap};
use swc_ecma_ast::*;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsConfig};

fn parse_module(file_path: &str) -> Result<Module> {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
    let content = crate::util::fs::read_file(file_path)
        .with_context(|| format!("Failed to read TypeScript file: {}", file_path))?;
    let fm = cm.new_source_file(swc_common::FileName::Custom(file_path.into()), content);
    let lexer = Lexer::new(
        Syntax::Typescript(TsConfig { tsx: false, decorators: true, dts: false, no_early_errors: false, disallow_ambiguous_jsx_like: false }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|e| {
            e.into_diagnostic(&handler).emit();
            anyhow!("Failed to parse TypeScript file: {}", file_path)
        })
}

fn resolve_import_path(current_file: &str, source: &str) -> Option<String> {
    // Relative paths
    if source.starts_with("./") || source.starts_with("../") {
        let base = Path::new(current_file).parent()?;
        let p = base.join(source);
        let with_ts = try_with_extensions(&p);
        return with_ts.map(|pb| pb.to_string_lossy().to_string());
    }
    // Simple alias '@/'
    if source.starts_with("@/") {
        // Find the nearest ancestor that IS the 'src' directory and resolve from there
        let mut dir = Path::new(current_file).parent();
        while let Some(d) = dir {
            if let Some(name) = d.file_name().and_then(|s| s.to_str()) {
                if name == "src" {
                    let candidate = d.join(&source[2..]);
                    if let Some(res) = try_with_extensions(&candidate) {
                        return Some(res.to_string_lossy().to_string());
                    }
                }
            }
            dir = d.parent();
        }
        // Fallback to CWD/src
        if let Ok(root) = std::env::current_dir() {
            let p = root.join("src").join(&source[2..]);
            if let Some(res) = try_with_extensions(&p) {
                return Some(res.to_string_lossy().to_string());
            }
        }
    }
    // Resolve via configured tsconfig paths aliases if available
    if let Some(cfg) = TS_PATHS.get() {
        if let Some(resolved) = resolve_via_ts_paths(cfg, source) {
            return Some(resolved);
        }
    }
    None
}

fn try_with_extensions(path: &Path) -> Option<PathBuf> {
    let candidates = [
        path.with_extension("ts"),
        path.with_extension("tsx"),
        path.with_extension("js"),
        path.with_extension("mjs"),
        path.with_extension("cjs"),
        PathBuf::from(path),
        path.join("index.ts"),
        path.join("index.tsx"),
        path.join("index.js"),
    ];
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    None
}

fn expr_to_json(expr: &Expr) -> Option<Value> { expr_to_json_ctx(expr, None, None) }

fn expr_to_json_ctx(expr: &Expr, current_file: Option<&str>, import_map: Option<&HashMap<String, String>>) -> Option<Value> {
    match expr {
        Expr::Lit(lit) => match lit {
            Lit::Str(s) => Some(Value::String(s.value.to_string())),
            Lit::Bool(b) => Some(Value::Bool(b.value)),
            Lit::Num(n) => serde_json::Number::from_f64(n.value).map(Value::Number),
            Lit::Null(_) => Some(Value::Null),
            _ => None,
        },
        Expr::Array(arr) => {
            let mut out = Vec::new();
            for el in &arr.elems {
                if let Some(e) = el {
                    if let Some(v) = expr_to_json_ctx(&e.expr, current_file, import_map) { out.push(v); }
                }
            }
            Some(Value::Array(out))
        }
        Expr::Object(obj) => {
            let mut map = serde_json::Map::new();
            for prop in &obj.props {
                if let PropOrSpread::Prop(p) = prop {
                    if let Prop::KeyValue(kv) = &**p {
                        let key = match &kv.key { PropName::Ident(i) => i.sym.to_string(), PropName::Str(s) => s.value.to_string(), _ => continue };
                        if let Some(v) = expr_to_json_ctx(&kv.value, current_file, import_map) { map.insert(key, v); }
                    }
                }
            }
            Some(Value::Object(map))
        }
        Expr::Ident(id) => {
            if let (Some(_file), Some(map)) = (current_file, import_map) {
                let name = id.sym.to_string();
                if let Some(path) = map.get(&name) {
                    if let Ok(m) = parse_module(path) {
                        if let Some(e) = find_export_expr_by_name(&m, &name).or_else(|| find_default_export_expr(&m)) {
                            let next_map = build_import_map(&m, path);
                            return expr_to_json_ctx(&e, Some(path), Some(&next_map));
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn build_import_map(module: &Module, file_path: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            let src = import.src.value.to_string();
            let Some(resolved) = resolve_import_path(file_path, &src) else { continue };
            for s in &import.specifiers {
                match s {
                    ImportSpecifier::Named(named) => {
                        let local = named.local.sym.to_string();
                        map.insert(local, resolved.clone());
                    }
                    ImportSpecifier::Default(def) => {
                        let local = def.local.sym.to_string();
                        map.insert(local, resolved.clone());
                    }
                    ImportSpecifier::Namespace(ns) => {
                        let local = ns.local.sym.to_string();
                        map.insert(local, resolved.clone());
                    }
                }
            }
        }
    }
    map
}

fn find_export_expr_by_name(module: &Module, name: &str) -> Option<Expr> {
    // Look for export const name = {...}
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ed)) => {
                if let Decl::Var(var) = &ed.decl {
                    for d in &var.decls {
                        if let Pat::Ident(bi) = &d.name {
                            if bi.id.sym == *name {
                                if let Some(init) = &d.init { return Some((**init).clone()); }
                            }
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for d in &var.decls {
                    if let Pat::Ident(bi) = &d.name {
                        if bi.id.sym == *name {
                            if let Some(init) = &d.init { return Some((**init).clone()); }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn find_default_export_expr(module: &Module) -> Option<Expr> {
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(e)) = item {
            return Some((*e.expr).clone());
        }
    }
    None
}

fn find_function_return_object(module: &Module, name: &str) -> Option<Expr> {
    for item in &module.body {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(f))) = item {
            if f.ident.sym == *name {
                // naive: find first return statement with object literal
                let mut obj: Option<Expr> = None;
                fn walk_stmt(obj: &mut Option<Expr>, s: &Stmt) {
                    match s {
                        Stmt::Return(ret) => {
                            if let Some(arg) = &ret.arg {
                                if matches!(&**arg, Expr::Object(_)) { *obj = Some((**arg).clone()); }
                            }
                        }
                        Stmt::Block(b) => { for st in &b.stmts { walk_stmt(obj, st); } }
                        _ => {}
                    }
                }
                for st in &f.function.body.as_ref()?.stmts { walk_stmt(&mut obj, st); }
                if obj.is_some() { return obj; }
            }
        }
    }
    None
}

fn resolve_expr(expr: &Expr, current_file: &str, import_map: &HashMap<String, String>) -> Option<Value> {
    match expr {
        Expr::Object(_) | Expr::Array(_) | Expr::Lit(_) => expr_to_json(expr),
        Expr::Ident(id) => {
            let name = id.sym.to_string();
            if let Some(path) = import_map.get(&name) {
                if let Ok(m) = parse_module(path) {
                    if let Some(e) = find_export_expr_by_name(&m, &name).or_else(|| find_default_export_expr(&m)) {
                        return resolve_expr(&e, path, &build_import_map(&m, path));
                    }
                }
            }
            None
        }
        Expr::Call(call) => {
            // handle factory like slugField()
            if let Callee::Expr(callee_expr) = &call.callee {
                if let Expr::Ident(id) = &**callee_expr {
                    let name = id.sym.to_string();
                    if let Some(path) = import_map.get(&name) {
                        if let Ok(m) = parse_module(path) {
                            // 1) Try function declaration matching the name
                            if let Some(ret) = find_function_return_object(&m, &name) {
                                let resolved = resolve_expr(&ret, path, &build_import_map(&m, path));
                                return resolved;
                            }
                            // 2) Try exported const/let with arrow/fn expression and extract return
                            if let Some(init_expr) = find_export_expr_by_name(&m, &name).or_else(|| find_default_export_expr(&m)) {
                                if let Some(ret_expr) = extract_return_from_function_expr(&init_expr) {
                                    let resolved = resolve_expr(&ret_expr, path, &build_import_map(&m, path));
                                    return resolved;
                                }
                            }
                        }
                    }
                }
            }
            // Also resolve inline object literal returns
            if let Some(arg0) = call.args.get(0) {
                if let Some(v) = expr_to_json_ctx(&arg0.expr, Some(current_file), Some(import_map)) { return Some(v); }
            }
            None
        }
        _ => None,
    }
}

pub fn extract_fields_json(schema_path: &str) -> Result<Vec<Value>> {
    let module = parse_module(schema_path)?;
    let import_map = build_import_map(&module, schema_path);

    // 1) Find a top-level object with a 'fields' property (collection or field config)
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ed)) = item {
            if let Decl::Var(var) = &ed.decl {
                for d in &var.decls {
                    if let Some(init) = &d.init {
                        if let Expr::Object(obj) = &**init {
                            for prop in &obj.props {
                                if let PropOrSpread::Prop(p) = prop {
                                    if let Prop::KeyValue(kv) = &**p {
                                        if let PropName::Ident(id) = &kv.key {
                                            if id.sym == *"fields" {
                                                if let Expr::Array(arr) = &*kv.value {
                                                    let mut out = Vec::new();
                                                    for el in &arr.elems {
                                                        if let Some(e) = el {
                                                            // Resolve identifiers/calls; otherwise attempt JSON fallback
                                                            if let Some(v) = resolve_expr(&e.expr, schema_path, &import_map)
                                                                .or_else(|| expr_to_json_ctx(&e.expr, Some(schema_path), Some(&import_map))) {
                                                                out.push(v);
                                                            }
                                                        }
                                                    }
                                                    return Ok(out);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2) If no direct export found, scan imports used in this file's fields arrays references
    // Look for array literals assigned to local identifiers used in export as 'fields: [hero]' etc.
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ed)) = item {
            if let Decl::Var(var) = &ed.decl {
                for d in &var.decls {
                    if let Some(init) = &d.init {
                        if let Expr::Object(obj) = &**init {
                            // find 'fields: [<ident or call>]' and resolve each
                            for prop in &obj.props {
                                if let PropOrSpread::Prop(p) = prop {
                                    if let Prop::KeyValue(kv) = &**p {
                                        if let PropName::Ident(id) = &kv.key {
                                            if id.sym == *"fields" {
                                                if let Expr::Array(arr) = &*kv.value {
                                                    let mut out = Vec::new();
                                                    for el in &arr.elems {
                                                        if let Some(e) = el {
                                                            if let Some(v) = resolve_expr(&e.expr, schema_path, &import_map) {
                                                                match v {
                                                                    Value::Array(items) => out.extend(items),
                                                                    Value::Object(_) => out.push(v),
                                                                    _ => {}
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if !out.is_empty() { return Ok(out); }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Err(anyhow!("Could not find fields array in schema: {}", schema_path))
}

fn extract_return_from_function_expr(expr: &Expr) -> Option<Expr> {
    match expr {
        Expr::Fn(fn_expr) => {
            if let Some(body) = &fn_expr.function.body {
                for st in &body.stmts {
                    if let Stmt::Return(ret) = st {
                        if let Some(arg) = &ret.arg { return Some((**arg).clone()); }
                    }
                }
            }
            None
        }
        Expr::Arrow(arrow) => {
            match &*arrow.body {
                swc_ecma_ast::BlockStmtOrExpr::Expr(e) => Some((**e).clone()),
                swc_ecma_ast::BlockStmtOrExpr::BlockStmt(block) => {
                    for st in &block.stmts {
                        if let Stmt::Return(ret) = st {
                            if let Some(arg) = &ret.arg { return Some((**arg).clone()); }
                        }
                    }
                    None
                }
            }
        }
        _ => None,
    }
}

// === TypeScript tsconfig paths support ===

#[derive(Debug)]
struct TsPathsConfig {
    base_dir: PathBuf,
    // e.g. "@/*" => ["./src/*"]
    mappings: Vec<(String, Vec<String>)>,
}

static TS_PATHS: OnceLock<TsPathsConfig> = OnceLock::new();

fn resolve_via_ts_paths(cfg: &TsPathsConfig, source: &str) -> Option<String> {
    for (alias, targets) in &cfg.mappings {
        // Exact match (no wildcard)
        if !alias.contains('*') {
            if alias == source {
                dlog!("TS alias exact match: {} => {:?}", alias, targets);
                for t in targets {
                    let candidate = cfg.base_dir.join(t);
                    if let Some(res) = try_with_extensions(&candidate) {
                        return Some(res.to_string_lossy().to_string());
                    }
                }
            }
            continue;
        }

        // Wildcard pattern like "@/*" or "@/lib/*"
        let parts: Vec<&str> = alias.split('*').collect();
        let (prefix, suffix) = match parts.as_slice() {
            [p, s] => (*p, *s),
            [p] => (*p, ""),
            _ => ("", ""),
        };

        if source.starts_with(prefix) && source.ends_with(suffix) && source.len() >= prefix.len() + suffix.len() {
            let middle = &source[prefix.len()..source.len() - suffix.len()];
            dlog!("TS alias wildcard: {} -> middle='{}' targets={:?}", alias, middle, targets);
            for t in targets {
                let replaced = if t.contains('*') { t.replace('*', middle) } else { t.clone() };
                let candidate = cfg.base_dir.join(replaced);
                if let Some(res) = try_with_extensions(&candidate) {
                    return Some(res.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

/// Configure tsconfig paths resolution globally. Safe to call multiple times; only first wins.
pub fn configure_ts_paths_from_file(tsconfig_path: &str) -> Result<()> {
    let tsconfig_path = PathBuf::from(tsconfig_path);
    if !tsconfig_path.exists() { return Ok(()); }
    let raw = crate::util::fs::read_file(tsconfig_path.to_str().unwrap())?;
    // Support standard JSON and JSONC-like via json5
    let json: Value = serde_json::from_str(&raw)
        .or_else(|_| json5::from_str(&raw))
        .unwrap_or(Value::Null);

    let compiler_options = json.get("compilerOptions").cloned().unwrap_or(Value::Null);
    let base_url = compiler_options.get("baseUrl").and_then(Value::as_str).unwrap_or(".");
    let base_dir = tsconfig_path.parent().unwrap_or(Path::new(".")).join(base_url);
    let base_dir = base_dir;

    let mut mappings: Vec<(String, Vec<String>)> = Vec::new();
    if let Some(paths_obj) = compiler_options.get("paths").and_then(Value::as_object) {
        for (alias, targets_val) in paths_obj.iter() {
            let targets: Vec<String> = match targets_val {
                Value::Array(arr) => arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect(),
                Value::String(s) => vec![s.clone()],
                _ => Vec::new(),
            };
            if !targets.is_empty() {
                mappings.push((alias.clone(), targets));
            }
        }
    }

    let _ = TS_PATHS.set(TsPathsConfig { base_dir, mappings });
    Ok(())
}

/// Configure ts paths directly from already-parsed config (preferred precise mappings)
pub fn configure_ts_paths_from_porter(cfg: &PorterConfig) {
    if let Some(ts) = &cfg.typescript {
        if let Some(pm) = &ts.path_mappings {
            let base = std::path::Path::new(&ts.tsconfig_path).parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
            let mappings: Vec<(String, Vec<String>)> = pm.iter().map(|m| (m.alias.clone(), m.paths.clone())).collect();
            let _ = TS_PATHS.set(TsPathsConfig { base_dir: base, mappings });
        }
    }
}


