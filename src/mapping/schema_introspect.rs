use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
        // Assume project root has ./src
        let root = std::env::current_dir().ok()?;
        let p = root.join("src").join(&source[2..]);
        let with_ts = try_with_extensions(&p);
        return with_ts.map(|pb| pb.to_string_lossy().to_string());
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

fn expr_to_json(expr: &Expr) -> Option<Value> {
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
                    if let Some(v) = expr_to_json(&e.expr) { out.push(v); }
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
                        if let Some(v) = expr_to_json(&kv.value) { map.insert(key, v); }
                    }
                }
            }
            Some(Value::Object(map))
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
                            if let Some(ret) = find_function_return_object(&m, &name) {
                                return resolve_expr(&ret, path, &build_import_map(&m, path));
                            }
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

pub fn extract_fields_json(schema_path: &str) -> Result<Vec<Value>> {
    let module = parse_module(schema_path)?;
    let import_map = build_import_map(&module, schema_path);

    // Find a top-level object with a 'fields' property
    // Naively scan ExportDecls/VarDecls
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
                                                        if let Some(e) = el { if let Some(v) = resolve_expr(&e.expr, schema_path, &import_map) { out.push(v); } }
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

    Err(anyhow!("Could not find fields array in schema: {}", schema_path))
}


