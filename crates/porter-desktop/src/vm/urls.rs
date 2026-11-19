//! URLs: the old-URL table (old URL, status, target, new URL) with its filters. Rows are
//! paged from the store by the filter, so counts are exact: "N of M" says how many the
//! filter keeps out of how many there are.

use porter_engine::session::{Page, RouteFilter, RouteRow, Routes};
use porter_model::{RouteTarget, Severity};

use crate::ui::chip::ChipState;

/// Routes per page.
pub const PAGE: u64 = 200;

pub struct Query {
    pub filter: RouteFilter,
    pub page: Page,
}

/// What a `Query` read. The error is kept verbatim.
pub struct Reply {
    pub offset: u64,
    pub result: Result<Routes, String>,
}

#[derive(Default)]
pub struct Urls {
    /// Bumped when the filter changes or the store does; replies to older queries are dropped.
    pub generation: u64,
    asked: bool,
    filter: RouteFilter,
    rows: Vec<RouteRow>,
    matching: u64,
    total: u64,
    loading: bool,
    error: Option<String>,
    /// What each filter matched the last time it was read; the chips show these.
    counts: [Option<u64>; 4],
}

/// Why a row needs attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// Nothing it points at: a 2xx route with no target, or a target that is gone.
    Unresolved,
    /// Its redirect went through more than one hop.
    Chain,
    NotFound,
}

impl Flag {
    /// How bad it is: an unresolved route fails to load its link, a chain is slow.
    pub fn level(self) -> Option<Severity> {
        match self {
            Flag::Unresolved => Some(Severity::Error),
            Flag::Chain => Some(Severity::Warning),
            Flag::NotFound => None,
        }
    }

    /// The chip a row's result column shows: a state and its word. A route with no
    /// flag is "in route table", dashed: it is porter's model, not a live check.
    pub fn chip(f: Option<Flag>) -> (ChipState, &'static str) {
        match f {
            Some(Flag::Unresolved) => (ChipState::Errors, "Unresolved"),
            Some(Flag::Chain) => (ChipState::Warnings, "Redirect chain"),
            Some(Flag::NotFound) => (ChipState::Skipped, "404 in source"),
            None => (ChipState::NotRun, "In route table"),
        }
    }
}

/// A row as the table shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct UrlRow {
    pub old: String,
    pub status: String,
    pub target: &'static str,
    pub new_url: String,
    pub flag: Option<Flag>,
}

fn slot(f: RouteFilter) -> usize {
    match f {
        RouteFilter::All => 0,
        RouteFilter::Unresolved => 1,
        RouteFilter::Chains => 2,
        RouteFilter::NotFound => 3,
    }
}

/// An empty table: what to say, and whether clearing the filter would show rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Empty {
    pub title: &'static str,
    pub body: String,
    pub clear_filter: bool,
}

pub fn flag(r: &RouteRow) -> Option<Flag> {
    // `unresolved` is the engine's one definition; the filter counts the same rows.
    if r.unresolved {
        Some(Flag::Unresolved)
    } else if r.chain {
        Some(Flag::Chain)
    } else if r.status == Some(404) {
        Some(Flag::NotFound)
    } else {
        None
    }
}

fn row(r: &RouteRow) -> UrlRow {
    UrlRow {
        old: r.url.clone(),
        status: r.status.map_or("—".into(), |s| s.to_string()),
        target: match r.target {
            RouteTarget::Document(_) => "document",
            RouteTarget::Media(_) => "media",
            RouteTarget::Redirect(_) => "redirect",
            RouteTarget::None => "none",
        },
        new_url: r.new_url.clone().unwrap_or_default(),
        flag: flag(r),
    }
}

impl Urls {
    /// The section was shown: reads the first page, once.
    pub fn open(&mut self) -> Vec<Query> {
        if self.asked {
            return vec![];
        }
        self.asked = true;
        self.loading = true;
        vec![self.query(0)]
    }

    fn query(&self, offset: u64) -> Query {
        Query {
            filter: self.filter,
            page: Page {
                offset,
                limit: PAGE,
            },
        }
    }

    pub fn filter(&self) -> RouteFilter {
        self.filter
    }

    /// Switches the filter and reads its first page; what was loaded is dropped.
    pub fn set_filter(&mut self, f: RouteFilter) -> Vec<Query> {
        if !self.asked || f == self.filter {
            self.filter = f;
            return vec![];
        }
        self.filter = f;
        self.restart()
    }

    fn restart(&mut self) -> Vec<Query> {
        self.generation += 1;
        self.rows.clear();
        self.matching = 0;
        self.counts[slot(self.filter)] = None;
        self.error = None;
        self.loading = true;
        vec![self.query(0)]
    }

    /// Reads the next page.
    pub fn more(&mut self) -> Vec<Query> {
        if !self.asked || self.loading || !self.has_more() {
            return vec![];
        }
        self.loading = true;
        self.error = None;
        vec![self.query(self.rows.len() as u64)]
    }

    /// The store changed: read again what is shown (the filter stays).
    pub fn invalidate(&mut self) -> Vec<Query> {
        if !self.asked {
            return vec![];
        }
        self.total = 0;
        self.restart()
    }

