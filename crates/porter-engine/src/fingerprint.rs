//! What a stage run depended on in porter.toml, recorded on its run row so a later front
//! end can tell whether the config it ran with is still the config in use.
//!
//! A fingerprint is `section=hash;section=hash;…` over exactly the sections the stage
//! reads, each hash a sha256 of the section's canonical JSON: object keys sorted at every
//! level (so reordering `[mapping.types]` is not a change), arrays kept in order
//! (transforms, `[[redirects]]` and connectors are ordered on purpose). Comments,
//! formatting, `[hub]` and `[project]` never count. Per-section hashes let the reason name
//! what changed.

use crate::config::ProjectConfig;
use porter_model::Stage;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Objects rebuilt with sorted keys, at every level; arrays untouched.
fn canonical(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let sorted: std::collections::BTreeMap<String, Value> =
                m.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(canonical).collect()),
        v => v,
    }
}

/// The sections `stage` reads, in a fixed order. This match is the stage→section table;
/// keep it in step with where each stage reads porter.toml:
/// - analyse, extract: `[source]`, connectors
/// - normalise: those, plus `[[redirects]]` and `mapping.redirects.from_fields`
/// - schema: `[mapping]`, `[destination]`
/// - load: `[source]`, `[mapping]`, `[destination]`
/// - verify: load's, plus `[verify]`
fn sections(cfg: &ProjectConfig, stage: Stage) -> Vec<(&'static str, String)> {
    fn h(v: &impl Serialize) -> String {
        // ponytail: an unserialisable section hashes as null; every section is plain data.
        let v = canonical(serde_json::to_value(v).unwrap_or_default());
        Sha256::digest(v.to_string().as_bytes())
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    let source = ("source", h(&cfg.source));
    let mapping = ("mapping", h(&cfg.mapping));
    let destination = ("destination", h(&cfg.destination));
    match stage {
        Stage::Analyse | Stage::Extract => vec![source, ("connectors", h(&cfg.connectors))],
        Stage::Normalise => vec![
            source,
            ("connectors", h(&cfg.connectors)),
            ("redirects", h(&cfg.redirects)),
            (
                "mapping.redirects.from_fields",
                h(&cfg.mapping.redirects.from_fields),
            ),
        ],
        Stage::Schema => vec![mapping, destination],
        Stage::Load => vec![source, mapping, destination],
        Stage::Verify => vec![source, mapping, destination, ("verify", h(&cfg.verify))],
    }
}

/// The fingerprint of what `stage` reads from `cfg`.
pub fn fingerprint(cfg: &ProjectConfig, stage: Stage) -> String {
    sections(cfg, stage)
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// The reason for a run from before porter recorded fingerprints.
pub const NOT_RECORDED: &str = "ran before settings were recorded; re-run to refresh";

/// Why a run recorded with `recorded` no longer matches `cfg`: "<section> changed since",
/// naming the first section that differs. A run with no record (an older porter) fails
/// closed.
pub fn changed(recorded: Option<&str>, cfg: &ProjectConfig, stage: Stage) -> Option<String> {
    let Some(recorded) = recorded else {
        return Some(NOT_RECORDED.into());
    };
    let old: Vec<&str> = recorded.split(';').collect();
    for (k, v) in sections(cfg, stage) {
        if !old.contains(&format!("{k}={v}").as_str()) {
            return Some(format!("{k} changed since"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(toml: &str) -> ProjectConfig {
        toml::from_str(toml).unwrap()
    }

    const BASE: &str = r#"
[project]
name = "demo"

[source]
adapter = "wordpress"

[destination]
adapter = "payload"
url = "https://cms.example.com"

[mapping.types.post]
to = "posts"
"#;

    #[test]
    fn comments_hub_and_name_never_count() {
        let a = cfg(BASE);
        let fp = fingerprint(&a, Stage::Load);
        let edited = format!(
            "# a comment\n{}\n[hub]\nurl = \"https://hub.example.com\"\nproject = \"demo\"\n",
            BASE.replace("name = \"demo\"", "name = \"renamed\"")
        );
        assert_eq!(fingerprint(&cfg(&edited), Stage::Load), fp);
        assert_eq!(changed(Some(&fp), &cfg(&edited), Stage::Load), None);
    }

    #[test]
    fn a_stage_sees_only_its_sections() {
        let a = cfg(BASE);
        let b = cfg(&BASE.replace("to = \"posts\"", "to = \"articles\""));
        // Extract does not read the mapping; load does, and says so.
        let ex = fingerprint(&a, Stage::Extract);
        assert_eq!(changed(Some(&ex), &b, Stage::Extract), None);
        let ld = fingerprint(&a, Stage::Load);
        assert_eq!(
            changed(Some(&ld), &b, Stage::Load).as_deref(),
            Some("mapping changed since")
        );
        let c = cfg(&BASE.replace("cms.example.com", "other.example.com"));
        assert_eq!(
            changed(Some(&ld), &c, Stage::Load).as_deref(),
            Some("destination changed since")
        );
        let nm = fingerprint(&a, Stage::Normalise);
        assert_eq!(changed(Some(&nm), &c, Stage::Normalise), None);
        // Normalise reads only the redirect fields of the mapping: a type edit is not its
        // business, a from_fields edit is.
        assert_eq!(changed(Some(&nm), &b, Stage::Normalise), None);
        let r = cfg(&format!(
            "{BASE}\n[mapping.redirects]\ncollection = \"redirects\"\nfrom_fields = [\"acf.old\"]\n"
        ));
        assert_eq!(
            changed(Some(&nm), &r, Stage::Normalise).as_deref(),
            Some("mapping.redirects.from_fields changed since")
        );
        // Nothing recorded: unknown, so stale.
        assert!(changed(None, &a, Stage::Analyse).is_some());
    }

    #[test]
    fn reordered_keys_are_not_a_change_but_reordered_lists_are() {
        let two = |a: &str, b: &str| cfg(&format!("[source]\nadapter = \"wordpress\"\n{a}\n{b}\n"));
        let post = "[mapping.types.post]\nto = \"posts\"\nkey_field = \"legacyId\"";
        let page = "[mapping.types.page]\nto = \"pages\"";
        let fp = fingerprint(&two(post, page), Stage::Load);
        assert_eq!(changed(Some(&fp), &two(page, post), Stage::Load), None);
        // Keys within a table, reordered.
        let post2 = "[mapping.types.post]\nkey_field = \"legacyId\"\nto = \"posts\"";
        assert_eq!(changed(Some(&fp), &two(post2, page), Stage::Load), None);
        // Transforms run in order: swapping them is a change.
        let t = |list: &str| {
            cfg(&format!(
                "[mapping.types.post]\nto = \"posts\"\n[mapping.types.post.fields.title]\nto = \"title\"\ntransform = [{list}]\n"
            ))
        };
        let fp = fingerprint(&t("{ op = \"trim\" }, { op = \"lower\" }"), Stage::Load);
        assert_eq!(
            changed(
                Some(&fp),
                &t("{ op = \"lower\" }, { op = \"trim\" }"),
                Stage::Load
            )
            .as_deref(),
            Some("mapping changed since")
        );
    }
}
