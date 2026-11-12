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

    /// Marks the item deleted (it stops being read); a later `put_item` revives it.
    pub fn soft_delete(&self, id: ItemId) -> Result<()> {
        self.conn().execute(
            &format!("UPDATE items SET deleted_at = {NOW}, updated_at = {NOW} WHERE id = ?1 AND deleted_at IS NULL"),
            [id.to_string()],
        )?;
        self.drop_url_index();
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

    /// Like `items_decodable`, but also returns the rows that are not a `T`, with the
    /// decode error, so a stage can give each one a status instead of losing it.
    #[allow(clippy::type_complexity)]
    pub fn items_decodable_with_failures<T: DeserializeOwned>(
        &self,
        kind: ItemKind,
    ) -> Result<(
        Vec<(ItemId, SourceRef, T)>,
        Vec<(ItemId, SourceRef, String)>,
    )> {
        let mut ok = vec![];
        let mut bad = vec![];
        for (id, src, v) in self.items::<serde_json::Value>(kind)? {
            match serde_json::from_value(v) {
                Ok(t) => ok.push((id, src, t)),
                Err(e) => bad.push((id, src, e.to_string())),
            }
        }
        Ok((ok, bad))
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

impl Store {
    /// One page of live items in insertion order, with the number that match. `kind`
    /// narrows by kind; `text` keeps rows whose source id, url or title contains it
    /// (case-insensitive). Rows are `(id, kind, label, title)`: the label is the url,
    /// else the source id.
    #[allow(clippy::type_complexity)]
    pub fn item_page(
        &self,
        kind: Option<ItemKind>,
        text: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<(ItemId, ItemKind, String, Option<String>)>, u64)> {
        const WHERE: &str = "WHERE deleted_at IS NULL AND (?1 IS NULL OR kind = ?1)
             AND (?2 IS NULL OR instr(lower(source_id || ' ' || coalesce(url, '') || ' ' ||
                  coalesce(json_extract(data, '$.title'), '')), lower(?2)) > 0)";
        let kind = kind.map(|k| k.as_str());
        let conn = self.conn();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM items {WHERE}"),
            params![kind, text],
            |r| r.get(0),
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT id, kind, coalesce(url, source_id),
                    CASE WHEN json_type(data, '$.title') = 'text' THEN json_extract(data, '$.title') END
             FROM items {WHERE} ORDER BY rowid LIMIT ?3 OFFSET ?4"
        ))?;
        let rows = stmt.query_map(
            params![
                kind,
                text,
                i64::try_from(limit).unwrap_or(i64::MAX),
                i64::try_from(offset).unwrap_or(i64::MAX)
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, label, title) = row?;
            out.push((id.parse()?, super::from_wire(kind)?, label, title));
        }
        Ok((out, u64::try_from(total).unwrap_or(0)))
    }

    /// The item's status in every stage that wrote one.
    pub fn item_statuses(&self, id: ItemId) -> Result<Vec<(Stage, ItemStatus)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT stage, status FROM statuses WHERE item_id = ?1 AND deleted_at IS NULL",
        )?;
        let rows = stmt.query_map([id.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (stage, status) = row?;
            out.push((super::from_wire(stage)?, serde_json::from_str(&status)?));
        }
        Ok(out)
    }

    /// The item's kind, source identity and stored data, if it is live.
    #[allow(clippy::type_complexity)]
    pub fn item_source(
        &self,
        id: ItemId,
    ) -> Result<Option<(ItemKind, SourceRef, serde_json::Value)>> {
        let row: Option<(String, String, String, Option<String>, String)> = self
            .conn()
            .query_row(
                "SELECT kind, connector, source_id, url, data FROM items WHERE id = ?1 AND deleted_at IS NULL",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        row.map(|(kind, connector, source_id, url, data)| {
            Ok((
                super::from_wire(kind)?,
                SourceRef {
                    connector,
                    source_id,
                    url,
                },
                serde_json::from_str(&data)?,
            ))
        })
        .transpose()
    }
}
