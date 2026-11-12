use super::{NOW, RunId, Store, wire};
use crate::events::Summary;
use anyhow::{Result, anyhow};
use porter_model::{ItemStatus, Resolution, Severity, Stage};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

impl Store {
    pub fn begin_run(&self, stage: Stage) -> Result<RunId> {
        let id = RunId(Uuid::now_v7());
        self.conn().execute(
            "INSERT INTO runs (id, stage, porter_version) VALUES (?1, ?2, ?3)",
            params![id.0.to_string(), wire(&stage)?, env!("CARGO_PKG_VERSION")],
        )?;
        Ok(id)
    }

    /// Records the summary and resolves issues this run proved gone. An `Open`
    /// issue of the run's stage, not reopened by this run, becomes `FixedByRerun`
    /// only if the run was not interrupted AND this run wrote a status for the
    /// issue's item in this stage. Item-less issues need only a complete
    /// (non-interrupted) run. `HandledManually` and `WontFix` never change.
    pub fn end_run(&self, run: RunId, summary: &Summary) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let run_id = run.0.to_string();
        let stage: String = tx
            .query_row("SELECT stage FROM runs WHERE id = ?1", [&run_id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| anyhow!("unknown run {run_id}"))?;
        tx.execute(
            &format!(
                "UPDATE runs SET ended_at = {NOW}, summary = ?2, updated_at = {NOW} WHERE id = ?1"
            ),
            params![run_id, serde_json::to_string(summary)?],
        )?;
        if summary.interrupted {
            tx.commit()?;
            return Ok(());
        }
        tx.execute(
            &format!(
                "UPDATE issues SET resolution = ?3, resolution_json = ?4, updated_at = {NOW}
                 WHERE stage = ?1 AND run_id != ?2 AND resolution = ?5 AND deleted_at IS NULL
                   AND (item_id = '' OR item_id IN (
                     SELECT item_id FROM statuses
                     WHERE stage = ?1 AND run_id = ?2 AND deleted_at IS NULL))"
            ),
            params![
                stage,
                run_id,
                wire(&Resolution::FixedByRerun)?,
                serde_json::to_string(&Resolution::FixedByRerun)?,
                wire(&Resolution::Open)?,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
}

impl Store {
    /// Counts of statuses this run wrote for `stage` and issues it opened or reopened that are still `Open` (waived issues don't count).
    /// `interrupted` is left false for the caller to set.
    pub fn run_tally(&self, run: RunId, stage: Stage) -> Result<Summary> {
        let conn = self.conn();
        let (run_id, stage) = (run.0.to_string(), wire(&stage)?);
        let mut s = Summary::default();
        let mut stmt = conn.prepare(
            "SELECT status FROM statuses WHERE run_id = ?1 AND stage = ?2 AND deleted_at IS NULL",
        )?;
        for row in stmt.query_map([&run_id, &stage], |r| r.get::<_, String>(0))? {
            match serde_json::from_str(&row?)? {
                ItemStatus::Done => s.done += 1,
                ItemStatus::Partial { .. } => s.partial += 1,
                ItemStatus::Skipped { .. } => s.skipped += 1,
                ItemStatus::Failed { .. } => s.failed += 1,
            }
        }
        let mut stmt = conn.prepare(
            "SELECT severity FROM issues WHERE run_id = ?1 AND stage = ?2 AND resolution = ?3 AND deleted_at IS NULL",
        )?;
        for row in stmt.query_map([&run_id, &stage, &wire(&Resolution::Open)?], |r| {
            r.get::<_, String>(0)
        })? {
            match super::from_wire(row?)? {
                Severity::Error => s.errors += 1,
                Severity::Warning => s.warnings += 1,
                Severity::Info => s.infos += 1,
            }
        }
        Ok(s)
    }
}

impl Store {
    /// Snapshot for `porter status`: item status counts per stage, open issues by
    /// severity, and the latest run per stage.
    pub fn overview(&self) -> Result<serde_json::Value> {
        use serde_json::{Map, Value, json};
        let conn = self.conn();
        let mut stages: Map<String, Value> = Map::new();
        let mut stmt = conn.prepare(
            "SELECT stage, status FROM statuses WHERE deleted_at IS NULL ORDER BY stage",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (stage, status) = row?;
            let tag = wire(&serde_json::from_str::<ItemStatus>(&status)?)?;
            let e = stages.entry(stage).or_insert_with(|| json!({}));
            let n = e[&tag].as_u64().unwrap_or(0) + 1;
            e[&tag] = json!(n);
        }
        let mut open: Map<String, Value> = Map::new();
        let mut stmt = conn.prepare(
            "SELECT severity, COUNT(*) FROM issues WHERE resolution = ?1 AND deleted_at IS NULL GROUP BY severity",
        )?;
        for row in stmt.query_map([wire(&Resolution::Open)?], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (sev, n) = row?;
            open.insert(sev, json!(n));
        }
        let mut last: Map<String, Value> = Map::new();
        let mut stmt = conn.prepare(
            "SELECT stage, id, started_at, ended_at, summary FROM runs WHERE deleted_at IS NULL ORDER BY id",
        )?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })? {
            let (stage, id, started, ended, summary) = row?;
            let summary = summary
                .map(|s| serde_json::from_str::<Value>(&s))
                .transpose()?;
            // ORDER BY id (UUIDv7) so the last row per stage wins.
            last.insert(
                stage,
                json!({"run": id, "started_at": started, "ended_at": ended, "summary": summary}),
            );
        }
        Ok(json!({"stages": stages, "open_issues": open, "last_runs": last}))
    }
}
