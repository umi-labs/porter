use super::{NOW, RunId, Store, wire};
use crate::events::Summary;
use anyhow::{Result, anyhow};
use porter_model::{ItemStatus, Resolution, Severity, Stage};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

impl Store {
    /// Test helper: records no fingerprint, so the run reads as stale forever. Production
    /// runs go through `begin_run_with`.
    pub fn begin_run(&self, stage: Stage) -> Result<RunId> {
        self.begin_run_with(stage, None)
    }

    /// Starts a run, recording the fingerprint of the config it uses
    /// (`fingerprint::fingerprint`). None reads as unknown, so stale.
    pub fn begin_run_with(&self, stage: Stage, config_fp: Option<&str>) -> Result<RunId> {
        let id = RunId(Uuid::now_v7());
        self.conn().execute(
            "INSERT INTO runs (id, stage, porter_version, config_fp) VALUES (?1, ?2, ?3, ?4)",
            params![
                id.0.to_string(),
                wire(&stage)?,
                env!("CARGO_PKG_VERSION"),
                config_fp
            ],
        )?;
        Ok(id)
    }

    /// What one run left out, grouped: `(what, why, how many)`. `what` is the document
    /// type, or the item kind for anything else; failed items group under "failed". Items
    /// a dry run would write are not left out.
    pub fn run_left_out(&self, run: &str, stage: Stage) -> Result<Vec<(String, String, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN i.kind = 'document'
                    THEN coalesce(json_extract(i.data, '$.type'), 'document') ELSE i.kind END,
                    s.status
             FROM statuses s JOIN items i ON i.id = s.item_id
             WHERE s.run_id = ?1 AND s.stage = ?2 AND s.deleted_at IS NULL",
        )?;
        let mut out: Vec<(String, String, u64)> = vec![];
        for row in stmt.query_map([run, &wire(&stage)?], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (what, status) = row?;
            let why = match serde_json::from_str(&status)? {
                // A dry run records what it would write as a skip: that is written, not left out.
                ItemStatus::Skipped { reason }
                    if reason == crate::destinations::payload::DRY_RUN =>
                {
                    continue;
                }
                ItemStatus::Skipped { reason } => reason,
                ItemStatus::Failed { .. } => "failed".into(),
                ItemStatus::Done | ItemStatus::Partial { .. } => continue,
            };
            match out.iter_mut().find(|(w, r, _)| *w == what && *r == why) {
                Some(e) => e.2 += 1,
                None => out.push((what, why, 1)),
            }
        }
        out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| (&a.0, &a.1).cmp(&(&b.0, &b.1))));
        Ok(out)
    }

    /// Marks a load run as a dry run: it wrote nothing to the destination.
    pub fn mark_dry_run(&self, run: RunId) -> Result<()> {
        self.conn().execute(
            &format!("UPDATE runs SET dry_run = 1, updated_at = {NOW} WHERE id = ?1"),
            [run.0.to_string()],
        )?;
        Ok(())
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
            "SELECT stage, id, started_at, ended_at, summary, dry_run FROM runs WHERE deleted_at IS NULL ORDER BY id",
        )?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, bool>(5)?,
            ))
        })? {
            let (stage, id, started, ended, summary, dry_run) = row?;
            let summary = summary
                .map(|s| serde_json::from_str::<Value>(&s))
                .transpose()?;
            // ORDER BY id (UUIDv7) so the last row per stage wins.
            last.insert(
                stage,
                // A dry-run load wrote nothing: say so, so its counts never read as writes.
                json!({"run": id, "started_at": started, "ended_at": ended, "summary": summary, "dry_run": dry_run}),
            );
        }
        Ok(json!({"stages": stages, "open_issues": open, "last_runs": last}))
    }
}

