//! The apply gate: a real (writing) load runs only when every gate passes (a fresh dry run
//! showed what it would write, no open errors, open warnings acknowledged) and the user
//! typed the destination host. The gates are read again when Apply is pressed. Nothing
//! here ever applies by default.

use std::collections::BTreeMap;
use std::time::SystemTime;

use porter_engine::session::{IssueQuery, RunPlan, Session, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::{IssueKind, Resolution, Severity, Stage};

/// The last dry run as the sheet needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewInfo {
    pub counts: BTreeMap<String, u64>,
    pub at: SystemTime,
    /// Why it can no longer be trusted, if so (`Session::preview_stale`).
    pub stale: Option<String>,
}

/// Everything the sheet checks and lists, read together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApplyFacts {
    /// `destination.url`, verbatim.
    pub url: String,
    pub preview: Option<PreviewInfo>,
    /// Open (errors, warnings); None when the issues could not be read.
    pub open: Option<(u64, u64)>,
    /// What the load leaves out, each with why.
    pub not_written: Vec<String>,
}

/// The last dry run with its staleness verdict. Blocking (files and SQLite): run it off
/// the UI thread.
pub fn preview_info(s: &Session) -> Option<PreviewInfo> {
    let p = s.load_preview_counts()?;
    let stale = s.preview_stale(&p).map(String::from);
    Some(PreviewInfo {
        counts: p.counts,
        at: p.at,
        stale,
    })
}

/// Reads the facts for the sheet. Blocking, like `preview_info`.
pub fn apply_facts(s: &Session) -> ApplyFacts {
    let config = s.config();
    let store = s.project().store();
    let open = store.issues(&IssueQuery {
        resolution: Some(Resolution::Open),
        ..Default::default()
    });
    let mut not_written = vec![];
    let counts = store.doc_types(&config.source.adapter).unwrap_or_default();
    for (ty, m) in &config.mapping.types {
        if m.to.is_some() {
            continue;
        }
        let n = counts.iter().find(|(t, _)| t == ty).map_or(0, |(_, n)| *n);
        let why = m.drop_reason.as_deref().unwrap_or("no reason given");
        not_written.push(format!("{n} {ty} documents: dropped ({why})"));
    }
    if let Ok(issues) = &open {
        let missing = issues
            .iter()
            .filter(|i| i.kind == IssueKind::MissingMedia)
            .count();
        if missing > 0 {
            not_written.push(format!("{missing} media files that were never found"));
        }
    }
    let failed = store
        .runs(Stage::Load)
        .ok()
        .and_then(|mut r| r.pop())
        .filter(|r| r.dry_run)
        .and_then(|r| r.summary)
        .map_or(0, |s| s.failed);
    if failed > 0 {
        not_written.push(format!("{failed} items that failed in the dry run"));
    }
    ApplyFacts {
        url: config.destination.url.clone(),
        preview: preview_info(s),
        open: open.ok().map(|is| {
            let n = |sev| is.iter().filter(|i| i.severity == sev).count() as u64;
            (n(Severity::Error), n(Severity::Warning))
        }),
        not_written,
    }
}

/// "just now", "5 min ago", "2 h ago", "3 d ago".
pub fn age(at: SystemTime, now: SystemTime) -> String {
    let s = now.duration_since(at).map_or(0, |d| d.as_secs());
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

/// One gate: passes or not, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Gate {
    pub pass: bool,
    pub title: String,
    pub detail: String,
}

pub struct ApplySheet {
    pub facts: ApplyFacts,
    host: Option<String>,
    /// How many open warnings the user ticked as reviewed.
    acked: Option<u64>,
    /// What was typed when Apply was pressed, while the recheck is in flight.
    pending: Option<String>,
    /// Why the last Apply was refused.
    pub error: Option<String>,
}

