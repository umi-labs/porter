use super::{NOW, RunId, Store, wire};
use anyhow::Result;
use porter_model::{ItemId, ItemKind, ItemStatus, Route, SourceRef, Stage};
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

    /// Document types among `connector`'s live documents, with counts, by type.
    pub fn doc_types(&self, connector: &str) -> Result<Vec<(String, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT coalesce(json_extract(data, '$.type'), ''), COUNT(*) FROM items
             WHERE kind = 'document' AND connector = ?1 AND deleted_at IS NULL
             GROUP BY 1 ORDER BY 1",
        )?;
        let rows = stmt.query_map([connector], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (t, n) = row?;
            out.push((t, u64::try_from(n).unwrap_or(0)));
        }
        Ok(out)
    }

    /// One page of `connector`'s live documents under `parent`, by title, with how many
    /// there are. With no parent: those with none, or whose parent is not a live document
    /// of `connector`. Rows are `(id, title, type, path, has_children)`.
    #[allow(clippy::type_complexity)]
    pub fn doc_children(
        &self,
        connector: &str,
        parent: Option<ItemId>,
        doc_type: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<(ItemId, String, String, String, bool)>, u64)> {
        // Spelled out per case (not `?2 IS NULL OR ...`) so SQLite uses the v5 indexes.
        let under = match parent {
            Some(_) => "json_extract(d.data, '$.parent') = ?2",
            None => {
                "(json_extract(d.data, '$.parent') IS NULL OR NOT EXISTS (
                    SELECT 1 FROM items p WHERE p.id = json_extract(d.data, '$.parent')
                    AND p.kind = 'document' AND p.connector = ?1 AND p.deleted_at IS NULL))"
            }
        };
        let typed = match doc_type {
            Some(_) => "AND json_extract(d.data, '$.type') = ?3",
            None => "AND ?3 IS NULL",
        };
        let from = format!(
            "FROM items d WHERE d.kind = 'document' AND d.connector = ?1 AND d.deleted_at IS NULL
             AND {under} {typed}"
        );
        let parent = parent.map(|p| p.to_string());
        let conn = self.conn();
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(*) {from}"),
            params![connector, parent, doc_type],
            |r| r.get(0),
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT d.id, coalesce(json_extract(d.data, '$.title'), ''),
                    coalesce(json_extract(d.data, '$.type'), ''),
                    coalesce(json_extract(d.data, '$.path'), ''),
                    -- `+d.id` drops the column's affinity so the parent index is searched.
                    EXISTS (SELECT 1 FROM items c WHERE c.kind = 'document'
                        AND json_extract(c.data, '$.parent') = +d.id
                        AND c.connector = ?1 AND c.deleted_at IS NULL)
             {from} ORDER BY lower(json_extract(d.data, '$.title')), d.id LIMIT ?4 OFFSET ?5"
        ))?;
        let rows = stmt.query_map(
            params![
                connector,
                parent,
                doc_type,
                i64::try_from(limit).unwrap_or(i64::MAX),
                i64::try_from(offset).unwrap_or(i64::MAX)
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (id, title, ty, path, kids) = row?;
            out.push((id.parse()?, title, ty, path, kids));
        }
        Ok((out, u64::try_from(total).unwrap_or(0)))
    }

    /// Every stage's status for each of `ids`.
    pub fn statuses_of(&self, ids: &[ItemId]) -> Result<Vec<(ItemId, ItemStatus)>> {
        let ids: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT item_id, status FROM statuses
             WHERE item_id IN (SELECT value FROM json_each(?1)) AND deleted_at IS NULL",
        )?;
        let rows = stmt.query_map([serde_json::to_string(&ids)?], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, status) = row?;
            out.push((id.parse()?, serde_json::from_str(&status)?));
        }
        Ok(out)
    }
}

/// What the URL table narrows to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RouteFilter {
    #[default]
    All,
    /// A 2xx (or unfetched) route that points nowhere, or at an item that is not there.
    Unresolved,
    /// A route whose redirect went through more than one hop.
    Chains,
    /// Old status 404.
    NotFound,
}

