use super::{NOW, RunId, Store, wire};
use anyhow::Result;
use porter_model::{ItemId, ItemKind, ItemStatus, SourceRef, Stage};
use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

impl Store {
    /// Upserts by `id`; a soft-deleted row comes back to life.
    pub fn put_item<T: Serialize>(
        &self,
        kind: ItemKind,
        id: ItemId,
        src: &SourceRef,
        data: &T,
    ) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO items (id, kind, connector, source_id, url, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, connector = excluded.connector,
                   source_id = excluded.source_id, url = excluded.url, data = excluded.data,
                   updated_at = {NOW}, deleted_at = NULL"
            ),
            params![id.to_string(), kind.as_str(), src.connector, src.source_id, src.url, serde_json::to_string(data)?],
        )?;
        if matches!(kind, ItemKind::Route | ItemKind::Document) {
            self.drop_url_index();
        }
        Ok(())
    }

    pub fn get_item<T: DeserializeOwned>(&self, id: ItemId) -> Result<Option<T>> {
        let data: Option<String> = self
            .conn()
            .query_row(
                "SELECT data FROM items WHERE id = ?1 AND deleted_at IS NULL",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(data.map(|d| serde_json::from_str(&d)).transpose()?)
    }

    pub fn items<T: DeserializeOwned>(
        &self,
        kind: ItemKind,
    ) -> Result<Vec<(ItemId, SourceRef, T)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, connector, source_id, url, data FROM items
             WHERE kind = ?1 AND deleted_at IS NULL ORDER BY rowid",
        )?;
        let rows = stmt.query_map([kind.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, connector, source_id, url, data) = row?;
            out.push((
                id.parse()?,
                SourceRef {
                    connector,
                    source_id,
                    url,
                },
                serde_json::from_str(&data)?,
            ));
        }
        Ok(out)
    }

    /// Like `items`, but rows whose data is not a `T` are left out. Reports use it because
    /// before normalise the rows hold connectors' raw summaries, not neutral-model values.
    pub fn items_decodable<T: DeserializeOwned>(
        &self,
        kind: ItemKind,
    ) -> Result<Vec<(ItemId, SourceRef, T)>> {
        Ok(self
            .items::<serde_json::Value>(kind)?
            .into_iter()
            .filter_map(|(id, src, v)| Some((id, src, serde_json::from_value(v).ok()?)))
            .collect())
    }

    pub fn set_status(
        &self,
        run: RunId,
        stage: Stage,
        id: ItemId,
        status: &ItemStatus,
    ) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO statuses (stage, item_id, run_id, status) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(stage, item_id) DO UPDATE SET run_id = excluded.run_id,
                   status = excluded.status, updated_at = {NOW}, deleted_at = NULL"
            ),
            params![
                wire(&stage)?,
                id.to_string(),
                run.0.to_string(),
                serde_json::to_string(status)?
            ],
        )?;
        Ok(())
    }

    pub fn status(&self, stage: Stage, id: ItemId) -> Result<Option<ItemStatus>> {
        let s: Option<String> = self
            .conn()
            .query_row(
                "SELECT status FROM statuses WHERE stage = ?1 AND item_id = ?2 AND deleted_at IS NULL",
                params![wire(&stage)?, id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(s.map(|s| serde_json::from_str(&s)).transpose()?)
    }

    /// Every status written for `stage`.
    pub fn statuses(&self, stage: Stage) -> Result<Vec<(ItemId, ItemStatus)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT item_id, status FROM statuses WHERE stage = ?1 AND deleted_at IS NULL ORDER BY item_id",
        )?;
        let rows = stmt.query_map([wire(&stage)?], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, status) = row?;
            out.push((id.parse()?, serde_json::from_str(&status)?));
        }
        Ok(out)
    }

    /// `(id, kind, connector)` of every live item, for reports.
    pub fn item_rows(&self) -> Result<Vec<(ItemId, String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, kind, connector FROM items WHERE deleted_at IS NULL ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, connector) = row?;
            out.push((id.parse()?, kind, connector));
        }
        Ok(out)
    }
}