    pub fn land(&mut self, generation: u64, reply: Reply) {
        if generation != self.generation || !self.loading || reply.offset != self.rows.len() as u64
        {
            return;
        }
        self.loading = false;
        match reply.result {
            Ok(r) => {
                self.rows.extend(r.rows);
                self.matching = r.matching;
                self.total = r.total;
                self.counts[slot(self.filter)] = Some(r.matching);
                self.counts[0] = Some(r.total);
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, i: usize) -> Option<UrlRow> {
        self.rows.get(i).map(row)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Some of the matching rows are not loaded yet.
    fn has_more(&self) -> bool {
        (self.rows.len() as u64) < self.matching
    }

    /// "12 of 340 URLs match, 12 shown": the filter's count out of all, never a guess.
    pub fn summary(&self) -> String {
        if !self.asked || (self.loading && self.rows.is_empty()) {
            return "Loading…".into();
        }
        let shown = self.rows.len() as u64;
        let mut s = if self.filter == RouteFilter::All {
            format!("{} URLs", self.total)
        } else {
            format!("{} of {} URLs match", self.matching, self.total)
        };
        if shown < self.matching {
            s += &format!(", {shown} loaded");
        }
        s
    }

    /// The number on `f`'s chip, or None until that filter has been read: "…", never a guess.
    pub fn count(&self, f: RouteFilter) -> Option<u64> {
        self.counts[slot(f)]
    }

    /// The table is still being read and has nothing to show: "Building route table · 0 of 840".
    pub fn loading_text(&self) -> Option<String> {
        if !self.asked || (self.loading && self.rows.is_empty()) {
            return Some(match self.total {
                0 => "Building route table…".into(),
                t => format!("Building route table · {} of {t}", self.rows.len()),
            });
        }
        None
    }

    /// Why the table is empty, when it is.
    pub fn empty(&self) -> Option<Empty> {
        if self.loading || !self.asked || self.error.is_some() || !self.rows.is_empty() {
            return None;
        }
        if self.total == 0 {
            return Some(Empty {
                title: "No URLs yet",
                body: "Run extract and normalise to build the route table.".into(),
                clear_filter: false,
            });
        }
        let (title, what) = match self.filter {
            RouteFilter::All => ("No URLs", "".to_string()),
            RouteFilter::Unresolved => ("No unresolved URLs", "lead somewhere".to_string()),
            RouteFilter::Chains => (
                "No redirect chains in the route table",
                "reach a document directly or in one hop".to_string(),
            ),
            RouteFilter::NotFound => (
                "No old URL returned 404",
                "answered when they were crawled".to_string(),
            ),
        };
        Some(Empty {
            title,
            body: format!(
                "All {} old URLs {what}. This is from porter's model; the live site hasn't been checked.",
                self.total
            ),
            clear_filter: self.filter != RouteFilter::All,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_model::{ItemId, ItemKind, SourceRef};

    fn id(n: u32) -> ItemId {
        ItemId::new(
            ItemKind::Route,
            &SourceRef {
                connector: "t".into(),
                source_id: n.to_string(),
                url: None,
            },
        )
    }

    fn r(n: u32, status: Option<u16>, target: RouteTarget, new_url: Option<&str>) -> RouteRow {
        RouteRow {
            id: id(n),
            url: format!("https://old.test/{n}"),
            status,
            target: target.clone(),
            new_url: new_url.map(Into::into),
            chain: false,
            // As the engine decides it, for these simple fixtures.
            unresolved: if target == RouteTarget::None {
                status.is_none_or(|s| (200..300).contains(&s))
            } else {
                new_url.is_none()
            },
        }
    }

    fn reply(offset: u64, rows: Vec<RouteRow>, matching: u64, total: u64) -> Reply {
        Reply {
            offset,
            result: Ok(Routes {
                rows,
                matching,
                total,
            }),
        }
    }

    #[test]
    fn urls_filter_unresolved() {
        let mut u = Urls::default();
        let q = u.open();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].filter, RouteFilter::All);
        assert!(u.open().is_empty(), "the first page is read once");
        let g = u.generation;
        u.land(
            g,
            reply(0, vec![r(1, Some(200), RouteTarget::None, None)], 1, 5),
        );
        assert_eq!(u.summary(), "5 URLs");

        // Switching the filter asks again from the top and drops what was loaded.
        let q = u.set_filter(RouteFilter::Unresolved);
        assert_eq!(
            (q.len(), q[0].filter, q[0].page.offset),
            (1, RouteFilter::Unresolved, 0)
        );
        assert_eq!(u.len(), 0);
        assert_ne!(u.generation, g);
        // The old filter's late reply is dropped, not mixed in.
        u.land(
            g,
            reply(0, vec![r(2, Some(200), RouteTarget::None, None)], 1, 5),
        );
        assert_eq!(u.len(), 0);
        let g2 = u.generation;
        u.land(
            g2,
            reply(
                0,
                vec![
                    r(1, Some(200), RouteTarget::None, None),
                    r(3, None, RouteTarget::None, None),
                    r(4, Some(301), RouteTarget::Redirect(id(9)), None),
                ],
                3,
                5,
            ),
        );
        // Counts are the store's, honestly: 3 of 5, all loaded.
        assert_eq!(u.summary(), "3 of 5 URLs match");
        assert!(!u.has_more() && u.more().is_empty());
        assert!((0..3).all(|i| u.row(i).unwrap().flag == Some(Flag::Unresolved)));
        assert_eq!(u.row(2).unwrap().target, "redirect");
        assert_eq!(u.row(1).unwrap().status, "—");
    }

    #[test]
    fn urls_pages_and_says_how_much_is_loaded() {
        let mut u = Urls::default();
        u.open();
        let rows: Vec<_> = (0..200)
            .map(|n| r(n, Some(404), RouteTarget::None, None))
            .collect();
        u.land(0, reply(0, rows, 450, 900));
        assert_eq!(u.summary(), "900 URLs, 200 loaded");
        let q = u.more();
        assert_eq!(q[0].page.offset, 200);
        assert!(u.more().is_empty(), "one page in flight at a time");
        // A repeat of page 0 is a leftover.
        u.land(0, reply(0, vec![], 450, 900));
        assert_eq!(u.len(), 200);
        u.land(
            0,
            Reply {
                offset: 200,
                result: Err("db locked".into()),
            },
        );
        assert_eq!(u.error(), Some("db locked"));
        // The store changing re-reads from the top and keeps the filter.
        u.set_filter(RouteFilter::NotFound);
        let q = u.invalidate();
        assert_eq!((q[0].filter, q[0].page.offset), (RouteFilter::NotFound, 0));
    }

    #[test]
    fn flags_follow_the_rules() {
        let doc = RouteTarget::Document(id(1));
        let mut ok = r(1, Some(200), doc.clone(), Some("/x"));
        assert_eq!(flag(&ok), None);
        // A target that is not there is unresolved even though the route has one.
        ok.new_url = None;
        ok.unresolved = true;
        assert_eq!(flag(&ok), Some(Flag::Unresolved));
        // A 404 with no target is the site's own dead link, not an unresolved route.
        assert_eq!(
            flag(&r(2, Some(404), RouteTarget::None, None)),
            Some(Flag::NotFound)
        );
        let mut c = r(3, Some(301), RouteTarget::Redirect(id(9)), Some("/y"));
        c.chain = true;
        assert_eq!(flag(&c), Some(Flag::Chain));
    }

    #[test]
    fn a_result_is_a_state_and_a_word_and_never_green() {
        for f in [
            None,
            Some(Flag::Unresolved),
            Some(Flag::Chain),
            Some(Flag::NotFound),
        ] {
            let (state, word) = Flag::chip(f);
            assert!(!word.is_empty());
            assert_ne!(
                state,
                ChipState::Clean,
                "a route table is a model, not a written result"
            );
        }
        assert_eq!(Flag::Unresolved.level(), Some(Severity::Error));
        assert_eq!(Flag::Chain.level(), Some(Severity::Warning));
        assert_eq!(Flag::NotFound.level(), None);
    }

    #[test]
    fn chips_show_what_each_filter_read_and_dots_until_then() {
        let mut u = Urls::default();
        u.open();
        assert_eq!(u.loading_text().as_deref(), Some("Building route table…"));
        u.land(
            0,
            reply(0, vec![r(1, Some(200), RouteTarget::None, None)], 1, 9),
        );
        assert_eq!(u.loading_text(), None);
        assert_eq!(u.count(RouteFilter::All), Some(9));
        assert_eq!(u.count(RouteFilter::Chains), None, "not read yet: no guess");
        u.set_filter(RouteFilter::Chains);
        let g = u.generation;
        u.land(g, reply(0, vec![], 0, 9));
        assert_eq!(u.count(RouteFilter::Chains), Some(0));
        assert_eq!(u.count(RouteFilter::All), Some(9));
    }

    #[test]
    fn an_empty_filter_says_what_it_found_and_offers_to_clear() {
        let mut u = Urls::default();
        u.open();
        u.land(
            0,
            reply(0, vec![r(1, Some(200), RouteTarget::None, None)], 1, 1612),
        );
        u.set_filter(RouteFilter::Chains);
        let g = u.generation;
        u.land(g, reply(0, vec![], 0, 1612));
        let e = u.empty().unwrap();
        assert_eq!(e.title, "No redirect chains in the route table");
        assert!(e.body.contains("All 1612 old URLs"), "{}", e.body);
        assert!(e.body.contains("live site hasn't been checked"));
        assert!(e.clear_filter);
    }

    #[test]
    fn no_rows_at_all_names_the_stages_that_fill_it() {
        let mut u = Urls::default();
        assert_eq!(u.empty(), None, "not asked yet is loading, not empty");
        u.open();
        assert_eq!(u.empty(), None, "loading is not empty");
        u.land(0, reply(0, vec![], 0, 0));
        let e = u.empty().unwrap();
        assert_eq!(e.title, "No URLs yet");
        assert!(!e.clear_filter);
    }
}
