//! Merges what the WordPress connectors found: matches content to live URLs, writes the
//! coverage report and opens the issues the user needs to see.

use crate::config::ConnectorConfig;
use crate::connectors::RawItem;
use crate::connectors::crawl::{Detected, PageHead};
use crate::report::{ConnectorCoverage, Coverage, write_coverage};
use crate::stage::StageCtx;
use crate::store::IssueFilter;
use crate::url::normalise_url;
use anyhow::Result;
use porter_model::{IssueKind, ItemKind, Resolution, Route, Severity, Stage};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const KINDS: [ItemKind; 11] = [
    ItemKind::Document,
    ItemKind::Media,
    ItemKind::Route,
    ItemKind::Redirect,
    ItemKind::Menu,
    ItemKind::Taxonomy,
    ItemKind::Term,
    ItemKind::Form,
    ItemKind::Submission,
    ItemKind::Settings,
    ItemKind::Author,
];

/// Post types that are not public content.
const NON_PUBLIC: [&str; 14] = [
    "attachment",
    "nav_menu_item",
    "revision",
    "wp_block",
    "wp_template",
    "wp_template_part",
    "wp_navigation",
    "wp_global_styles",
    "custom_css",
    "customize_changeset",
    "oembed_cache",
    "user_request",
    "acf-field",
    "acf-field-group",
];

/// Extensions that mark a live URL as a file rather than a page.
const FILE_EXTS: [&str; 6] = ["html", "htm", "php", "asp", "aspx", "xml"];

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct MergeReport {
    /// Published content (URL, or `label:id` when it has none) with no live route.
    pub only_in_source: Vec<String>,
    /// Live 2xx pages no source content matches.
    pub only_live: Vec<String>,
    pub matched: usize,
}

/// One WordPress post as the connectors saw it; connectors describing the same id count once.
#[derive(Default)]
struct Post {
    label: String,
    published: bool,
    keys: BTreeSet<String>,
    shown: Option<String>,
}

fn wp_id(connector: &str, source_id: &str) -> Option<String> {
    let id = match connector {
        "wxr" => source_id,
        _ => source_id.rsplit_once(':')?.1,
    };
    id.parse::<u64>().ok().map(|n| n.to_string())
}

fn body_ids(classes: &[String]) -> Vec<String> {
    classes
        .iter()
        .filter_map(|c| {
            let n = c
                .strip_prefix("postid-")
                .or_else(|| c.strip_prefix("page-id-"))?;
            n.parse::<u64>().ok().map(|n| n.to_string())
        })
        .collect()
}

fn is_file(key: &str) -> bool {
    key.split_once('/')
        .map_or("", |(_, path)| path)
        .rsplit('/')
        .find(|s| !s.is_empty())
        .and_then(|s| s.rsplit_once('.'))
        .is_some_and(|(stem, ext)| {
            !stem.is_empty() && !FILE_EXTS.contains(&ext.to_lowercase().as_str())
        })
}

/// Slug of a plugin path such as `elementor/elementor.php`.
fn plugin_slug(p: &str) -> String {
    let p = p.split('/').next().unwrap_or(p);
    p.strip_suffix(".php").unwrap_or(p).to_string()
}

