//! Redirects from every source, deduped by `from` key (Manual > Plugin > Crawl), with chains
//! collapsed to their final target and loops dropped; and the route table every old URL ends
//! up in.

use super::load::Raw;
use super::normalise::{Out, Skeletons, canon, key_path, url_key};
use crate::url::normalise_url;
use ::url::Url;
use anyhow::Result;
use porter_model::{
    IssueKind, ItemId, ItemKind, ItemStatus, Redirect, RedirectOrigin, Route, RouteTarget, Severity,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Hops followed before a chain is treated as endless.
const MAX_HOPS: usize = 50;

/// One redirect as a source states it.
#[derive(Clone, Debug)]
pub(crate) struct Input {
    pub from: String,
    pub to: String,
    pub code: u16,
    pub origin: RedirectOrigin,
    /// Connector row it came from (none for config).
    pub raw: Option<ItemId>,
    /// Set when the row can't be represented as a plain redirect.
    pub skip: Option<String>,
}

/// What happens to one `from` key.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    /// Kept; `hops` > 1 means a chain was collapsed into it.
    Kept {
        redirect: Redirect,
        hops: usize,
    },
    /// Every path from here comes back round; `chain` lists the keys.
    Loop {
        chain: Vec<String>,
    },
    Skipped {
        reason: String,
    },
}

#[derive(Debug)]
pub(crate) struct Planned {
    /// `from` key (or the raw `from` when it has no key).
    pub key: String,
    pub outcome: Outcome,
    /// Rows that described this `from` (winner first).
    pub raw: Vec<ItemId>,
}

fn rank(o: RedirectOrigin) -> u8 {
    match o {
        RedirectOrigin::Manual => 0,
        RedirectOrigin::Plugin => 1,
        RedirectOrigin::Field => 2,
        RedirectOrigin::Crawl => 3,
    }
}

pub(crate) fn key(u: &str, base: &Url) -> Option<String> {
    normalise_url(u, Some(base)).map(|k| k.0)
}

/// Dedupes, collapses chains and finds loops. `base` resolves relative URLs.
pub(crate) fn plan(inputs: Vec<Input>, base: &Url) -> Vec<Planned> {
    let mut best: BTreeMap<String, (Input, Vec<ItemId>)> = BTreeMap::new();
    let mut skipped: BTreeMap<String, (String, Vec<ItemId>)> = BTreeMap::new();
    for i in inputs {
        let k = key(&i.from, base).unwrap_or_else(|| i.from.clone());
        if let Some(reason) = i.skip.clone() {
            let e = skipped.entry(k).or_insert_with(|| (reason, vec![]));
            e.1.extend(i.raw);
            continue;
        }
        match best.get_mut(&k) {
            Some((cur, raw)) => {
                raw.extend(i.raw);
                if rank(i.origin) < rank(cur.origin) {
                    *cur = i;
                }
            }
            None => {
                let raw = i.raw.into_iter().collect();
                best.insert(k, (i, raw));
            }
        }
    }
    let next: HashMap<&str, &Input> = best.iter().map(|(k, (i, _))| (k.as_str(), i)).collect();
    let mut out = vec![];
    for (from, (input, raw)) in &best {
        let mut chain = vec![from.clone()];
        let mut target = input;
        let outcome = loop {
            let Some(tk) = key(&target.to, base) else {
                break Outcome::Kept {
                    redirect: redirect(input, &target.to),
                    hops: chain.len(),
                };
            };
            if chain.contains(&tk) {
                chain.push(tk);
                break Outcome::Loop { chain };
            }
            match next.get(tk.as_str()) {
                Some(n) if chain.len() < MAX_HOPS => {
                    chain.push(tk);
                    target = n;
                }
                _ => {
                    break Outcome::Kept {
                        redirect: redirect(input, &target.to),
                        hops: chain.len(),
                    };
                }
            }
        };
        out.push(Planned {
            key: from.clone(),
            outcome,
            raw: raw.clone(),
        });
    }
    let at: HashMap<String, usize> = out
        .iter()
        .enumerate()
        .map(|(i, p)| (p.key.clone(), i))
        .collect();
    for (k, (reason, raw)) in skipped {
        // A representable redirect for the same key wins; the skipped rows still count as it.
        if let Some(&i) = at.get(&k) {
            out[i].raw.extend(raw);
            continue;
        }
        out.push(Planned {
            key: k,
            outcome: Outcome::Skipped { reason },
            raw,
        });
    }
    out
}

fn redirect(first: &Input, to: &str) -> Redirect {
    Redirect {
        from: first.from.clone(),
        to: to.to_string(),
        code: first.code,
        origin: first.origin,
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty() && s != "0" && s != "false",
        _ => false,
    }
}

