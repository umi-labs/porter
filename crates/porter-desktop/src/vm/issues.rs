//! Issues: the filterable table, a selection, bulk resolve (handled manually needs a note,
//! as in the TUI) and the jump to the item an issue is about. All issues are read once and
//! filtered here, so the counts are exact. Shared with the Destination view.

use std::collections::BTreeSet;

use porter_model::{Issue, IssueKind, ItemKind, Resolution, Severity, Stage};
use uuid::Uuid;

/// Which resolutions the table shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Open,
    Closed,
    All,
}

/// What the resolve form can do to the selected issues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    HandledManually,
    WontFix,
    Reopen,
}

impl Choice {
    /// What was done, for the report: never "resolved" for a reopen.
    pub fn done(self) -> &'static str {
        match self {
            Choice::HandledManually => "marked handled",
            Choice::WontFix => "marked won't fix",
            Choice::Reopen => "reopened",
        }
    }
}

/// Where "jump to item" goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    Content,
    Media,
}

#[derive(Default)]
pub struct Issues {
    /// Bumped when the store changes; replies to older reads are dropped.
    pub generation: u64,
    asked: bool,
    loading: bool,
    all: Vec<Issue>,
    error: Option<String>,
    pub kind: Option<IssueKind>,
    pub severity: Option<Severity>,
    pub status: Status,
    pub text: String,
    selected: BTreeSet<Uuid>,
    /// How the last resolve (or jump, or open) went, kept verbatim.
    pub note: Option<String>,
}