fn read_json<T: serde::de::DeserializeOwned>(p: &std::path::Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

pub fn merge(ctx: &StageCtx) -> Result<MergeReport> {
    let project = &ctx.project;
    let store = project.store();
    let raw = |c: &str| project.raw_dir(c);

    // Posts from the content connectors, one per WordPress id.
    let mut posts: BTreeMap<String, Post> = BTreeMap::new();
    let mut have_content = false;
    for (_, src, row) in store.items::<RawItem>(ItemKind::Document)? {
        if src.connector == "crawl" || NON_PUBLIC.contains(&row.label.as_str()) {
            continue;
        }
        have_content = true;
        let key = wp_id(&src.connector, &src.source_id)
            .unwrap_or_else(|| format!("{}/{}", src.connector, src.source_id));
        let p = posts.entry(key.clone()).or_default();
        p.published |= row.status.as_deref() == Some("publish");
        if p.label.is_empty() {
            p.label = row.label.clone();
        }
        if let Some(u) = row.url.as_deref().filter(|u| !u.is_empty()) {
            p.shown.get_or_insert_with(|| u.to_string());
            p.keys.extend(normalise_url(u, None).map(|k| k.0));
        }
        p.shown
            .get_or_insert_with(|| format!("{}:{key}", row.label));
    }

    // Live pages (2xx only) and the WordPress ids their body classes name.
    let routes: Vec<(String, String)> = store
        .items::<Route>(ItemKind::Route)?
        .into_iter()
        .filter(|(_, src, r)| {
            src.connector == "crawl" && r.http_status.is_some_and(|s| (200..300).contains(&s))
        })
        .map(|(_, src, r)| (src.source_id, r.url))
        .filter(|(k, _)| !is_file(k))
        .collect();
    let have_routes = !routes.is_empty();
    let mut page_ids: HashMap<String, Vec<String>> = HashMap::new();
    if let Ok(dir) = std::fs::read_dir(raw("crawl").join("pages")) {
        for e in dir.flatten() {
            let Some(h) = read_json::<PageHead>(&e.path()) else {
                continue;
            };
            if let Some(k) = normalise_url(&h.url, None) {
                page_ids
                    .entry(k.0)
                    .or_default()
                    .extend(body_ids(&h.body_classes));
            }
        }
    }

    let route_keys: BTreeSet<&str> = routes.iter().map(|(k, _)| k.as_str()).collect();
    let mut matched_routes: BTreeSet<&str> = BTreeSet::new();
    let mut report = MergeReport::default();
    for (id, p) in posts.iter().filter(|(_, p)| p.published) {
        let mut hit: Vec<&str> = p
            .keys
            .iter()
            .filter_map(|k| route_keys.get(k.as_str()).copied())
            .collect();
        hit.extend(
            routes
                .iter()
                .filter(|(k, _)| page_ids.get(k).is_some_and(|ids| ids.contains(id)))
                .map(|(k, _)| k.as_str()),
        );
        if hit.is_empty() {
            report
                .only_in_source
                .push(p.shown.clone().unwrap_or_default());
        } else {
            report.matched += 1;
            matched_routes.extend(hit);
        }
    }
    // Without live data nothing is "only in the source"; without content nothing is "only live".
    if !have_routes {
        report.only_in_source.clear();
    }
    if have_content {
        report.only_live = routes
            .iter()
            .filter(|(k, _)| {
                !matched_routes.contains(k.as_str())
                    && k.split_once('/').is_some_and(|(_, p)| !p.is_empty())
            })
            .map(|(_, u)| u.clone())
            .collect();
    }

    // Coverage.
    let detected: Detected = read_json(&raw("crawl").join("detect.json")).unwrap_or_default();
    let db_plugins: Vec<String> = read_json::<serde_json::Value>(&raw("wp-db").join("export.json"))
        .and_then(|v| {
            v["site"]["active_plugins"].as_array().map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(plugin_slug))
                    .collect()
            })
        })
        .unwrap_or_default();

    let cfg_names: Vec<&str> = ctx
        .config
        .connectors
        .iter()
        .filter_map(|c| match c {
            ConnectorConfig::WpDb { .. } => Some("wp-db"),
            ConnectorConfig::WpRest { .. } => Some("wp-rest"),
            ConnectorConfig::Wxr { .. } => Some("wxr"),
            ConnectorConfig::Crawl { .. } => Some("crawl"),
            _ => None,
        })
        .collect();
    let mut contributed: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for kind in KINDS {
        for (_, src, data) in store.items::<serde_json::Value>(kind)? {
            let label = match (src.connector.as_str(), data["label"].as_str()) {
                ("crawl", _) | (_, None) => kind.as_str(),
                (_, Some(l)) => l,
            };
            *contributed
                .entry(src.connector)
                .or_default()
                .entry(label.to_string())
                .or_default() += 1;
        }
    }
    let gaps = store.issues(&IssueFilter {
        stage: Some(Stage::Analyse),
        kind: Some(IssueKind::AccessGap),
        resolution: Some(Resolution::Open),
        ..Default::default()
    })?;
    let order = ["wp-db", "wp-rest", "wxr", "crawl"];
    let connectors: Vec<ConnectorCoverage> = order
        .iter()
        .filter(|n| cfg_names.contains(n) || contributed.contains_key(**n))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|n| ConnectorCoverage {
            name: n.to_string(),
            contributed: contributed.remove(*n).unwrap_or_default(),
            gaps: gaps
                .iter()
                .filter(|i| i.details["connector"] == *n)
                .map(|i| i.message.clone())
                .collect(),
        })
        .collect();
    let ran_names: Vec<String> = connectors.iter().map(|c| c.name.clone()).collect();
    let ran = |n: &str| ran_names.iter().any(|c| c == n);
    let mut plugins: BTreeSet<String> = detected.plugins.iter().cloned().collect();
    plugins.extend(db_plugins.iter().cloned());
    let coverage = Coverage {
        platform: detected
            .platform
            .clone()
            .or_else(|| (!connectors.is_empty()).then(|| "wordpress".to_string())),
        builders: detected.builders.clone(),
        plugins: plugins.iter().cloned().collect(),
        connectors,
    };

    // converter-missing: builders and plugins whose content we keep as raw blocks.
    let has = |names: &[&str]| {
        detected
            .builders
            .iter()
            .chain(&plugins)
            .any(|d| names.contains(&d.as_str()))
    };
    for (label, names) in [
        ("Elementor", &["elementor", "elementor-pro"][..]),
        ("WPBakery", &["wpbakery", "js_composer"][..]),
        ("Smart Slider", &["smart-slider", "smart-slider-3"][..]),
    ] {
        if has(names) {
            ctx.issue(
                IssueKind::ConverterMissing,
                Severity::Warning,
                None,
                format!(
                    "{label} detected: no converter yet, its content will be kept as raw blocks"
                ),
                serde_json::json!({ "builder": label }),
            )?;
        }
    }

    // access-gap: kinds of data nothing configured can reach.
    let authed_rest = ctx.config.connectors.iter().any(|c| {
        matches!(c, ConnectorConfig::WpRest { auth, keyring, .. } if auth.is_some() || keyring.is_some())
    }) || (ran("wp-rest") && std::env::var_os("PORTER_WP_AUTH_TYPE").is_some());
    let gap = |msg: &str, what: &str| {
        ctx.issue(
            IssueKind::AccessGap,
            Severity::Info,
            None,
            msg,
            serde_json::json!({ "missing": what }),
        )
    };
    if !ran("wp-db") && !authed_rest {
        gap(
            "form definitions and submissions unavailable: needs a database dump or authenticated REST access",
            "forms",
        )?;
    }
    if !ran("crawl") {
        gap(
            "live URL statuses unknown: no crawl connector configured",
            "live-urls",
        )?;
    }
    if !have_content {
        gap(
            "no source content: only live pages are known (add a WXR export, REST or database connector)",
            "content",
        )?;
    }

    write_coverage(project, &coverage)?;
    let dir = raw("analysis");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("merge.json"), serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}