/// A Redirection plugin row (SQL row or REST record).
pub(crate) fn from_redirection(v: &Value, raw: Option<ItemId>) -> Input {
    let s = |k: &str| match &v[k] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    let to = match &v["action_data"] {
        Value::Object(o) => o
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => s("action_data"),
    };
    let code = v["action_code"]
        .as_u64()
        .or_else(|| s("action_code").parse().ok())
        .and_then(|c| u16::try_from(c).ok())
        .unwrap_or(301);
    let enabled = match &v["enabled"] {
        Value::Null => matches!(s("status").as_str(), "" | "enabled"),
        e => truthy(e),
    };
    let from = s("url");
    let action = s("action_type");
    let match_type = s("match_type");
    let skip = if !enabled {
        Some("disabled in Redirection".to_string())
    } else if truthy(&v["regex"]) {
        Some("regular-expression redirect cannot be represented".to_string())
    } else if !matches!(action.as_str(), "" | "url") {
        Some(format!("Redirection action \"{action}\" is not a redirect"))
    } else if !matches!(match_type.as_str(), "" | "url") {
        Some(format!(
            "conditional redirect (match on {match_type}) cannot be represented"
        ))
    } else if from.contains('?') {
        Some("redirect with a query string cannot be represented".to_string())
    } else if to.trim().is_empty() || !(300..400).contains(&code) {
        Some("redirect has no target".to_string())
    } else {
        None
    };
    Input {
        from,
        to,
        code,
        origin: RedirectOrigin::Plugin,
        raw,
        skip,
    }
}

/// Redirects from every source; kept ones become routes. Returns `from` key → redirect.
pub(crate) fn write(
    out: &mut Out,
    raw: &Raw,
    sk: &mut Skeletons,
) -> Result<HashMap<String, ItemId>> {
    let site = &raw.site;
    let mut inputs: Vec<Input> = raw
        .redirection
        .iter()
        .map(|(v, r)| from_redirection(v, *r))
        .collect();
    for (rid, src, r) in out.store.items_decodable::<Redirect>(ItemKind::Redirect)? {
        if raw.crawl && src.connector == "crawl" {
            inputs.push(Input {
                from: r.from,
                to: r.to,
                code: r.code,
                origin: RedirectOrigin::Crawl,
                raw: Some(rid),
                skip: None,
            });
        }
    }
    for m in &out.ctx.config.redirects {
        inputs.push(Input {
            from: m.from.clone(),
            to: m.to.clone(),
            code: m.code,
            origin: RedirectOrigin::Manual,
            raw: None,
            skip: None,
        });
    }
    let base = &site.home;
    let own_hosts: HashSet<String> = site
        .hosts
        .iter()
        .filter_map(|h| url_key(&format!("https://{h}/")))
        .map(|k| k.trim_end_matches('/').to_string())
        .collect();
    let abs = |u: &str| base.join(u).map_or(u.to_string(), |x| x.to_string());
    let mut by_key = HashMap::new();
    for planned in plan(inputs, base) {
        let c = canon(
            ItemKind::Redirect,
            &format!("redirect:{}", planned.key),
            None,
        );
        let rid = c.0;
        match planned.outcome {
            Outcome::Kept { redirect, hops } => {
                out.put(ItemKind::Redirect, &c, &redirect)?;
                if hops > 1 {
                    out.issue(
                        IssueKind::RedirectChain,
                        Severity::Info,
                        rid,
                        format!(
                            "{} went through {hops} redirects; now goes straight to {}",
                            redirect.from, redirect.to
                        ),
                        json!({ "hops": hops, "to": redirect.to }),
                    )?;
                }
                out.status(ItemKind::Redirect, rid, ItemStatus::Done, &planned.raw)?;
                let from = Route {
                    url: abs(&redirect.from),
                    target: RouteTarget::Redirect(rid),
                    http_status: Some(redirect.code),
                    title: None,
                };
                sk.routes.insert(planned.key.clone(), from);
                // A target that is a document keeps its URL, and the route records which.
                if let Some(tk) = key(&redirect.to, base) {
                    let host = &tk[..tk.find('/').unwrap_or(tk.len())];
                    let doc = sk.doc_keys.get(&tk).copied().or_else(|| {
                        own_hosts
                            .contains(host)
                            .then(|| sk.doc_paths.get(key_path(&tk)).copied())
                            .flatten()
                    });
                    if let Some(d) = doc {
                        sk.routes.entry(tk).or_insert(Route {
                            url: abs(&redirect.to),
                            target: RouteTarget::Document(d),
                            http_status: None,
                            title: None,
                        });
                    }
                }
                by_key.insert(planned.key, rid);
            }
            Outcome::Loop { chain } => {
                out.issue(
                    IssueKind::RedirectLoop,
                    Severity::Warning,
                    rid,
                    format!("redirect loop dropped: {}", chain.join(" -> ")),
                    json!({ "chain": chain }),
                )?;
                let reason = "redirect loop".to_string();
                out.status(
                    ItemKind::Redirect,
                    rid,
                    ItemStatus::Skipped { reason },
                    &planned.raw,
                )?;
            }
            Outcome::Skipped { reason } => {
                out.status(
                    ItemKind::Redirect,
                    rid,
                    ItemStatus::Skipped { reason },
                    &planned.raw,
                )?;
            }
        }
    }
    Ok(by_key)
}

