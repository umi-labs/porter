//! Suggested tidy-ups: groups of types (or block types) that look like one thing split
//! in several, found by how much their field names overlap. Every suggestion is written
//! disabled, with a plain label and the reason; enabling one is `porter tidy enable <id>`.

use super::{Discriminator, TidyKind, TidyRule};
use crate::mapping::MappingConfig;
use crate::mapping::edit::MappingDoc;
use crate::mapping::summary::TypeSummary;
use crate::project::Project;
use anyhow::Result;
use indexmap::IndexMap;
use std::collections::BTreeSet;

/// Jaccard similarity of field-name sets at or above which two types are grouped.
const SIMILAR: f64 = 0.7;
/// The lower bar for two types where one has a reference field to the other.
const SIMILAR_LINKED: f64 = 0.5;

/// Suggestions for what `summary` holds, none enabled, skipping groups an existing rule
/// already covers (same `from` set, whatever its id).
pub fn suggest(summary: &TypeSummary, existing: &[TidyRule]) -> Vec<TidyRule> {
    suggest_in(summary, existing, &MappingConfig::default())
}

/// As `suggest`, also avoiding discriminator names the mapping already sends fields to.
pub fn suggest_in(
    summary: &TypeSummary,
    existing: &[TidyRule],
    mapping: &MappingConfig,
) -> Vec<TidyRule> {
    let mut rules: Vec<TidyRule> = vec![];
    let mut covered: Vec<BTreeSet<String>> = existing.iter().map(from_set).collect();
    let mut ids: BTreeSet<String> = existing.iter().map(|r| r.id.clone()).collect();

    let types: Vec<(String, BTreeSet<String>)> = sorted(
        summary
            .types
            .iter()
            .map(|(t, s)| (t.clone(), s.fields.keys().cloned().collect())),
    );
    let blocks: Vec<(String, BTreeSet<String>)> = sorted(
        summary
            .blocks
            .iter()
            .filter_map(|b| Some((b.module.clone()?, b.props.keys().cloned().collect()))),
    );

    for (is_types, items) in [(true, &types), (false, &blocks)] {
        let linked = |a: &str, b: &str| is_types && references(summary, a, b);
        for (group, via_link) in components(items, &linked) {
            let names: Vec<&String> = group.iter().map(|&i| &items[i].0).collect();
            if covered.contains(&names.iter().map(|n| n.to_string()).collect()) {
                continue;
            }
            let sets: Vec<&BTreeSet<String>> = group.iter().map(|&i| &items[i].1).collect();
            let common = sets
                .iter()
                .skip(1)
                .fold(sets[0].clone(), |a, s| a.intersection(s).cloned().collect());
            let all: BTreeSet<&String> = sets.iter().flat_map(|s| s.iter()).collect();
            let from: Vec<String> = names.iter().map(|n| n.to_string()).collect();
            let into = into_name(&from);
            let id = unique(&mut ids, &into);
            let kind = if is_types {
                TidyKind::MergeTypes {
                    discriminator: Some(discriminator(&from, summary, mapping)),
                    parent: parents(&from, summary),
                    from: from.clone(),
                    into: into.clone(),
                    drop: IndexMap::new(),
                }
            } else {
                TidyKind::MergeBlocks {
                    from: from.clone(),
                    into: into.clone(),
                    fields: IndexMap::new(),
                    drop: IndexMap::new(),
                }
            };
            covered.push(from.iter().cloned().collect());
            rules.push(TidyRule {
                id,
                enabled: false,
                label: Some(format!(
                    "Merge {} into {}",
                    list(
                        &from
                            .iter()
                            .map(|n| humanise(&plural_name(n)))
                            .collect::<Vec<_>>()
                    ),
                    humanise(&into)
                )),
                why: Some(format!(
                    "They share {} of {} fields{}",
                    common.len(),
                    all.len(),
                    if via_link {
                        " and link to each other"
                    } else {
                        ""
                    }
                )),
                kind,
            });
        }
    }
    rules
}

/// Writes the suggestions for what is normalised into porter.toml (disabled), returning
/// the rules added. Errors when nothing is normalised yet.
pub fn append(project: &Project) -> Result<Vec<TidyRule>> {
    let summary = TypeSummary::from_store(project.store())?;
    if summary.types.is_empty() {
        anyhow::bail!("nothing is normalised yet; run porter normalise first");
    }
    let path = project.dir().join("porter.toml");
    let mut doc = MappingDoc::open(&path)?;
    let cfg = doc.config()?;
    let skip = excluded(project, &cfg.mapping, &summary);
    let mut filtered = summary;
    filtered.types.retain(|t, _| !skip.contains(t));
    let new = suggest_in(&filtered, &cfg.tidy, &cfg.mapping);
    for r in &new {
        doc.add_tidy(r)?;
    }
    let problems = doc.save();
    if !problems.is_empty() {
        anyhow::bail!("{}", problems.join("; "));
    }
    Ok(new)
}

