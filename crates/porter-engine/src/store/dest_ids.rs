use super::{NOW, Store};
use anyhow::Result;
use porter_model::{ItemId, ItemKind};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

impl Store {
    /// Records where `item` landed; a later call for the same item replaces it.
    pub fn put_dest_id(&self, item: ItemId, collection: &str, id: &Value) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO dest_ids (item_id, collection, dest_id) VALUES (?1, ?2, ?3)
                 ON CONFLICT(item_id) DO UPDATE SET collection = excluded.collection,
                   dest_id = excluded.dest_id, updated_at = {NOW}, deleted_at = NULL"
            ),
            params![item.to_string(), collection, id.to_string()],
        )?;
        Ok(())
    }

    pub fn dest_id(&self, item: ItemId) -> Result<Option<(String, Value)>> {
        let row: Option<(String, String)> = self
            .conn()
            .query_row(
                "SELECT collection, dest_id FROM dest_ids WHERE item_id = ?1 AND deleted_at IS NULL",
                [item.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(c, v)| Ok((c, serde_json::from_str(&v)?)))
            .transpose()
    }

    /// Every `(item, collection, dest id)`.
    pub fn dest_ids(&self) -> Result<Vec<(ItemId, String, Value)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT item_id, collection, dest_id FROM dest_ids WHERE deleted_at IS NULL ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, c, v) = row?;
            out.push((id.parse()?, c, serde_json::from_str(&v)?));
        }
        Ok(out)
    }

    /// Items of `connector` deleted from the store (gone from the source) that still have
    /// a `dest_ids` row: `(item, kind, collection, dest id)`.
    pub fn removed_with_dest(
        &self,
        connector: &str,
    ) -> Result<Vec<(ItemId, ItemKind, String, Value)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.id, i.kind, d.collection, d.dest_id FROM items i
             JOIN dest_ids d ON d.item_id = i.id
             WHERE i.connector = ?1 AND i.deleted_at IS NOT NULL AND d.deleted_at IS NULL
             ORDER BY i.rowid",
        )?;
        let rows = stmt.query_map([connector], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, c, v) = row?;
            out.push((
                id.parse()?,
                super::from_wire(kind)?,
                c,
                serde_json::from_str(&v)?,
            ));
        }
        Ok(out)
    }
}
