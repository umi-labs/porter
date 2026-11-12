//! What a stage run depended on in porter.toml, recorded on its run row so a later front
//! end can tell whether the config it ran with is still the config in use.
//!
//! A fingerprint is `section=hash;section=hash;…` over only the sections the stage reads,
//! each hash a sha256 of the section's canonical (serde JSON) form. Comments, formatting,
//! `[hub]` and `[project]` never count. Per-section hashes let the reason name what changed.

use crate::config::ProjectConfig;
use porter_model::Stage;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The sections `stage` reads, in a fixed order.
fn sections(cfg: &ProjectConfig, stage: Stage) -> Vec<(&'static str, String)> {
    fn h(v: &impl Serialize) -> String {
        // ponytail: an unserialisable section hashes as empty; every section is plain data.
        let json = serde_json::to_vec(v).unwrap_or_default();
        Sha256::digest(&json)
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    let mut out = vec![
        ("source", h(&cfg.source)),
        ("connectors", h(&cfg.connectors)),
    ];
    if matches!(
        stage,
        Stage::Normalise | Stage::Schema | Stage::Load | Stage::Verify
    ) {
        out.push(("mapping", h(&cfg.mapping)));
        out.push(("redirects", h(&cfg.redirects)));
    }
    if matches!(stage, Stage::Schema | Stage::Load | Stage::Verify) {
        out.push(("destination", h(&cfg.destination)));
    }
    if stage == Stage::Verify {
        out.push(("verify", h(&cfg.verify)));
    }
    out
}

/// The fingerprint of what `stage` reads from `cfg`.
pub fn fingerprint(cfg: &ProjectConfig, stage: Stage) -> String {
    sections(cfg, stage)
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// Why a run recorded with `recorded` no longer matches `cfg`: "<section> changed since",
/// naming the first section that differs. A run with no record (an older porter) fails
/// closed.
pub fn changed(recorded: Option<&str>, cfg: &ProjectConfig, stage: Stage) -> Option<String> {
    let Some(recorded) = recorded else {
        return Some("its config was not recorded".into());
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
        // Nothing recorded: unknown, so stale.
        assert!(changed(None, &a, Stage::Analyse).is_some());
    }
}