impl ApplySheet {
    pub fn new(facts: ApplyFacts) -> Self {
        let host = url::Url::parse(&facts.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        ApplySheet {
            facts,
            host,
            acked: None,
            pending: None,
            error: None,
        }
    }

    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Writes the dry run says the load will make.
    pub fn writes(&self) -> u64 {
        self.facts
            .preview
            .as_ref()
            .map_or(0, |p| p.counts.values().sum())
    }

    /// No fresh dry run: the sheet offers one in place of the confirm field.
    pub fn needs_dry_run(&self) -> bool {
        self.facts
            .preview
            .as_ref()
            .is_none_or(|p| p.stale.is_some())
    }

    /// The warnings box is shown only when there are open warnings.
    pub fn open_warnings(&self) -> u64 {
        self.facts.open.map_or(0, |(_, w)| w)
    }

    pub fn acked(&self) -> bool {
        self.acked.is_some()
    }

    /// Ticks or clears "I've reviewed the warnings": it covers the warnings open now.
    pub fn toggle_ack(&mut self) {
        self.acked = match self.acked {
            Some(_) => None,
            None => Some(self.open_warnings()),
        };
    }

    /// The three gates, in order: fresh dry run, no open errors, warnings acknowledged.
    pub fn gates(&self) -> [Gate; 3] {
        let g = |pass, title: &str, detail: String| Gate {
            pass,
            title: title.into(),
            detail,
        };
        let dry = match &self.facts.preview {
            None => g(false, "No dry run yet", "Run Load (dry run) first.".into()),
            Some(PreviewInfo {
                stale: Some(why), ..
            }) => g(
                false,
                "The dry run is stale",
                format!("{why}. Its counts no longer describe what Apply would write."),
            ),
            Some(p) => g(
                true,
                "Dry run is fresh",
                format!(
                    "Ran {}. Model and mapping unchanged since.",
                    age(p.at, SystemTime::now())
                ),
            ),
        };
        let errors = match self.facts.open {
            None => g(
                false,
                "Open errors unknown",
                "The issues could not be read.".into(),
            ),
            Some((0, _)) => g(true, "No open errors", "Nothing blocks the load.".into()),
            Some((n, _)) => g(
                false,
                &format!("{n} open error{}", if n == 1 { "" } else { "s" }),
                "Resolve them in Issues first.".into(),
            ),
        };
        let w = self.open_warnings();
        let warnings = match (w, self.acked) {
            (0, _) if self.facts.open.is_some() => {
                g(true, "No open warnings", "Nothing to acknowledge.".into())
            }
            (0, _) => g(
                false,
                "Open warnings unknown",
                "The issues could not be read.".into(),
            ),
            (w, Some(a)) if a >= w => g(
                true,
                &format!("{w} open warnings"),
                "Reviewed and acknowledged.".into(),
            ),
            (w, _) => g(
                false,
                &format!("{w} open warnings"),
                "Tick the box to confirm you have reviewed them.".into(),
            ),
        };
        [dry, errors, warnings]
    }

    /// Ok when Apply may be pressed with `typed` in the box; else why not.
    pub fn check(&self, typed: &str) -> Result<(), String> {
        let Some(host) = self.host() else {
            return Err("destination.url in porter.toml is not a URL with a host".into());
        };
        if let Some(g) = self.gates().into_iter().find(|g| !g.pass) {
            return Err(format!("{}: {}", g.title, g.detail));
        }
        if !typed.trim().eq_ignore_ascii_case(host) {
            return Err(format!("type {host} to confirm"));
        }
        Ok(())
    }

    pub fn confirming(&self) -> bool {
        self.pending.is_some()
    }

    /// Apply was pressed: passes the gate as it stands, then waits for the recheck.
    pub fn begin_confirm(&mut self, typed: &str) -> Result<(), String> {
        let r = self.check(typed);
        self.error = r.as_ref().err().cloned();
        if r.is_ok() {
            self.pending = Some(typed.to_string());
        }
        r
    }

    /// The recheck landed: every gate is judged again on what is true now. The real load
    /// to run, or why it is refused.
    pub fn finish(&mut self, fresh: ApplyFacts) -> Result<RunPlan, String> {
        let typed = self.pending.take();
        let url = fresh.url.clone();
        let changed = url != self.facts.url;
        // The destination shown stays the one confirmed; the rest is what is true now.
        self.facts = ApplyFacts {
            url: self.facts.url.clone(),
            ..fresh
        };
        let r = match typed {
            None => Err("Apply was not pressed".into()),
            // porter.toml changed under the sheet: what was confirmed is not what would run.
            Some(_) if changed => Err(format!(
                "the destination is now {url}, not the one confirmed: open Apply load… again"
            )),
            Some(t) => self.check(&t).map(|()| apply_plan()),
        };
        self.error = r.as_ref().err().cloned();
        r
    }
}

/// The real load. Only `ApplySheet::finish` builds it.
fn apply_plan() -> RunPlan {
    RunPlan::Stage(
        Stage::Load,
        StageOpts {
            refresh: false,
            apply: true,
            schema_mode: SchemaMode::Check,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> PreviewInfo {
        PreviewInfo {
            counts: BTreeMap::from([("pages".to_string(), 3)]),
            at: SystemTime::now(),
            stale: None,
        }
    }

    fn stale() -> PreviewInfo {
        PreviewInfo {
            stale: Some("normalise ran after it".into()),
            ..fresh()
        }
    }

    const URL: &str = "https://cms.example.com/admin";

    fn facts(preview: Option<PreviewInfo>, errors: u64, warnings: u64) -> ApplyFacts {
        ApplyFacts {
            url: URL.into(),
            preview,
            open: Some((errors, warnings)),
            not_written: vec![],
        }
    }

    #[test]
    fn recheck_refuses_a_changed_destination() {
        let mut s = ApplySheet::new(facts(Some(fresh()), 0, 0));
        s.begin_confirm("cms.example.com").unwrap();
        // porter.toml now points elsewhere, even if the dry run still looks fresh.
        let why = s
            .finish(ApplyFacts {
                url: "https://other.example.com/admin".into(),
                ..facts(Some(fresh()), 0, 0)
            })
            .unwrap_err();
        assert!(why.contains("destination"), "{why}");
        assert!(!s.confirming());
    }

    #[test]
    fn apply_sheet_requires_host() {
        let mut s = ApplySheet::new(facts(Some(fresh()), 0, 0));
        assert_eq!(s.host(), Some("cms.example.com"));
        assert_eq!(s.writes(), 3);
        // Never by default: an empty box or the wrong host is refused.
        assert!(s.check("").is_err());
        assert!(s.check("example.com").is_err());
        assert!(s.begin_confirm("cms.example.co").is_err());
        assert!(!s.confirming());
        assert!(s.check(" CMS.example.com ").is_ok());

        // Typed right, but the dry run went stale while the sheet was open: refused.
        s.begin_confirm("cms.example.com").unwrap();
        assert!(s.confirming());
        let why = s.finish(facts(Some(stale()), 0, 0)).unwrap_err();
        assert!(why.contains("normalise ran after it"), "{why}");
        assert!(!s.confirming());
        assert!(s.needs_dry_run());
        // ...and when the dry run has gone away entirely.
        s.facts.preview = Some(fresh());
        s.begin_confirm("cms.example.com").unwrap();
        assert!(s.finish(facts(None, 0, 0)).is_err());

        // Typed right, still fresh at confirm: the real load, and only that.
        s.facts.preview = Some(fresh());
        assert!(!s.needs_dry_run());
        s.begin_confirm("cms.example.com").unwrap();
        assert_eq!(s.finish(facts(Some(fresh()), 0, 0)), Ok(apply_plan()));
        // A recheck with no Apply pressed runs nothing.
        assert!(s.finish(facts(Some(fresh()), 0, 0)).is_err());

        // No dry run, or a stale one, refuses even with the host typed.
        assert!(
            ApplySheet::new(facts(None, 0, 0))
                .check("cms.example.com")
                .is_err()
        );
        assert!(
            ApplySheet::new(facts(Some(stale()), 0, 0))
                .check("cms.example.com")
                .is_err()
        );
        // No usable destination URL: nothing can be typed to pass.
        let none = ApplySheet::new(ApplyFacts {
            url: String::new(),
            ..facts(Some(fresh()), 0, 0)
        });
        assert_eq!(none.host(), None);
        assert!(none.check("").is_err());
    }

    #[test]
    fn apply_blocked_by_open_errors() {
        let mut s = ApplySheet::new(facts(Some(fresh()), 2, 0));
        let [dry, errors, warnings] = s.gates();
        assert!(dry.pass && warnings.pass);
        assert!(!errors.pass);
        assert_eq!(errors.title, "2 open errors");
        let why = s.check("cms.example.com").unwrap_err();
        assert!(why.contains("2 open errors"), "{why}");
        // Errors that appear between opening the sheet and pressing Apply refuse too.
        let mut s2 = ApplySheet::new(facts(Some(fresh()), 0, 0));
        s2.begin_confirm("cms.example.com").unwrap();
        assert!(s2.finish(facts(Some(fresh()), 1, 0)).is_err());
        // Unreadable issues fail closed.
        s.facts.open = None;
        assert!(s.gates().iter().skip(1).all(|g| !g.pass));
        assert!(s.check("cms.example.com").is_err());
    }

    #[test]
    fn apply_requires_warning_ack() {
        let mut s = ApplySheet::new(facts(Some(fresh()), 0, 3));
        assert_eq!(s.open_warnings(), 3);
        assert!(!s.gates()[2].pass);
        assert!(s.check("cms.example.com").is_err());
        s.toggle_ack();
        assert!(s.acked() && s.gates()[2].pass);
        assert!(s.check("cms.example.com").is_ok());
        s.toggle_ack();
        assert!(s.check("cms.example.com").is_err());
        // The tick covers the warnings it was given for: more by the recheck refuses.
        s.toggle_ack();
        s.begin_confirm("cms.example.com").unwrap();
        assert!(s.finish(facts(Some(fresh()), 0, 4)).is_err());
        // ...and stays refused until ticked again. Fewer warnings still pass.
        s.begin_confirm("cms.example.com").unwrap_err();
        let mut s = ApplySheet::new(facts(Some(fresh()), 0, 3));
        s.toggle_ack();
        s.begin_confirm("cms.example.com").unwrap();
        assert!(s.finish(facts(Some(fresh()), 0, 2)).is_ok());
        // No warnings: nothing to tick, the gate passes.
        assert!(ApplySheet::new(facts(Some(fresh()), 0, 0)).gates()[2].pass);
    }
}
