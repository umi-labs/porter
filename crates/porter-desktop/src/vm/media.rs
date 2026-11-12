//! Media: a paged, lazily loaded grid of the canonical media items, and the detail of the
//! selected one (variants, usages, issues). Thumbnails are the `thumbs` cache's business;
//! every store read is a `Query` the window hands to the bridge.

use std::collections::{HashMap, HashSet};

use porter_engine::session::{ItemDetail, MediaRow, Page, Paged};
use porter_model::{Issue, ItemId, Media as MediaItem, SourceRef};

use super::content::Labels;

/// Items per page.
pub const PAGE: u64 = 200;

#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    Page(Page),
    Detail(ItemId),
    /// Titles of the documents that use the selected item.
    Labels(Vec<ItemId>),
}

pub enum Reply {
    Page {
        offset: u64,
        result: Result<Paged<MediaRow>, String>,
    },
    Detail(ItemId, Result<Box<ItemDetail>, String>),
    /// None: no live item with that id.
    Labels(Result<Vec<(ItemId, Option<String>)>, String>),
}

/// The selected media item: what is stored, and what it decodes to.
pub struct Detail {
    pub source: SourceRef,
    pub media: Result<MediaItem, String>,
    pub issues: Vec<Issue>,
}

#[derive(Default)]
pub struct Media {
    /// Bumped by `invalidate`; replies to older queries are dropped.
    pub generation: u64,
    asked: bool,
    rows: Vec<MediaRow>,
    total: u64,
    loading: bool,
    error: Option<String>,
    selected: Option<ItemId>,
    detail: Option<Result<Detail, String>>,
    labels: Labels,
    asked_labels: HashSet<ItemId>,
    pub labels_error: Option<String>,
}

impl Media {
    /// The section was shown: reads the first page, once.
    pub fn open(&mut self) -> Vec<Query> {
        if self.asked {
            return vec![];
        }
        self.asked = true;
        self.loading = true;
        vec![Query::Page(Page {
            offset: 0,
            limit: PAGE,
        })]
    }

    /// Reads the next page, when there is one and none is in flight.
    pub fn more(&mut self) -> Vec<Query> {
        if !self.asked || self.loading || !self.has_more() {
            return vec![];
        }
        self.loading = true;
        self.error = None;
        vec![Query::Page(Page {
            offset: self.rows.len() as u64,
            limit: PAGE,
        })]
    }

    pub fn select(&mut self, id: ItemId) -> Vec<Query> {
        self.selected = Some(id);
        self.detail = None;
        vec![Query::Detail(id)]
    }

    /// The store changed: forget what was read and read again what is shown.
    pub fn invalidate(&mut self) -> Vec<Query> {
        if !self.asked {
            return vec![];
        }
        let selected = self.selected;
        *self = Media {
            generation: self.generation + 1,
            ..Default::default()
        };
        let mut q = self.open();
        if let Some(id) = selected {
            q.extend(self.select(id));
        }
        q
    }