impl Store {
    /// One page of `connector`'s live media items by filename: `(id, hash, filename, mime,
    /// variant count)`, and how many there are.
    #[allow(clippy::type_complexity)]
    pub fn media_page(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<(ItemId, String, String, String, u64)>, u64)> {
        const FROM: &str =
            "FROM items WHERE kind = 'media' AND connector = 'media' AND deleted_at IS NULL";
        let conn = self.conn();
        let total: i64 = conn.query_row(&format!("SELECT COUNT(*) {FROM}"), [], |r| r.get(0))?;
        let mut stmt = conn.prepare(&format!(
            "SELECT id, coalesce(json_extract(data, '$.hash'), ''),
                    coalesce(json_extract(data, '$.filename'), ''),
                    coalesce(json_extract(data, '$.mime'), ''),
                    coalesce(json_array_length(data, '$.variants'), 0)
             {FROM} ORDER BY lower(json_extract(data, '$.filename')), id LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = stmt.query_map(
            params![
                i64::try_from(limit).unwrap_or(i64::MAX),
                i64::try_from(offset).unwrap_or(i64::MAX)
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (id, hash, name, mime, n) = row?;
            out.push((id.parse()?, hash, name, mime, u64::try_from(n).unwrap_or(0)));
        }
        Ok((out, u64::try_from(total).unwrap_or(0)))
    }

    /// One page of `connector`'s live routes by URL that match `filter`: the stored route
    /// and whether its redirect is a chain; then how many match, and how many there are.
    #[allow(clippy::type_complexity)]
    pub fn route_page(
        &self,
        connector: &str,
        filter: RouteFilter,
        offset: u64,
        limit: u64,
    ) -> Result<(Vec<(ItemId, Route, bool)>, u64, u64)> {
        // The target's id, whichever kind it is ("none" is a bare string, so NULL).
        const TID: &str = "coalesce(json_extract(r.data, '$.target.document'),
            json_extract(r.data, '$.target.media'), json_extract(r.data, '$.target.redirect'))";
        const STATUS: &str = "json_extract(r.data, '$.http_status')";
        let chain = "EXISTS (SELECT 1 FROM issues i WHERE i.kind = 'redirect-chain'
            AND i.deleted_at IS NULL AND i.item_id = json_extract(r.data, '$.target.redirect'))";
        let cond = match filter {
            RouteFilter::All => "1".to_string(),
            RouteFilter::Unresolved => format!(
                "(({TID} IS NULL AND ({STATUS} IS NULL OR {STATUS} BETWEEN 200 AND 299))
                  OR ({TID} IS NOT NULL AND NOT EXISTS (SELECT 1 FROM items t
                      WHERE t.id = {TID} AND t.deleted_at IS NULL)))"
            ),
            RouteFilter::Chains => chain.to_string(),
            RouteFilter::NotFound => format!("{STATUS} = 404"),
        };
        let base =
            "FROM items r WHERE r.kind = 'route' AND r.connector = ?1 AND r.deleted_at IS NULL";
        let conn = self.conn();
        let count = |extra: &str| -> Result<u64> {
            let n: i64 = conn.query_row(
                &format!("SELECT COUNT(*) {base} AND {extra}"),
                [connector],
                |r| r.get(0),
            )?;
            Ok(u64::try_from(n).unwrap_or(0))
        };
        let (matching, total) = (count(&cond)?, count("1")?);
        let mut stmt = conn.prepare(&format!(
            "SELECT r.id, r.data, {chain} {base} AND {cond}
             ORDER BY lower(coalesce(json_extract(r.data, '$.url'), r.url, '')), r.id LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(
            params![
                connector,
                i64::try_from(limit).unwrap_or(i64::MAX),
                i64::try_from(offset).unwrap_or(i64::MAX)
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (id, data, chain) = row?;
            out.push((id.parse()?, serde_json::from_str(&data)?, chain));
        }
        Ok((out, matching, total))
    }
}
