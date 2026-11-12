//! Where a count goes. Every count links to the rows it counts: the overview's issue
//! counts and a collection's schema counts open Issues filtered by kind or severity.
//! (The URL chips filter the URL table in place, and the sidebar badges open their
//! section.) This is the plain data of those links; the window follows them.

use porter_model::{IssueKind, Severity};
use serde_json::Value;

/// A count's link: the open issues of this kind and severity (None: any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub kind: Option<IssueKind>,
    pub severity: Option<Severity>,
}

impl Link {
    pub fn kind(k: IssueKind) -> Link {
        Link {
            kind: Some(k),
            severity: None,
        }
    }

    pub fn severity(s: Severity) -> Link {
        Link {
            kind: None,
            severity: Some(s),
        }
    }
}

/// The issue kind for its kebab-case name, as the overview's kind counts carry it.
pub fn kind_from_name(name: &str) -> Option<IssueKind> {
    serde_json::from_value(Value::String(name.into())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::issues::kind_name;

    #[test]
    fn kind_names_round_trip() {
        for k in [
            IssueKind::UnresolvedLink,
            IssueKind::MissingMedia,
            IssueKind::RedirectChain,
            IssueKind::SchemaMismatch,
            IssueKind::WriteFailed,
        ] {
            assert_eq!(kind_from_name(&kind_name(k)), Some(k));
        }
        assert_eq!(kind_from_name("not-a-kind"), None);
        assert_eq!(kind_from_name(""), None);
    }
}
