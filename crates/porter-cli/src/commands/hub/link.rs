use super::{Ctx, check_slug, origin};
use crate::output::print_json;
use anyhow::{Context, Result, anyhow};
use toml_edit::{DocumentMut, Item, Table, value};

/// Writes `[hub] url, project` into porter.toml, keeping comments and layout.
pub fn run(ctx: &Ctx, slug: &str, url: Option<String>) -> Result<i32> {
    check_slug(slug)?;
    let url = url.ok_or_else(|| anyhow!("pass --url <hub> (porter.toml has no [hub] url yet)"))?;
    let o = origin(&url)?.origin().ascii_serialization();
    let path = ctx.dir.join("porter.toml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;
    let hub = doc
        .entry("hub")
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| anyhow!("porter.toml: `hub` is not a table"))?;
    hub.insert("url", value(o.clone()));
    hub.insert("project", value(slug));
    porter_engine::mapping::edit::write_atomic(&path, &doc.to_string())?;
    if ctx.json {
        print_json(&serde_json::json!({"v": 1, "event": "hub-link", "hub": o, "project": slug}));
    } else {
        eprintln!("linked to {slug} on {o}");
    }
    Ok(0)
}