impl Store {
    /// A load that was not a dry run finished at some point (`Overview::ever_written`).
    pub fn ever_written(&self) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM runs WHERE stage = ?1 AND dry_run = 0 AND summary IS NOT NULL
                 AND deleted_at IS NULL LIMIT 1",
                [wire(&Stage::Load)?],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

/// One row of the run history.
#[derive(Clone, Debug)]
pub struct RunRow {
    pub id: String,
    pub porter_version: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<Summary>,
    /// A load that wrote nothing to the destination.
    pub dry_run: bool,
    /// The config it ran with (`fingerprint::fingerprint`); None before v6 recorded it.
    pub config_fp: Option<String>,
}

impl Store {
    /// Runs of `stage`, oldest first (UUIDv7 order).
    pub fn runs(&self, stage: Stage) -> Result<Vec<RunRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, porter_version, started_at, ended_at, summary, dry_run, config_fp FROM runs
             WHERE stage = ?1 AND deleted_at IS NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([wire(&stage)?], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, porter_version, started_at, ended_at, summary, dry_run, config_fp) = row?;
            out.push(RunRow {
                id,
                porter_version,
                started_at,
                ended_at,
                summary: summary.map(|s| serde_json::from_str(&s)).transpose()?,
                dry_run,
                config_fp,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SCHEMA, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, Store};
    use porter_model::Stage;

    #[test]
    fn runs_record_dry_run_and_v3_databases_migrate() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("project.db");
        // A v3 database with one run in it, as an earlier porter left it.
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!("BEGIN; {SCHEMA} {SCHEMA_V2} {SCHEMA_V3} COMMIT;"))
            .unwrap();
        c.execute(
            "INSERT INTO runs (id, stage, porter_version) VALUES ('0', 'load', '2.0.0')",
            [],
        )
        .unwrap();
        drop(c);
        let s = Store::open(&path, d.path().join("media")).unwrap();
        assert!(
            !s.runs(Stage::Load).unwrap()[0].dry_run,
            "old rows read as real runs"
        );
        let dry = s.begin_run(Stage::Load).unwrap();
        s.mark_dry_run(dry).unwrap();
        let real = s.begin_run(Stage::Load).unwrap();
        let rows = s.runs(Stage::Load).unwrap();
        let get = |id: super::RunId| rows.iter().find(|r| r.id == id.0.to_string()).unwrap();
        assert!(get(dry).dry_run);
        assert!(!get(real).dry_run);
    }

    #[test]
    fn v5_databases_migrate_to_v6_and_old_runs_have_no_fingerprint() {
        use super::super::SCHEMA_V5;
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("project.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "BEGIN; {SCHEMA} {SCHEMA_V2} {SCHEMA_V3} {SCHEMA_V4} {SCHEMA_V5} COMMIT;"
        ))
        .unwrap();
        c.execute(
            "INSERT INTO runs (id, stage, porter_version) VALUES ('0', 'load', '2.0.0')",
            [],
        )
        .unwrap();
        drop(c);
        let s = Store::open(&path, d.path().join("media")).unwrap();
        assert_eq!(s.runs(Stage::Load).unwrap()[0].config_fp, None, "unknown");
        s.begin_run_with(Stage::Load, Some("source=ab")).unwrap();
        assert_eq!(
            s.runs(Stage::Load).unwrap()[1].config_fp.as_deref(),
            Some("source=ab")
        );
        drop(s);
        let c = rusqlite::Connection::open(&path).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 6);
    }

    #[test]
    fn v4_databases_migrate_to_v5_with_the_content_indexes() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("project.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "BEGIN; {SCHEMA} {SCHEMA_V2} {SCHEMA_V3} {SCHEMA_V4} COMMIT;"
        ))
        .unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 4);
        drop(c);
        drop(Store::open(&path, d.path().join("media")).unwrap());
        let c = rusqlite::Connection::open(&path).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 6, "and on to v6");
        for name in ["items_doc_parent", "items_doc_type"] {
            let n: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "{name}");
        }
    }
}