pub fn kind_name(k: IssueKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// The view a jump lands in; items of other kinds have no view.
pub fn jump_target(k: ItemKind) -> Result<Jump, String> {
    match k {
        ItemKind::Document => Ok(Jump::Content),
        ItemKind::Media => Ok(Jump::Media),
        other => Err(format!("no view shows {} items", other.as_str())),
    }
}

impl Issues {
    /// The section was shown: true when the issues must be read (first time only).
    pub fn open(&mut self) -> bool {
        if self.asked {
            return false;
        }
        self.asked = true;
        self.loading = true;
        true
    }

    /// The store changed: true when what is shown must be read again.
    pub fn invalidate(&mut self) -> bool {
        if !self.asked {
            return false;
        }
        self.generation += 1;
        self.loading = true;
        self.error = None;
        true
    }

    pub fn land(&mut self, generation: u64, result: Result<Vec<Issue>, String>) {
        if generation != self.generation || !self.loading {
            return;
        }
        self.loading = false;
        match result {
            Ok(v) => {
                self.all = v;
                let ids: BTreeSet<Uuid> = self.all.iter().map(|i| i.id).collect();
                self.selected.retain(|id| ids.contains(id));
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn all(&self) -> &[Issue] {
        &self.all
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn loaded(&self) -> bool {
        self.asked && !self.loading
    }

    /// The issues the filters keep, in store order.
    pub fn visible(&self) -> Vec<&Issue> {
        let text = self.text.trim().to_lowercase();
        self.all
            .iter()
            .filter(|i| {
                self.kind.is_none_or(|k| k == i.kind)
                    && self.severity.is_none_or(|s| s == i.severity)
                    && match self.status {
                        Status::Open => i.resolution == Resolution::Open,
                        Status::Closed => i.resolution != Resolution::Open,
                        Status::All => true,
                    }
                    && (text.is_empty()
                        || i.message.to_lowercase().contains(&text)
                        || kind_name(i.kind).contains(&text))
            })
            .collect()
    }

    pub fn is_selected(&self, id: Uuid) -> bool {
        self.selected.contains(&id)
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub fn toggle(&mut self, id: Uuid) {
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
    }

    /// Selects every visible issue, or clears the selection when all of them already are.
    pub fn toggle_all(&mut self) {
        let ids: Vec<Uuid> = self.visible().iter().map(|i| i.id).collect();
        if !ids.is_empty() && ids.iter().all(|id| self.selected.contains(id)) {
            for id in ids {
                self.selected.remove(&id);
            }
        } else {
            self.selected.extend(ids);
        }
    }

    /// What pressing Resolve would do: the selected ids and the resolution, or why not.
    pub fn plan(&self, choice: Choice, note: &str) -> Result<(Vec<Uuid>, Resolution), String> {
        if self.selected.is_empty() {
            return Err("select at least one issue".into());
        }
        let r = match choice {
            Choice::HandledManually if note.trim().is_empty() => {
                return Err("a note is required for \"handled manually\"".into());
            }
            Choice::HandledManually => Resolution::HandledManually {
                note: note.trim().to_string(),
            },
            Choice::WontFix => Resolution::WontFix,
            Choice::Reopen => Resolution::Open,
        };
        Ok((self.selected.iter().copied().collect(), r))
    }

    /// Takes the per-issue results of a bulk resolve: resolved issues leave the selection,
    /// failed ones stay selected. Returns the report (never "all resolved" when some failed).
    pub fn resolved(&mut self, choice: Choice, results: Vec<(Uuid, Result<(), String>)>) -> String {
        let total = results.len();
        let mut failed = vec![];
        for (id, r) in results {
            match r {
                Ok(()) => {
                    self.selected.remove(&id);
                }
                Err(e) => {
                    let what = self
                        .all
                        .iter()
                        .find(|i| i.id == id)
                        .map_or(id.to_string(), |i| {
                            format!("{}: {}", kind_name(i.kind), i.message)
                        });
                    failed.push(format!("{what} ({e})"));
                }
            }
        }
        let mut s = format!("{} {} of {total}", choice.done(), total - failed.len());
        if !failed.is_empty() {
            s += &format!("; {} failed: {}", failed.len(), failed.join("; "));
        }
        s
    }

    /// The bulk call itself failed (not one issue): nothing is confirmed, the selection stays.
    pub fn resolve_failed(&self, error: &str) -> String {
        format!("bulk update failed: {error} — nothing confirmed")
    }

    /// "12 of 340 issues match, 3 selected".
    pub fn summary(&self) -> String {
        if !self.loaded() {
            return "Loading…".into();
        }
        format!(
            "{} of {} issues match, {} selected",
            self.visible().len(),
            self.all.len(),
            self.selected.len()
        )
    }
}

/// True for the issues the schema check opened, which the Destination view groups.
pub fn is_schema_check(i: &Issue) -> bool {
    i.stage == Stage::Schema && i.kind == IssueKind::SchemaMismatch
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(n: u128, kind: IssueKind, sev: Severity, res: Resolution) -> Issue {
        Issue {
            id: Uuid::from_u128(n),
            kind,
            severity: sev,
            item: None,
            stage: Stage::Normalise,
            message: format!("message {n}"),
            details: serde_json::json!({}),
            resolution: res,
        }
    }

    fn loaded(v: Vec<Issue>) -> Issues {
        let mut s = Issues::default();
        assert!(s.open());
        s.land(s.generation, Ok(v));
        s
    }

    fn three() -> Issues {
        loaded(vec![
            issue(
                1,
                IssueKind::MissingMedia,
                Severity::Error,
                Resolution::Open,
            ),
            issue(
                2,
                IssueKind::UnmappedField,
                Severity::Warning,
                Resolution::Open,
            ),
            issue(
                3,
                IssueKind::MissingMedia,
                Severity::Warning,
                Resolution::WontFix,
            ),
        ])
    }

    #[test]
    fn bulk_resolve_requires_note_for_handled() {
        let mut s = three();
        // Nothing selected: refused.
        assert!(s.plan(Choice::WontFix, "").is_err());
        s.toggle_all();
        assert_eq!(s.selected_count(), 2, "only the visible (open) issues");
        // Handled manually needs a non-blank note.
        let e = s.plan(Choice::HandledManually, "  \n ").unwrap_err();
        assert!(e.contains("note is required"), "{e}");
        let (ids, r) = s.plan(Choice::HandledManually, "  fixed in CMS ").unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(
            r,
            Resolution::HandledManually {
                note: "fixed in CMS".into()
            }
        );
        // The others need none.
        assert_eq!(s.plan(Choice::WontFix, "").unwrap().1, Resolution::WontFix);
        assert_eq!(s.plan(Choice::Reopen, "").unwrap().1, Resolution::Open);
    }

    #[test]
    fn bulk_resolve_reports_partial_failures() {
        let mut s = three();
        s.toggle_all();
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let report = s.resolved(
            Choice::HandledManually,
            vec![(a, Ok(())), (b, Err("database is locked".into()))],
        );
        assert!(report.starts_with("marked handled 1 of 2"), "{report}");
        assert!(report.contains("1 failed"), "{report}");
        assert!(report.contains("database is locked"), "{report}");
        assert!(!s.is_selected(a));
        assert!(s.is_selected(b), "a failed one stays selected for a retry");
        let all_ok = s.resolved(Choice::WontFix, vec![(b, Ok(()))]);
        assert_eq!(all_ok, "marked won't fix 1 of 1");
    }

    #[test]
    fn report_names_the_action() {
        let mut s = three();
        s.toggle_all();
        let ids: Vec<_> = s.selected.iter().copied().collect();
        let ok = |c| s_clone(c, &ids);
        fn s_clone(c: Choice, ids: &[Uuid]) -> String {
            let mut s = three();
            s.toggle_all();
            s.resolved(c, ids.iter().map(|i| (*i, Ok(()))).collect())
        }
        assert_eq!(ok(Choice::Reopen), "reopened 2 of 2");
        assert_eq!(ok(Choice::HandledManually), "marked handled 2 of 2");
    }

    #[test]
    fn failed_bulk_call_confirms_nothing() {
        let mut s = three();
        s.toggle_all();
        let m = s.resolve_failed("task panicked");
        assert_eq!(m, "bulk update failed: task panicked — nothing confirmed");
        assert_eq!(s.selected_count(), 2);
    }

    #[test]
    fn filters_narrow_and_count() {
        let mut s = three();
        assert_eq!(s.visible().len(), 2);
        assert_eq!(s.summary(), "2 of 3 issues match, 0 selected");
        s.status = Status::All;
        s.kind = Some(IssueKind::MissingMedia);
        assert_eq!(s.visible().len(), 2);
        s.severity = Some(Severity::Error);
        assert_eq!(s.visible().len(), 1);
        s.text = "MESSAGE 1".into();
        assert_eq!(s.visible().len(), 1);
        s.text = "nothing".into();
        assert!(s.visible().is_empty());
    }

    #[test]
    fn stale_reads_are_dropped_and_selection_follows_the_store() {
        let mut s = three();
        s.toggle(Uuid::from_u128(1));
        assert!(s.invalidate());
        let old = s.generation - 1;
        s.land(old, Ok(vec![]));
        assert!(!s.loaded(), "a reply to the old generation is ignored");
        // The issue was resolved elsewhere: it leaves the selection with the next read.
        s.land(
            s.generation,
            Ok(vec![issue(
                2,
                IssueKind::MissingMedia,
                Severity::Error,
                Resolution::Open,
            )]),
        );
        assert_eq!(s.selected_count(), 0);
    }

    #[test]
    fn jump_goes_to_documents_and_media_only() {
        assert_eq!(jump_target(ItemKind::Document), Ok(Jump::Content));
        assert_eq!(jump_target(ItemKind::Media), Ok(Jump::Media));
        assert!(jump_target(ItemKind::Route).unwrap_err().contains("route"));
    }
}
