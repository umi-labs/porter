use super::{NOW, RunId, Store, wire};
use crate::events::Summary;
use anyhow::{Result, anyhow};
use porter_model::{Resolution, Stage};
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

    /// Records the summary, then marks every `Open` issue of the run's stage that
    /// this run did not (re)open as `FixedByRerun`. Other resolutions are untouched.
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
        tx.execute(
            &format!(
                "UPDATE issues SET resolution = ?3, resolution_json = ?4, updated_at = {NOW}
                 WHERE stage = ?1 AND run_id != ?2 AND resolution = ?5 AND deleted_at IS NULL"
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
