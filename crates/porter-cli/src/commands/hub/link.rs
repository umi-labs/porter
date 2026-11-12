use super::{Ctx, Hub, check_slug, hub_error, origin};
use crate::output::print_json;
use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Method, StatusCode};
use toml_edit::{DocumentMut, Item, Table, value};

/// Writes `[hub] url, project` into porter.toml, keeping comments and layout. With
/// `create` (`Some(name)`), first creates the project on the hub (`POST /api/projects`).
pub async fn run(
    ctx: &Ctx,
    slug: &str,
    url: Option<String>,
    create: Option<Option<String>>,
) -> Result<i32> {
    check_slug(slug)?;
    let url = url.ok_or_else(|| anyhow!("pass --url <hub> (porter.toml has no [hub] url yet)"))?;
    let hub_origin = origin(&url)?;
    let o = hub_origin.origin().ascii_serialization();
    let path = ctx.dir.join("porter.toml");
    if !path.exists() {
        bail!("no porter.toml in {}", ctx.dir.display());
    }
    let created = create.is_some();
    if let Some(name) = create {
        let hub = Hub::new(hub_origin, slug.to_string())?;
        let body = serde_json::json!({ "name": name.as_deref().unwrap_or(slug), "slug": slug });
        let (st, v) = hub
            .call(Method::POST, &["api", "projects"], Some(&body))
            .await?;
        if st != StatusCode::CREATED {
            bail!("{}", hub_error(st, &v));
        }
        if !ctx.json {
            eprintln!("created {slug} on {o}");
        }
    }
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
        print_json(&serde_json::json!({
            "v": 1, "event": "hub-link", "hub": o, "project": slug, "created": created,
        }));
    } else {
        eprintln!("linked to {slug} on {o}");
    }
    Ok(0)
}
