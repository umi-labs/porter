use super::{NOW, RunId, Store, from_wire, wire};
use anyhow::{Result, bail};
use porter_model::{Issue, IssueKind, Resolution, Severity, Stage};
use rusqlite::params;
use rusqlite::types::Value;
use uuid::Uuid;

/// `None` fields match anything. `resolution` matches by variant
/// (a `HandledManually` filter matches any note).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IssueFilter {
    pub stage: Option<Stage>,
    pub kind: Option<IssueKind>,
    pub severity: Option<Severity>,
    pub resolution: Option<Resolution>,
}

impl Store {
    /// Dedupes on `(kind, item, stage, message)`. Reopening an existing issue moves
    /// it to `run`, refreshes severity/details, and turns `FixedByRerun` back into
    /// `Open`; `HandledManually` and `WontFix` are kept.
    pub fn open_issue(&self, run: RunId, issue: Issue) -> Result<Issue> {
        let open = Resolution::Open;
        let key = (
            wire(&issue.kind)?,
            issue.item.map(|i| i.to_string()).unwrap_or_default(),
            wire(&issue.stage)?,
            issue.message.clone(),
        );
        let conn = self.conn();
        conn.execute(
            &format!(
                "INSERT INTO issues (id, run_id, kind, severity, item_id, stage, message, details, resolution, resolution_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(kind, item_id, stage, message) DO UPDATE SET
                   run_id = excluded.run_id, severity = excluded.severity, details = excluded.details,
                   resolution = CASE WHEN resolution = ?11 THEN excluded.resolution ELSE resolution END,
                   resolution_json = CASE WHEN resolution = ?11 THEN excluded.resolution_json ELSE resolution_json END,
                   updated_at = {NOW}, deleted_at = NULL"
            ),
            params![
                issue.id.to_string(),
                run.0.to_string(),
                wire(&issue.kind)?,
                wire(&issue.severity)?,
                issue.item.map(|i| i.to_string()).unwrap_or_default(),
                wire(&issue.stage)?,
                issue.message,
                serde_json::to_string(&issue.details)?,
                wire(&open)?,
                serde_json::to_string(&open)?,
                wire(&Resolution::FixedByRerun)?,
            ],
        )?;
        // The stored row: canonical id and current resolution after any dedupe.
        let (id, severity, details, resolution): (String, String, String, String) = conn
            .query_row(
                "SELECT id, severity, details, resolution_json FROM issues
             WHERE kind = ?1 AND item_id = ?2 AND stage = ?3 AND message = ?4",
                params![key.0, key.1, key.2, key.3],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        Ok(Issue {
            id: id.parse()?,
            severity: from_wire(severity)?,
            details: serde_json::from_str(&details)?,
            resolution: serde_json::from_str(&resolution)?,
            ..issue
        })
    }

    pub fn issues(&self, filter: &IssueFilter) -> Result<Vec<Issue>> {
        let mut sql = String::from(
            "SELECT id, kind, severity, item_id, stage, message, details, resolution_json
             FROM issues WHERE deleted_at IS NULL",
        );
        let mut args: Vec<Value> = Vec::new();
        let mut add = |col: &str, v: Option<String>| {
            if let Some(v) = v {
                args.push(Value::Text(v));
                sql.push_str(&format!(" AND {col} = ?{}", args.len()));
            }
        };
        add("stage", filter.stage.as_ref().map(wire).transpose()?);
        add("kind", filter.kind.as_ref().map(wire).transpose()?);
        add("severity", filter.severity.as_ref().map(wire).transpose()?);
        add(
            "resolution",
            filter.resolution.as_ref().map(wire).transpose()?,
        );
        sql.push_str(" ORDER BY id");

        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, severity, item, stage, message, details, resolution) = row?;
            out.push(Issue {
                id: id.parse()?,
                kind: from_wire(kind)?,
                severity: from_wire(severity)?,
                item: if item.is_empty() {
                    None
                } else {
                    Some(item.parse()?)
                },
                stage: from_wire(stage)?,
                message,
                details: serde_json::from_str(&details)?,
                resolution: serde_json::from_str(&resolution)?,
            });
        }
        Ok(out)
    }

    pub fn resolve_issue(&self, id: Uuid, r: Resolution) -> Result<()> {
        let n = self.conn().execute(
            &format!(
                "UPDATE issues SET resolution = ?2, resolution_json = ?3, updated_at = {NOW}
                 WHERE id = ?1 AND deleted_at IS NULL"
            ),
            params![id.to_string(), wire(&r)?, serde_json::to_string(&r)?],
        )?;
        if n == 0 {
            bail!("no issue {id}");
        }
        Ok(())
    }
}