/// Types never worth merging: form submissions, dropped types and singletons (from
/// the Sanity analysis when there is one, else any type with a single document).
fn excluded(project: &Project, m: &MappingConfig, summary: &TypeSummary) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = m.forms.submission_types.iter().cloned().collect();
    out.extend(
        m.types
            .iter()
            .filter(|(_, t)| t.to.is_none())
            .map(|(t, _)| t.clone()),
    );
    let analysis = std::fs::read_to_string(project.raw_dir("sanity").join("analysis.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    match analysis.as_ref().and_then(|a| a["singletons"].as_array()) {
        Some(s) => out.extend(s.iter().filter_map(|v| v.as_str().map(String::from))),
        None => out.extend(
            summary
                .types
                .iter()
                .filter(|(_, s)| s.count == 1)
                .map(|(t, _)| t.clone()),
        ),
    }
    out
}

fn from_set(r: &TidyRule) -> BTreeSet<String> {
    match &r.kind {
        TidyKind::MergeTypes { from, .. } | TidyKind::MergeBlocks { from, .. } => {
            from.iter().cloned().collect()
        }
    }
}

fn sorted(it: impl Iterator<Item = (String, BTreeSet<String>)>) -> Vec<(String, BTreeSet<String>)> {
    let mut v: Vec<_> = it.collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.dedup_by(|a, b| a.0 == b.0);
    v
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / union as f64
}

/// Connected components (of two or more) over qualifying pairs, in first-member order,
/// each with whether a pair in it qualified only through the linked bar.
fn components(
    items: &[(String, BTreeSet<String>)],
    linked: &dyn Fn(&str, &str) -> bool,
) -> Vec<(Vec<usize>, bool)> {
    let n = items.len();
    let j = |a: usize, b: usize| jaccard(&items[a].1, &items[b].1);
    let link =
        |a: usize, b: usize| linked(&items[a].0, &items[b].0) || linked(&items[b].0, &items[a].0);
    let only_linked =
        |a: usize, b: usize| j(a, b) < SIMILAR && j(a, b) >= SIMILAR_LINKED && link(a, b);
    let mut comp: Vec<usize> = (0..n).collect();
    for a in 0..n {
        for b in a + 1..n {
            if j(a, b) >= SIMILAR || only_linked(a, b) {
                let (x, y) = (comp[a], comp[b]);
                comp.iter_mut().filter(|c| **c == y).for_each(|c| *c = x);
            }
        }
    }
    let mut out = vec![];
    for i in 0..n {
        if comp[i] == i {
            let g: Vec<usize> = (0..n).filter(|&k| comp[k] == i).collect();
            if g.len() > 1 {
                let l = g
                    .iter()
                    .any(|&a| g.iter().any(|&b| a < b && only_linked(a, b)));
                out.push((g, l));
            }
        }
    }
    out
}

