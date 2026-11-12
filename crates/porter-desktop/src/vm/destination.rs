//! Destination: the schema check's issues grouped by collection, and the schema modes
//! (check, generate, prompt, spec) with the folder each writes to.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use porter_engine::session::{RunPlan, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::{Issue, Resolution, Severity, Stage};

use crate::vm::issues::is_schema_check;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Check,
    Generate,
    Prompt,
    Spec,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Check, Mode::Generate, Mode::Prompt, Mode::Spec];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Check => "Check schema",
            Mode::Generate => "Generate",
            Mode::Prompt => "Prompt",
            Mode::Spec => "Spec",
        }
    }

    /// A normal run of the schema stage in this mode.
    pub fn plan(self) -> RunPlan {
        RunPlan::Stage(
            Stage::Schema,
            StageOpts {
                refresh: false,
                apply: false,
                schema_mode: match self {
                    Mode::Check => SchemaMode::Check,
                    Mode::Generate => SchemaMode::Generate,
                    Mode::Prompt => SchemaMode::Prompt,
                    Mode::Spec => SchemaMode::Spec,
                },
            },
        )
    }

    /// The folder to open when the run finished; the check writes a report only.
    pub fn output_dir(self, project: &Path, reports: &Path) -> Option<PathBuf> {
        match self {
            Mode::Check => None,
            Mode::Generate => Some(project.join("generated").join("payload")),
            Mode::Prompt | Mode::Spec => Some(reports.to_path_buf()),
        }
    }
}

/// Heading for schema issues whose details name no collection.
pub const UNTIED: &str = "(not tied to a collection)";

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub collection: String,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub messages: Vec<(Severity, String)>,
}

/// Open schema-check issues by `details.collection`; the rest go under "(not tied to a
/// collection)", last. Collections sort by name.
pub fn groups(issues: &[Issue]) -> Vec<Group> {
    let mut by: BTreeMap<String, Group> = BTreeMap::new();
    for i in issues
        .iter()
        .filter(|i| is_schema_check(i) && i.resolution == Resolution::Open)
    {
        let name = i
            .details
            .get("collection")
            .and_then(|c| c.as_str())
            .unwrap_or(UNTIED)
            .to_string();
        let g = by.entry(name.clone()).or_insert_with(|| Group {
            collection: name,
            errors: 0,
            warnings: 0,
            infos: 0,
            messages: vec![],
        });
        match i.severity {
            Severity::Error => g.errors += 1,
            Severity::Warning => g.warnings += 1,
            Severity::Info => g.infos += 1,
        }
        g.messages.push((i.severity, i.message.clone()));
    }
    let untied = by.remove(UNTIED);
    by.into_values().chain(untied).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_model::IssueKind;
    use serde_json::json;
    use uuid::Uuid;

    fn issue(n: u128, sev: Severity, details: serde_json::Value) -> Issue {
        Issue {
            id: Uuid::from_u128(n),
            kind: IssueKind::SchemaMismatch,
            severity: sev,
            item: None,
            stage: Stage::Schema,
            message: format!("m{n}"),
            details,
            resolution: Resolution::Open,
        }
    }

    #[test]
    fn destination_groups_by_collection() {
        let mut other_stage = issue(9, Severity::Error, json!({"collection": "posts"}));
        other_stage.stage = Stage::Load;
        let mut closed = issue(8, Severity::Error, json!({"collection": "posts"}));
        closed.resolution = Resolution::WontFix;
        let g = groups(&[
            issue(1, Severity::Error, json!({"collection": "posts"})),
            issue(2, Severity::Warning, json!({"collection": "pages"})),
            issue(
                3,
                Severity::Warning,
                json!({"collection": "posts", "field": "x"}),
            ),
            issue(4, Severity::Info, json!({})),
            other_stage,
            closed,
        ]);
        let names: Vec<_> = g.iter().map(|g| g.collection.as_str()).collect();
        assert_eq!(names, ["pages", "posts", "(not tied to a collection)"]);
        let posts = &g[1];
        assert_eq!((posts.errors, posts.warnings, posts.infos), (1, 1, 0));
        assert_eq!(posts.messages.len(), 2);
        assert_eq!(g[2].infos, 1);
    }

    #[test]
    fn modes_run_the_schema_stage_and_name_their_folder() {
        let p = Path::new("/proj");
        let r = Path::new("/proj/reports");
        for (m, sm) in [
            (Mode::Check, SchemaMode::Check),
            (Mode::Generate, SchemaMode::Generate),
            (Mode::Prompt, SchemaMode::Prompt),
            (Mode::Spec, SchemaMode::Spec),
        ] {
            let RunPlan::Stage(Stage::Schema, o) = m.plan() else {
                panic!("{m:?}")
            };
            assert_eq!(o.schema_mode, sm);
            assert!(!o.apply);
        }
        assert_eq!(Mode::Check.output_dir(p, r), None);
        assert_eq!(
            Mode::Generate.output_dir(p, r),
            Some(PathBuf::from("/proj/generated/payload"))
        );
        assert_eq!(Mode::Prompt.output_dir(p, r), Some(r.to_path_buf()));
        assert_eq!(Mode::Spec.output_dir(p, r), Some(r.to_path_buf()));
    }
}
