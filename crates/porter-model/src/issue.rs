use crate::{ItemId, Stage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IssueKind {
    UnresolvedLink,
    MissingMedia,
    UnmappedField,
    UnsupportedBlock,
    UnresolvedRelation,
    ParentMissing,
    RedirectChain,
    RedirectLoop,
    WriteFailed,
    AccessGap,
    ConverterMissing,
    SchemaMismatch,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    Open,
    FixedByRerun,
    HandledManually { note: String },
    WontFix,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Issue {
    pub id: Uuid,
    pub kind: IssueKind,
    pub severity: Severity,
    pub item: Option<ItemId>,
    pub stage: Stage,
    pub message: String,
    pub details: serde_json::Value,
    pub resolution: Resolution,
}

#[cfg(test)]
mod tests {
    use super::*;
    use IssueKind::*;

    #[test]
    fn issue_kind_wire_names() {
        let all = [
            (UnresolvedLink, "unresolved-link"),
            (MissingMedia, "missing-media"),
            (UnmappedField, "unmapped-field"),
            (UnsupportedBlock, "unsupported-block"),
            (UnresolvedRelation, "unresolved-relation"),
            (ParentMissing, "parent-missing"),
            (RedirectChain, "redirect-chain"),
            (RedirectLoop, "redirect-loop"),
            (WriteFailed, "write-failed"),
            (AccessGap, "access-gap"),
            (ConverterMissing, "converter-missing"),
            (SchemaMismatch, "schema-mismatch"),
        ];
        assert_eq!(all.len(), 12);
        for (k, w) in all {
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{w}\""));
        }
    }
}