/// `regionPage` -> ["region", "page"].
fn words(s: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    for c in s.chars() {
        if (c.is_uppercase() || !c.is_alphanumeric()) && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The words every name ends with.
fn common_suffix(names: &[String]) -> Vec<String> {
    let ws: Vec<Vec<String>> = names.iter().map(|n| words(n)).collect();
    let mut n = 0;
    while ws
        .iter()
        .all(|w| w.len() > n && w[w.len() - 1 - n] == ws[0][ws[0].len() - 1 - n])
    {
        n += 1;
    }
    ws[0][ws[0].len() - n..].to_vec()
}

fn plural(w: &str) -> String {
    let vowel = |c: char| "aeiou".contains(c);
    if w.ends_with('s') || w.ends_with('x') || w.ends_with("ch") || w.ends_with("sh") {
        format!("{w}es")
    } else if w.ends_with('y') && !w.chars().rev().nth(1).is_some_and(vowel) {
        format!("{}ies", &w[..w.len() - 1])
    } else {
        format!("{w}s")
    }
}

/// The plural of the members' common camelCase suffix, else the first member's name
/// with `-merged`.
fn into_name(from: &[String]) -> String {
    let suffix = common_suffix(from);
    if suffix.is_empty() {
        return format!("{}-merged", words(&from[0]).join("-"));
    }
    let (last, rest) = suffix.split_last().map_or(("", &[][..]), |(l, r)| (l, r));
    let mut w: Vec<String> = rest.to_vec();
    w.push(plural(last));
    w.join("-")
}

/// A free id from `base`: `base`, then `base-2`, `base-3`...
fn unique(ids: &mut BTreeSet<String>, base: &str) -> String {
    let mut id = base.to_string();
    let mut n = 2;
    while ids.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    ids.insert(id.clone());
    id
}

/// "cityPage" -> "city-pages" (the last word pluralised), for labels.
fn plural_name(s: &str) -> String {
    let mut w = words(s);
    if let Some(l) = w.pop() {
        w.push(plural(&l));
    }
    w.join("-")
}

/// "regionPage" / "region-pages" -> "Region page" / "Region pages".
fn humanise(s: &str) -> String {
    let w = words(s).join(" ");
    let mut c = w.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// "A, B and C".
fn list(names: &[String]) -> String {
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => last.clone(),
        None => String::new(),
    }
}

/// Field `kind` (or the first of `page_kind`, `kind_2` no member already maps), one
/// value per type: its name without the shared suffix.
fn discriminator(from: &[String], summary: &TypeSummary, mapping: &MappingConfig) -> Discriminator {
    let taken = |name: &str| {
        from.iter().any(|t| {
            summary
                .types
                .get(t)
                .is_some_and(|s| s.fields.contains_key(name))
                || mapping
                    .types
                    .get(t)
                    .is_some_and(|m| m.fields.values().any(|f| f.to == name))
        })
    };
    let field = ["kind", "page_kind", "kind_2"]
        .into_iter()
        .find(|n| !taken(n))
        .unwrap_or("kind_3");
    let suffix = common_suffix(from).len();
    let values = from
        .iter()
        .map(|t| {
            let w = words(t);
            let kept = &w[..w.len() - suffix];
            (
                t.clone(),
                if kept.is_empty() {
                    w.join(" ")
                } else {
                    kept.join(" ")
                },
            )
        })
        .collect();
    Discriminator {
        field: field.into(),
        values,
    }
}

/// Whether type `a` has a reference field pointing at type `b`.
fn references(summary: &TypeSummary, a: &str, b: &str) -> bool {
    summary.types.get(a).is_some_and(|s| {
        s.fields
            .values()
            .any(|f| f.variants.contains("Doc") && f.relation_types.contains(b))
    })
}

/// A member whose reference field points at another member gets that field as parent.
fn parents(from: &[String], summary: &TypeSummary) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for t in from {
        let Some(s) = summary.types.get(t) else {
            continue;
        };
        let field = s.fields.iter().find(|(_, f)| {
            f.variants.contains("Doc")
                && f.relation_types.iter().any(|r| r != t && from.contains(r))
        });
        if let Some((name, _)) = field {
            out.insert(t.clone(), name.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::summary::{BlockStats, FieldStats, TypeStats};

    fn stats(fields: &[&str]) -> TypeStats {
        TypeStats {
            count: 3,
            fields: fields
                .iter()
                .map(|f| (f.to_string(), FieldStats::default()))
                .collect(),
            ..Default::default()
        }
    }

    fn summary() -> TypeSummary {
        let mut s = TypeSummary::default();
        let base = ["title", "slug", "seo", "body", "image"];
        s.types.insert("regionPage".into(), stats(&base));
        s.types.insert("countryPage".into(), stats(&base));
        s.types.insert("cityPage".into(), stats(&base));
        s.types.insert("post".into(), stats(&["a", "b"]));
        s
    }

    #[test]
    fn groups_similar_types_and_names_them() {
        let r = suggest(&summary(), &[]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "pages");
        assert!(!r[0].enabled);
        assert_eq!(
            r[0].label.as_deref(),
            Some("Merge City pages, Country pages and Region pages into Pages")
        );
        assert_eq!(r[0].why.as_deref(), Some("They share 5 of 5 fields"));
    }

    #[test]
    fn rerun_is_a_no_op_and_ids_do_not_clash() {
        let first = suggest(&summary(), &[]);
        assert!(suggest(&summary(), &first).is_empty());
        let mut other = first.clone();
        other[0].kind = TidyKind::MergeBlocks {
            from: vec!["x".into(), "y".into()],
            into: "z".into(),
            fields: IndexMap::new(),
            drop: IndexMap::new(),
        };
        assert_eq!(suggest(&summary(), &other)[0].id, "pages-2");
    }

    #[test]
    fn names_without_a_shared_noun_get_merged() {
        let mut s = TypeSummary::default();
        s.types.insert("alpha".into(), stats(&["a", "b"]));
        s.types.insert("beta".into(), stats(&["a", "b"]));
        assert_eq!(suggest(&s, &[])[0].id, "alpha-merged");
    }

    #[test]
    fn carousels_pluralise_the_shared_words() {
        let mut s = TypeSummary::default();
        for m in ["cardCarousel", "customCardCarousel"] {
            s.blocks.push(BlockStats {
                kind: "x".into(),
                module: Some(m.into()),
                count: 1,
                props: [("cards".to_string(), FieldStats::default())].into(),
            });
        }
        let r = suggest(&s, &[]);
        assert_eq!(r[0].id, "card-carousels");
    }

    #[test]
    fn discriminator_avoids_a_mapped_kind() {
        let mut s = summary();
        s.types
            .get_mut("cityPage")
            .map(|t| t.fields.insert("kind".into(), FieldStats::default()));
        let r = suggest(&s, &[]);
        let TidyKind::MergeTypes {
            discriminator: Some(d),
            ..
        } = &r[0].kind
        else {
            panic!("not a type merge")
        };
        assert_eq!(d.field, "page_kind");
        assert_eq!(d.values["regionPage"], "region");
    }

    /// A type with `own` made-up fields plus the ones named in `shared`, and references.
    fn ty(shared: &[&str], own: usize, tag: &str, refs: &[&str]) -> TypeStats {
        let mut t = stats(shared);
        for i in 0..own {
            t.fields.insert(format!("{tag}{i}"), FieldStats::default());
        }
        for r in refs {
            let mut f = FieldStats::default();
            f.variants.insert("Doc".into());
            f.relation_types.insert(r.to_string());
            t.fields.insert(format!("ref_{r}"), f);
        }
        t
    }

    /// Field sets built to the ratios measured on a real dataset.
    #[test]
    fn real_world_pairings_cluster_as_measured() {
        let mut s = TypeSummary::default();
        // Each pair shares a core of 7 fields, so J = 7 / (7 + extras): 0.7 needs 3
        // extras between them, 0.5 needs 7.
        let core = ["a", "b", "c", "d", "e", "f", "g"];
        // destinations: city links country (0.67-ish), region/country 0.53, city/region 0.62
        s.types.insert("regionPage".into(), ty(&core, 0, "r", &[]));
        s.types
            .insert("countryPage".into(), ty(&core, 4, "c", &["regionPage"]));
        s.types
            .insert("cityPage".into(), ty(&core, 0, "y", &["countryPage"]));
        // unrelated: 7/(7+10) = 0.41, no reference
        s.types
            .insert("hotelBreak".into(), ty(&core[..4], 3, "h", &[]));
        // holidays: chain of >= 0.7 pairs
        let h = ["h1", "h2", "h3", "h4", "h5", "h6", "h7", "h8", "h9"];
        s.types.insert("railJourney".into(), ty(&h, 0, "x", &[]));
        s.types.insert("riverCruise".into(), ty(&h, 1, "y", &[]));
        s.types
            .insert("tailorMadeHoliday".into(), ty(&h[..8], 2, "z", &[]));
        // 0.67 with riverCruise-like fields but no reference: not merged
        s.types.insert("opera".into(), ty(&h[..6], 0, "o", &[]));
        let r = suggest(&s, &[]);
        let froms: Vec<Vec<String>> = r
            .iter()
            .map(|r| match &r.kind {
                TidyKind::MergeTypes { from, .. } => from.clone(),
                _ => vec![],
            })
            .collect();
        assert!(
            froms.contains(&vec![
                "cityPage".to_string(),
                "countryPage".into(),
                "regionPage".into()
            ]),
            "{froms:?}"
        );
        assert!(froms.iter().all(|f| !f.contains(&"hotelBreak".to_string())));
        assert!(
            froms.iter().all(|f| !f.contains(&"opera".to_string())),
            "{froms:?}"
        );
        let holidays = froms
            .iter()
            .find(|f| f.contains(&"railJourney".to_string()))
            .unwrap();
        assert_eq!(holidays.len(), 3, "{froms:?}");
        let dest = r.iter().find(|r| r.id == "pages").unwrap();
        assert!(
            dest.why
                .as_deref()
                .unwrap()
                .ends_with("and link to each other")
        );
    }
}
