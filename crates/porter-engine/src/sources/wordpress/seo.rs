//! SEO from Yoast (`_yoast_wpseo_*`) or Rank Math (`rank_math_*`) meta, with their title
//! variables substituted; then the REST Yoast head and the crawled `<head>` fill what is left.

use crate::connectors::crawl::PageHead;
use porter_model::{ItemId, Seo};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

/// Site-wide values the variables need.
pub(crate) struct SeoSite<'a> {
    pub site_name: &'a str,
    /// Yoast's `wpseo_titles` option (separator and per-type templates).
    pub titles: &'a Value,
}

impl SeoSite<'_> {
    /// Yoast's separator setting as text (`sc-dash` is Yoast's default).
    pub fn sep(&self) -> &'static str {
        match self.titles["separator"].as_str().unwrap_or("sc-dash") {
            "sc-ndash" => "–",
            "sc-mdash" => "—",
            "sc-colon" => ":",
            "sc-middot" => "·",
            "sc-bull" => "•",
            "sc-star" => "*",
            "sc-smstar" => "⋆",
            "sc-pipe" => "|",
            "sc-tilde" => "~",
            "sc-laquo" => "«",
            "sc-raquo" => "»",
            "sc-lt" => "<",
            "sc-gt" => ">",
            _ => "-",
        }
    }
}

/// What the variables stand for on one page.
pub(crate) struct Vars<'a> {
    pub title: &'a str,
    pub excerpt: &'a str,
}

static YOAST_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"%%([A-Za-z0-9_]+)%%").expect("valid regex"));
static RANKMATH_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"%([A-Za-z0-9_]+)%").expect("valid regex"));

/// Substitutes `%%title%%`, `%%sep%%`, `%%sitename%%`, `%%excerpt%%` and `%%page%%` (Rank Math:
/// `%title%`…); unknown variables are removed and collected in `unknown` (as written).
pub(crate) fn substitute(
    t: &str,
    site: &SeoSite,
    vars: &Vars,
    rank_math: bool,
    unknown: &mut BTreeSet<String>,
) -> Option<String> {
    let re = if rank_math { &RANKMATH_VAR } else { &YOAST_VAR };
    let out = re.replace_all(t, |c: &regex::Captures| match &c[1] {
        "title" => vars.title.to_string(),
        "sep" | "separator" => site.sep().to_string(),
        "sitename" | "site_name" => site.site_name.to_string(),
        "excerpt" => vars.excerpt.to_string(),
        "page" => String::new(),
        _ => {
            unknown.insert(c[0].to_string());
            String::new()
        }
    });
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (!out.is_empty()).then_some(out)
}

fn text(meta: &Map<String, Value>, k: &str) -> Option<String> {
    match meta.get(k)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Array(a) => a.first().and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn id(meta: &Map<String, Value>, k: &str) -> Option<u64> {
    match meta.get(k)? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n| *n > 0)
}

/// Everything one document's SEO is built from.
pub(crate) struct SeoIn<'a> {
    pub post_type: &'a str,
    pub meta: &'a Map<String, Value>,
    pub vars: Vars<'a>,
    /// `yoast_head_json` from REST.
    pub rest_head: Option<&'a Value>,
    pub crawl: Option<&'a PageHead>,
}