/// True when the key's last path segment has an extension (a file, not a page).
fn is_file_key(k: &str) -> bool {
    key_path(k)
        .split('?')
        .next()
        .and_then(|p| p.rsplit('/').next())
        .and_then(|s| s.rsplit_once('.'))
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
}

/// Every old URL: documents, redirects, crawled pages (with their status and title) and
/// attachment files.
pub(crate) fn routes(
    out: &mut Out,
    raw: &Raw,
    sk: &mut Skeletons,
    redirect_by_key: &HashMap<String, ItemId>,
    media_ids: &HashMap<u64, ItemId>,
) -> Result<()> {
    for (cid, src, r) in out.store.items_decodable::<Route>(ItemKind::Route)? {
        if !raw.crawl || src.connector != "crawl" {
            continue;
        }
        let Some(k) = url_key(&r.url) else {
            continue;
        };
        let doc = sk.doc_keys.get(&k).copied().or_else(|| {
            let wp = raw.page_ids.get(&k)?;
            sk.docs.get(wp).copied()
        });
        let target = match (doc, redirect_by_key.get(&k)) {
            (Some(d), _) => RouteTarget::Document(d),
            (None, Some(r)) => RouteTarget::Redirect(*r),
            _ if is_file_key(&k) => out
                .store
                .media_by_url(&r.url)?
                .map_or(RouteTarget::None, RouteTarget::Media),
            _ => RouteTarget::None,
        };
        let e = sk.routes.entry(k.clone()).or_insert(Route {
            url: r.url.clone(),
            target: RouteTarget::None,
            http_status: None,
            title: None,
        });
        if e.target == RouteTarget::None {
            e.target = target;
        }
        e.http_status = r.http_status.or(e.http_status);
        e.title = r.title.clone().or(e.title.take());
        out.prov
            .insert(cid, canon(ItemKind::Route, &format!("route:{k}"), None).0);
    }
    for p in raw.posts.values().filter(|p| p.post_type == "attachment") {
        if let (Some(m), Some(l)) = (media_ids.get(&p.id), p.link.as_deref())
            && let Some(k) = url_key(l)
        {
            sk.routes.entry(k).or_insert(Route {
                url: l.to_string(),
                target: RouteTarget::Media(*m),
                http_status: None,
                title: None,
            });
        }
    }
    for (k, r) in &sk.routes {
        let c = canon(ItemKind::Route, &format!("route:{k}"), Some(r.url.clone()));
        out.put(ItemKind::Route, &c, r)?;
        out.status(ItemKind::Route, c.0, ItemStatus::Done, [])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(from: &str, to: &str, origin: RedirectOrigin) -> Input {
        Input {
            from: from.into(),
            to: to.into(),
            code: 301,
            origin,
            raw: None,
            skip: None,
        }
    }

    fn base() -> Url {
        Url::parse("https://example.test/").unwrap()
    }

    #[test]
    fn manual_beats_plugin_beats_crawl() {
        let p = plan(
            vec![
                inp("https://example.test/x", "/crawl/", RedirectOrigin::Crawl),
                inp("/x/", "/plugin/", RedirectOrigin::Plugin),
                inp("/X", "/manual/", RedirectOrigin::Manual),
            ],
            &base(),
        );
        assert_eq!(p.len(), 1);
        match &p[0].outcome {
            Outcome::Kept { redirect, hops: 1 } => {
                assert_eq!(redirect.to, "/manual/");
                assert_eq!(redirect.origin, RedirectOrigin::Manual);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn chain_into_loop_is_a_loop() {
        let p = plan(
            vec![
                inp("/a/", "/b/", RedirectOrigin::Plugin),
                inp("/b/", "/c/", RedirectOrigin::Plugin),
                inp("/c/", "/b/", RedirectOrigin::Plugin),
                inp("/d/", "/d/", RedirectOrigin::Plugin),
            ],
            &base(),
        );
        assert!(
            p.iter().all(|x| matches!(x.outcome, Outcome::Loop { .. })),
            "{p:?}"
        );
    }

    #[test]
    fn redirection_rows_screened() {
        let row = |extra: Value| {
            let mut v = serde_json::json!({"url": "/a/", "action_type": "url", "action_data": "/b/", "action_code": 301, "match_type": "url", "regex": 0, "status": "enabled"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            from_redirection(&v, None).skip
        };
        assert_eq!(row(serde_json::json!({})), None);
        assert!(row(serde_json::json!({"regex": 1})).is_some());
        assert!(row(serde_json::json!({"url": "/a/?x=1"})).is_some());
        assert!(row(serde_json::json!({"status": "disabled"})).is_some());
        assert!(row(serde_json::json!({"enabled": false})).is_some());
        assert!(row(serde_json::json!({"action_type": "error"})).is_some());
        assert!(row(serde_json::json!({"match_type": "login"})).is_some());
        assert_eq!(
            row(serde_json::json!({"action_data": {"url": "/b/"}, "enabled": true})),
            None
        );
    }
}
