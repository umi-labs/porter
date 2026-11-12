//! The apply gate: a real (writing) load runs only when a dry run has shown what it would
//! write, that dry run is still fresh, and the user typed the destination host. The
//! freshness is checked again when Apply is pressed. Nothing here ever applies by default.

use std::collections::BTreeMap;
use std::time::SystemTime;

use porter_engine::session::{RunPlan, Session, StageOpts};
use porter_engine::stage::SchemaMode;
use porter_model::Stage;

/// The last dry run as the sheet needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewInfo {
    pub counts: BTreeMap<String, u64>,
    pub at: SystemTime,
    /// Why it can no longer be trusted, if so (`Session::preview_stale`).
    pub stale: Option<String>,
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

pub struct ApplySheet {
    /// `destination.url`, shown verbatim.
    pub url: String,
    host: Option<String>,
    pub preview: Option<PreviewInfo>,
    /// What was typed when Apply was pressed, while the recheck is in flight.
    pending: Option<String>,
    /// Why the last Apply was refused.
    pub error: Option<String>,
}

impl ApplySheet {
    pub fn new(url: String, preview: Option<PreviewInfo>) -> Self {
        let host = url::Url::parse(&url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        ApplySheet {
            url,
            host,
            preview,
            pending: None,
            error: None,
        }
    }

    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Ok when Apply may be pressed with `typed` in the box; else why not.
    pub fn check(&self, typed: &str) -> Result<(), String> {
        gate(self.host(), self.preview.as_ref(), typed)
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

    /// The recheck landed: the real load to run, or why it is refused.
    pub fn finish(&mut self, url: &str, fresh: Option<PreviewInfo>) -> Result<RunPlan, String> {
        let typed = self.pending.take();
        self.preview = fresh;
        let r = match typed {
            None => Err("Apply was not pressed".into()),
            // porter.toml changed under the sheet: what was confirmed is not what would run.
            Some(_) if url != self.url => Err(format!(
                "the destination is now {url}, not the one confirmed: open Apply load… again"
            )),
            Some(t) => self.check(&t).map(|()| apply_plan()),
        };
        self.error = r.as_ref().err().cloned();
        r
    }
}

fn gate(host: Option<&str>, preview: Option<&PreviewInfo>, typed: &str) -> Result<(), String> {
    let Some(host) = host else {
        return Err("destination.url in porter.toml is not a URL with a host".into());
    };
    match preview {
        None => return Err("no dry run yet: run Load (dry run) first".into()),
        Some(PreviewInfo {
            stale: Some(why), ..
        }) => {
            return Err(format!(
                "the dry run is stale ({why}): run Load (dry run) again"
            ));
        }
        Some(_) => {}
    }
    if !typed.trim().eq_ignore_ascii_case(host) {
        return Err(format!("type {host} to confirm"));
    }
    Ok(())
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

    #[test]
    fn recheck_refuses_a_changed_destination() {
        let mut s = ApplySheet::new(URL.into(), Some(fresh()));
        s.begin_confirm("cms.example.com").unwrap();
        // porter.toml now points elsewhere, even if the dry run still looks fresh.
        let why = s
            .finish("https://other.example.com/admin", Some(fresh()))
            .unwrap_err();
        assert!(why.contains("destination"), "{why}");
        assert!(!s.confirming());
    }

    #[test]
    fn apply_sheet_requires_host() {
        let url = URL;
        let mut s = ApplySheet::new(url.into(), Some(fresh()));
        assert_eq!(s.host(), Some("cms.example.com"));
        // Never by default: an empty box or the wrong host is refused.
        assert!(s.check("").is_err());
        assert!(s.check("example.com").is_err());
        assert!(s.begin_confirm("cms.example.co").is_err());
        assert!(!s.confirming());
        assert!(s.check(" CMS.example.com ").is_ok());

        // Typed right, but the dry run went stale while the sheet was open: refused.
        s.begin_confirm("cms.example.com").unwrap();
        assert!(s.confirming());
        let why = s.finish(URL, Some(stale())).unwrap_err();
        assert!(why.contains("normalise ran after it"), "{why}");
        assert!(!s.confirming());
        // ...and when the dry run has gone away entirely.
        s.preview = Some(fresh());
        s.begin_confirm("cms.example.com").unwrap();
        assert!(s.finish(URL, None).is_err());

        // Typed right, still fresh at confirm: the real load, and only that.
        s.preview = Some(fresh());
        s.begin_confirm("cms.example.com").unwrap();
        assert_eq!(s.finish(URL, Some(fresh())), Ok(apply_plan()));
        // A recheck with no Apply pressed runs nothing.
        assert!(s.finish(URL, Some(fresh())).is_err());

        // No dry run, or a stale one, refuses even with the host typed.
        assert!(
            ApplySheet::new(url.into(), None)
                .check("cms.example.com")
                .is_err()
        );
        assert!(
            ApplySheet::new(url.into(), Some(stale()))
                .check("cms.example.com")
                .is_err()
        );
        // No usable destination URL: nothing can be typed to pass.
        let none = ApplySheet::new(String::new(), Some(fresh()));
        assert_eq!(none.host(), None);
        assert!(none.check("").is_err());
    }
}