/// `media(id_or_url)` resolves an OG image.
pub(crate) fn seo(
    i: &SeoIn,
    site: &SeoSite,
    media_ids: &HashMap<u64, ItemId>,
    media_url: &dyn Fn(&str) -> Option<ItemId>,
    unknown: &mut BTreeSet<String>,
) -> Seo {
    let m = i.meta;
    let yoast = m.keys().any(|k| k.starts_with("_yoast_wpseo_"));
    let rank = !yoast && m.keys().any(|k| k.starts_with("rank_math_"));
    let mut seo = Seo::default();
    let sub = |t: Option<String>, unknown: &mut BTreeSet<String>| {
        t.and_then(|t| substitute(&t, site, &i.vars, rank, unknown))
    };
    let og_image = |idk: &str, urlk: &str| {
        id(m, idk)
            .and_then(|n| media_ids.get(&n).copied())
            .or_else(|| text(m, urlk).and_then(|u| media_url(&u)))
    };
    if yoast {
        seo.title = sub(text(m, "_yoast_wpseo_title"), unknown);
        seo.description = sub(text(m, "_yoast_wpseo_metadesc"), unknown);
        seo.canonical = text(m, "_yoast_wpseo_canonical");
        seo.noindex = text(m, "_yoast_wpseo_meta-robots-noindex").as_deref() == Some("1");
        seo.og_title = sub(text(m, "_yoast_wpseo_opengraph-title"), unknown);
        seo.og_description = sub(text(m, "_yoast_wpseo_opengraph-description"), unknown);
        seo.og_image = og_image(
            "_yoast_wpseo_opengraph-image-id",
            "_yoast_wpseo_opengraph-image",
        );
    } else if rank {
        seo.title = sub(text(m, "rank_math_title"), unknown);
        seo.description = sub(text(m, "rank_math_description"), unknown);
        seo.canonical = text(m, "rank_math_canonical_url");
        seo.noindex = match m.get("rank_math_robots") {
            Some(Value::Array(a)) => a.iter().any(|x| x == "noindex"),
            Some(Value::String(s)) => s.contains("noindex"),
            _ => false,
        };
        seo.og_title = sub(text(m, "rank_math_facebook_title"), unknown);
        seo.og_description = sub(text(m, "rank_math_facebook_description"), unknown);
        seo.og_image = og_image("rank_math_facebook_image_id", "rank_math_facebook_image");
    }
    // Fully rendered heads next: REST's Yoast head, then the live page.
    if let Some(h) = i.rest_head {
        let s = |k: &str| h[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        seo.title = seo.title.or_else(|| s("title"));
        seo.description = seo.description.or_else(|| s("description"));
        seo.canonical = seo.canonical.or_else(|| s("canonical"));
        seo.og_title = seo.og_title.or_else(|| s("og_title"));
        seo.og_description = seo.og_description.or_else(|| s("og_description"));
        seo.noindex |= h["robots"]["index"] == "noindex";
        if seo.og_image.is_none() {
            seo.og_image = h["og_image"][0]["url"].as_str().and_then(media_url);
        }
    }
    if let Some(c) = i.crawl {
        let og = |k: &str| c.og.get(k).filter(|s| !s.is_empty()).cloned();
        seo.title = seo.title.or_else(|| c.title.clone());
        seo.description = seo.description.or_else(|| c.description.clone());
        seo.canonical = seo.canonical.or_else(|| c.canonical.clone());
        seo.og_title = seo.og_title.or_else(|| og("og:title"));
        seo.og_description = seo.og_description.or_else(|| og("og:description"));
        if seo.og_image.is_none() {
            seo.og_image = og("og:image").and_then(|u| media_url(&u));
        }
    }
    // Yoast's per-type templates last: they are what the page would show by default.
    if yoast || !m.keys().any(|k| k.starts_with("rank_math_")) {
        let tpl = |k: String| site.titles[k].as_str().map(str::to_string);
        if seo.title.is_none() {
            seo.title = sub(tpl(format!("title-{}", i.post_type)), unknown);
        }
        if seo.description.is_none() {
            seo.description = sub(tpl(format!("metadesc-{}", i.post_type)), unknown);
        }
        seo.noindex |= site.titles[format!("noindex-{}", i.post_type)] == true;
    }
    seo
}

/// Site-wide defaults: Yoast's home-page title and description.
pub(crate) fn defaults(site: &SeoSite, tagline: &str, unknown: &mut BTreeSet<String>) -> Seo {
    let vars = Vars {
        title: site.site_name,
        excerpt: tagline,
    };
    let mut t = |k: &str| {
        site.titles[k]
            .as_str()
            .and_then(|t| substitute(t, site, &vars, false, unknown))
    };
    Seo {
        title: t("title-home-wpseo"),
        description: t("metadesc-home-wpseo"),
        ..Seo::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yoast_and_rank_math_vars() {
        let titles = json!({"separator": "sc-pipe"});
        let site = SeoSite {
            site_name: "Site",
            titles: &titles,
        };
        let vars = Vars {
            title: "About",
            excerpt: "Short",
        };
        let mut unknown = BTreeSet::new();
        assert_eq!(
            substitute(
                "%%title%% %%page%% %%sep%% %%sitename%% %%x%%",
                &site,
                &vars,
                false,
                &mut unknown
            )
            .as_deref(),
            Some("About | Site")
        );
        assert_eq!(
            substitute(
                "%excerpt% %sep% %sitename%",
                &site,
                &vars,
                true,
                &mut unknown
            )
            .as_deref(),
            Some("Short | Site")
        );
        assert_eq!(unknown.into_iter().collect::<Vec<_>>(), ["%%x%%"]);
    }
}
