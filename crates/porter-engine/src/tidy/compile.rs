//! Enabled rules compiled into the `[mapping]` they act on.

use super::{TidyKind, split_key};
use crate::config::ProjectConfig;
use crate::mapping::{BlockMap, BlockMatch, FieldMap, MappingConfig, Transform};
use anyhow::{Result, bail};

/// The source path the discriminator is mapped from. No document has it, so the
/// `default` transform always supplies the type's value.
pub const DISCRIMINATOR: &str = "_tidy.discriminator";

/// The mapping schema and load use: `[mapping]` with every enabled rule compiled in.
/// A merged type keeps its hand edits (`fields`, `path`, `status`); merged blocks get
/// one map per block type, ahead of the hand-written ones, so they match first.
pub fn effective_mapping(cfg: &ProjectConfig) -> Result<MappingConfig> {
    let mut m = cfg.mapping.clone();
    let mut compiled: Vec<BlockMap> = vec![];
    for rule in cfg.tidy.iter().filter(|r| r.enabled) {
        let id = &rule.id;
        // Every `"<type>.<field>"` / type key must name a from-type.
        let check = |from: &[String], keys: &mut dyn Iterator<Item = &String>, what: &str| {
            for k in keys {
                let ty = split_key(k).map_or(k.as_str(), |(t, _)| t);
                if !from.iter().any(|f| f == ty) {
                    bail!("tidy \"{id}\": {what} `{k}` names no type in `from`");
                }
            }
            Ok(())
        };
        match &rule.kind {
            TidyKind::MergeTypes {
                from,
                into,
                discriminator,
                parent,
                drop,
            } => {
                check(from, &mut parent.keys(), "parent")?;
                check(from, &mut drop.keys(), "drop")?;
                // The discriminator would overwrite whatever a from-type already sends to
                // its field (or a sub-field of it): refuse rather than lose that data.
                if let Some(d) = discriminator {
                    let used: Vec<String> = from
                        .iter()
                        .filter_map(|ty| Some((ty, m.types.get(ty)?)))
                        .flat_map(|(ty, tm)| {
                            tm.fields
                                .iter()
                                .filter(|(src, fm)| {
                                    !fm.drop
                                        && src.as_str() != DISCRIMINATOR
                                        && (fm.to == d.field
                                            || fm.to.starts_with(&format!("{}.", d.field)))
                                })
                                .map(move |(src, _)| format!("{ty}.{src}"))
                        })
                        .collect();
                    if !used.is_empty() {
                        bail!(
                            "tidy \"{id}\": discriminator field {} is already used by {}; pick another name",
                            d.field,
                            used.join(", ")
                        );
                    }
                }
                for ty in from {
                    let Some(tm) = m.types.get_mut(ty) else {
                        bail!(
                            "tidy \"{id}\": `{ty}` has no [mapping.types.{ty}] entry, so its fields would be left behind; run `porter analyse` to seed it"
                        );
                    };
                    tm.to = Some(into.clone());
                    tm.drop_reason = None;
                    if let Some(d) = discriminator {
                        let Some(value) = d.values.get(ty) else {
                            bail!("tidy \"{id}\": discriminator has no value for `{ty}`");
                        };
                        tm.fields.insert(
                            DISCRIMINATOR.into(),
                            FieldMap {
                                to: d.field.clone(),
                                transform: vec![Transform::Default {
                                    value: value.clone(),
                                }],
                                ..FieldMap::default()
                            },
                        );
                    }
                    if let Some(p) = parent.get(ty) {
                        tm.parent = Some(p.clone());
                    }
                }
                for (key, reason) in drop {
                    let Some((ty, field)) = split_key(key) else {
                        bail!("tidy \"{id}\": drop `{key}` is not `<type>.<field>`");
                    };
                    if let Some(tm) = m.types.get_mut(ty) {
                        tm.fields.insert(
                            field.into(),
                            FieldMap {
                                drop: true,
                                reason: Some(reason.clone()),
                                ..FieldMap::default()
                            },
                        );
                    }
                }
            }
            TidyKind::MergeBlocks {
                from,
                into,
                fields,
                drop,
            } => {
                check(from, &mut fields.keys(), "field")?;
                check(from, &mut drop.keys(), "drop")?;
                for ty in from {
                    // Start from the block type's own map (every prop it carries), so
                    // nothing it carried is lost; then the renames, then the drops.
                    let mut map = BlockMap {
                        r#match: BlockMatch {
                            kind: None,
                            module: Some(ty.clone()),
                        },
                        to: into.clone(),
                        fields: Default::default(),
                    };
                    if let Some(own) = cfg
                        .mapping
                        .blocks
                        .iter()
                        .find(|b| b.r#match.module.as_deref() == Some(ty.as_str()))
                    {
                        map.fields = own.fields.clone();
                    }
                    for (key, dest) in fields {
                        if let Some((t, f)) = split_key(key)
                            && t == ty
                        {
                            map.fields.insert(f.into(), dest.clone());
                        }
                    }
                    for key in drop.keys() {
                        if let Some((t, f)) = split_key(key)
                            && t == ty
                        {
                            map.fields.shift_remove(f);
                        }
                    }
                    compiled.push(map);
                }
            }
        }
    }
    compiled.append(&mut m.blocks);
    m.blocks = compiled;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{BlockMap, BlockMatch, FieldMap, Transform, TypeMap};

    const RULES: &str = r#"
[[tidy]]
id = "destinations"
kind = "merge-types"
enabled = true
from = ["regionPage", "countryPage", "cityPage"]
into = "destinations"
discriminator = { field = "kind", values = { regionPage = "region", countryPage = "country", cityPage = "city" } }
parent = { cityPage = "country", countryPage = "region" }
drop = { "cityPage.addAnchor" = "unused layout flag" }

[[tidy]]
id = "card-carousels"
kind = "merge-blocks"
enabled = true
from = ["cardCarousel", "customCardCarousel"]
into = "card-carousel"
fields = { "customCardCarousel.cards" = "items" }
"#;

    fn ident(fields: &[&str]) -> indexmap::IndexMap<String, FieldMap> {
        fields
            .iter()
            .map(|f| {
                (
                    f.to_string(),
                    FieldMap {
                        to: f.to_string(),
                        ..FieldMap::default()
                    },
                )
            })
            .collect()
    }

    fn block(module: &str, to: &str, fields: &[(&str, &str)]) -> BlockMap {
        BlockMap {
            r#match: BlockMatch {
                kind: None,
                module: Some(module.into()),
            },
            to: to.into(),
            fields: fields
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }

    fn cfg() -> ProjectConfig {
        let mut c: ProjectConfig = toml::from_str(RULES).unwrap();
        for (ty, fs) in [
            ("regionPage", &["seo"][..]),
            ("countryPage", &["seo", "region"][..]),
            ("cityPage", &["seo", "country", "addAnchor"][..]),
        ] {
            c.mapping.types.insert(
                ty.into(),
                TypeMap {
                    to: Some(ty.into()),
                    fields: ident(fs),
                    ..TypeMap::default()
                },
            );
        }
        c.mapping.blocks = vec![
            block("hero", "hero", &[("heading", "heading")]),
            block("cardCarousel", "card-carousel", &[("cards", "cards")]),
            block(
                "customCardCarousel",
                "custom-card-carousel",
                &[("cards", "cards"), ("title", "title")],
            ),
        ];
        c
    }

    #[test]
    fn rules_parse_and_typos_error() {
        let c = cfg();
        assert_eq!(c.tidy.len(), 2);
        assert!(c.tidy[0].enabled);
        let back: ProjectConfig = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        let bad =
            "[[tidy]]\nid = \"x\"\nkind = \"merge-types\"\nfrom = []\ninto = \"y\"\nparnet = {}\n";
        let e = toml::from_str::<ProjectConfig>(bad).unwrap_err();
        assert!(e.to_string().contains("parnet"), "{e}");
    }

    #[test]
    fn disabled_rule_changes_nothing() {
        let mut c = cfg();
        c.tidy.iter_mut().for_each(|r| r.enabled = false);
        assert_eq!(effective_mapping(&c).unwrap(), c.mapping);
    }

    #[test]
    fn merge_types_compiles_into_each_from_type() {
        let m = effective_mapping(&cfg()).unwrap();
        for (ty, kind) in [
            ("regionPage", "region"),
            ("countryPage", "country"),
            ("cityPage", "city"),
        ] {
            let tm = &m.types[ty];
            assert_eq!(tm.to.as_deref(), Some("destinations"), "{ty}");
            let d = tm.fields.values().find(|f| f.to == "kind").unwrap();
            assert_eq!(d.transform, vec![Transform::Default { value: kind.into() }]);
        }
        assert_eq!(m.types["cityPage"].parent.as_deref(), Some("country"));
        assert_eq!(m.types["countryPage"].parent.as_deref(), Some("region"));
        assert_eq!(m.types["regionPage"].parent, None);
        let a = &m.types["cityPage"].fields["addAnchor"];
        assert!(a.drop && a.reason.as_deref() == Some("unused layout flag"));
    }

    #[test]
    fn merge_blocks_compiles_block_maps() {
        let m = effective_mapping(&cfg()).unwrap();
        assert_eq!(
            m.blocks[..2],
            [
                block("cardCarousel", "card-carousel", &[("cards", "cards")]),
                block(
                    "customCardCarousel",
                    "card-carousel",
                    &[("cards", "items"), ("title", "title")]
                ),
            ]
        );
        // The hand-written maps stay behind them; the first match wins.
        assert_eq!(m.blocks[2..], cfg().mapping.blocks[..]);
    }

    #[test]
    fn hand_renames_survive_compile() {
        let mut c = cfg();
        let tm = c.mapping.types.get_mut("cityPage").unwrap();
        tm.fields.get_mut("seo").unwrap().to = "meta".into();
        tm.path = Some("/d/{slug}/".into());
        tm.status.insert("draft".into(), "skip".into());
        let m = effective_mapping(&c).unwrap();
        let tm = &m.types["cityPage"];
        assert_eq!(tm.fields["seo"].to, "meta");
        assert_eq!(tm.path.as_deref(), Some("/d/{slug}/"));
        assert_eq!(tm.status["draft"], "skip");
        assert_eq!(tm.to.as_deref(), Some("destinations"));
    }

    #[test]
    fn discriminator_may_not_overwrite_a_mapped_field() {
        let mut c = cfg();
        c.mapping
            .types
            .get_mut("cityPage")
            .unwrap()
            .fields
            .get_mut("addAnchor")
            .unwrap()
            .to = "kind".into();
        let e = effective_mapping(&c).unwrap_err().to_string();
        assert_eq!(
            e,
            "tidy \"destinations\": discriminator field kind is already used by cityPage.addAnchor; pick another name"
        );
        // A dropped field sends nothing, so it can't clash.
        c.tidy[0].enabled = false;
        assert!(effective_mapping(&c).is_ok());
    }

    #[test]
    fn bad_rules_error() {
        let mut c = cfg();
        c.mapping.types.shift_remove("cityPage");
        let e = effective_mapping(&c).unwrap_err().to_string();
        assert!(e.contains("cityPage"), "{e}");
        let mut c = cfg();
        if let crate::tidy::TidyKind::MergeTypes { discriminator, .. } = &mut c.tidy[0].kind {
            discriminator
                .as_mut()
                .unwrap()
                .values
                .shift_remove("cityPage");
        }
        let e = effective_mapping(&c).unwrap_err().to_string();
        assert!(e.contains("discriminator") && e.contains("cityPage"), "{e}");
    }
}