    pub fn land(&mut self, generation: u64, reply: Reply) -> Vec<Query> {
        if generation != self.generation {
            return vec![];
        }
        match reply {
            Reply::Page { offset, result } => {
                if !self.loading || offset != self.rows.len() as u64 {
                    return vec![];
                }
                self.loading = false;
                match result {
                    Ok(p) => {
                        self.rows.extend(p.rows);
                        self.total = p.total;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Reply::Detail(id, d) => {
                if self.selected != Some(id) {
                    return vec![];
                }
                self.detail = Some(d.map(|d| {
                    let d = *d;
                    Detail {
                        media: serde_json::from_value(d.data).map_err(|e| {
                            format!("not a normalised media item ({e}); run normalise")
                        }),
                        source: d.source,
                        issues: d.issues,
                    }
                }));
                return self.ask_labels();
            }
            Reply::Labels(r) => match r {
                Ok(found) => {
                    for (id, l) in found {
                        self.asked_labels.remove(&id);
                        self.labels.insert(id, l);
                    }
                }
                Err(e) => self.labels_error = Some(e),
            },
        }
        vec![]
    }

    fn ask_labels(&mut self) -> Vec<Query> {
        let Some(Ok(d)) = &self.detail else {
            return vec![];
        };
        let Ok(m) = &d.media else {
            return vec![];
        };
        let want: Vec<ItemId> = m
            .usages
            .iter()
            .map(|u| u.item)
            .filter(|id| !self.labels.contains_key(id))
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|id| self.asked_labels.insert(*id))
            .collect();
        if want.is_empty() {
            vec![]
        } else {
            vec![Query::Labels(want)]
        }
    }

    pub fn selected(&self) -> Option<ItemId> {
        self.selected
    }

    pub fn detail(&self) -> Option<&Result<Detail, String>> {
        self.detail.as_ref()
    }

    pub fn labels(&self) -> &Labels {
        &self.labels
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn has_more(&self) -> bool {
        (self.rows.len() as u64) < self.total
    }

    /// How many grid rows `cols` columns make of what is loaded.
    pub fn grid_rows(&self, cols: usize) -> usize {
        self.rows.len().div_ceil(cols.max(1))
    }

    /// The items in grid row `row`.
    pub fn grid_row(&self, row: usize, cols: usize) -> &[MediaRow] {
        let cols = cols.max(1);
        let start = (row * cols).min(self.rows.len());
        &self.rows[start..(start + cols).min(self.rows.len())]
    }

    /// True when the grid row `row` is near the end of what is loaded and more exists.
    pub fn near_end(&self, row: usize, cols: usize) -> bool {
        self.has_more() && row + 3 >= self.grid_rows(cols)
    }

    /// "200 of 4,311 loaded", honestly, or why there is nothing.
    pub fn summary(&self) -> String {
        if !self.asked || (self.loading && self.rows.is_empty()) {
            "Loading…".into()
        } else if self.total == 0 {
            "No media yet: run extract and normalise.".into()
        } else if self.has_more() {
            format!("{} of {} media items loaded", self.rows.len(), self.total)
        } else {
            format!("{} media items", self.total)
        }
    }
}

/// One usage line: where the item is used, by title when the title is known.
pub fn usage_line(
    u: &porter_model::Usage,
    labels: &HashMap<ItemId, Option<String>>,
) -> (String, bool) {
    match labels.get(&u.item) {
        Some(Some(t)) => (format!("{t} · {}", u.field), false),
        Some(None) => (
            format!("unresolved: no document {} · {}", u.item, u.field),
            true,
        ),
        None => (format!("resolving… · {}", u.field), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_model::{ItemKind, Usage};

    fn id(n: u32) -> ItemId {
        ItemId::new(
            ItemKind::Media,
            &SourceRef {
                connector: porter_engine::project::MEDIA_CONNECTOR.into(),
                source_id: n.to_string(),
                url: None,
            },
        )
    }

    fn row(n: u32) -> MediaRow {
        MediaRow {
            id: id(n),
            hash: format!("h{n}"),
            filename: format!("p{n}.png"),
            mime: "image/png".into(),
            variants: 1,
        }
    }

    fn page(offset: u64, n: u32, total: u64) -> Reply {
        Reply::Page {
            offset,
            result: Ok(Paged {
                rows: (offset as u32..offset as u32 + n).map(row).collect(),
                total,
            }),
        }
    }

    #[test]
    fn media_pages_lazily_and_grids() {
        let mut m = Media::default();
        assert_eq!(m.summary(), "Loading…");
        assert_eq!(m.open().len(), 1);
        assert!(m.open().is_empty());
        assert!(m.more().is_empty(), "one page in flight at a time");
        m.land(0, page(0, 200, 450));
        assert_eq!(m.summary(), "200 of 450 media items loaded");
        // 200 items at 5 a row is 40 rows; the last row holds 5.
        assert_eq!((m.grid_rows(5), m.grid_row(39, 5).len()), (40, 5));
        assert!(m.grid_row(40, 5).is_empty());
        assert!(!m.near_end(10, 5) && m.near_end(37, 5));
        let q = m.more();
        assert_eq!(
            q,
            [Query::Page(Page {
                offset: 200,
                limit: PAGE
            })]
        );
        // A repeat of the first page is a leftover.
        m.land(0, page(0, 200, 450));
        assert_eq!(m.grid_rows(5), 40);
        m.land(0, page(200, 200, 450));
        assert_eq!(m.more().len(), 1);
        m.land(0, page(400, 50, 450));
        assert_eq!(m.summary(), "450 media items");
        assert!(m.more().is_empty() && !m.near_end(89, 5));
        // A new generation drops late replies.
        let q = m.invalidate();
        assert_eq!(q.len(), 1);
        m.land(0, page(0, 200, 450));
        assert_eq!(m.grid_rows(5), 0);
    }

    #[test]
    fn media_detail_asks_for_usage_titles_once() {
        let mut m = Media::default();
        m.open();
        m.land(0, page(0, 1, 1));
        let doc = ItemId::new(
            ItemKind::Document,
            &SourceRef {
                connector: "w".into(),
                source_id: "d".into(),
                url: None,
            },
        );
        let media = MediaItem {
            id: id(0),
            hash: "h0".into(),
            variants: vec!["https://old.test/a.png".into()],
            filename: "p0.png".into(),
            mime: "image/png".into(),
            width: None,
            height: None,
            alt: None,
            caption: None,
            title: None,
            credit: None,
            folder: None,
            usages: vec![
                Usage {
                    item: doc,
                    field: "hero".into(),
                },
                Usage {
                    item: doc,
                    field: "body".into(),
                },
            ],
        };
        let item = |data| ItemDetail {
            row: porter_engine::session::ItemRow {
                id: id(0),
                kind: ItemKind::Media,
                label: "x".into(),
                title: None,
                statuses: vec![],
            },
            source: SourceRef {
                connector: porter_engine::project::MEDIA_CONNECTOR.into(),
                source_id: "h0".into(),
                url: None,
            },
            data,
            issues: vec![],
        };
        assert_eq!(m.select(id(0)), [Query::Detail(id(0))]);
        // A reply for a different selection is a leftover.
        assert!(
            m.land(
                0,
                Reply::Detail(id(7), Ok(Box::new(item(serde_json::json!({})))))
            )
            .is_empty()
        );
        let q = m.land(
            0,
            Reply::Detail(
                id(0),
                Ok(Box::new(item(serde_json::to_value(&media).unwrap()))),
            ),
        );
        assert_eq!(q, [Query::Labels(vec![doc])], "one title for two usages");
        // Missing document: said so, not hidden.
        m.land(0, Reply::Labels(Ok(vec![(doc, None)])));
        let (line, bad) = usage_line(&media.usages[0], m.labels());
        assert!(bad && line.starts_with("unresolved: no document"), "{line}");
        // Raw (un-normalised) data does not decode: the reason is kept.
        m.select(id(0));
        m.land(
            0,
            Reply::Detail(id(0), Ok(Box::new(item(serde_json::json!({"x": 1}))))),
        );
        assert!(matches!(m.detail(), Some(Ok(d)) if d.media.is_err()));
    }
}
