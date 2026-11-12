//! Pure state for the Issues and Items panels: bounded paging windows, filters.
use crate::msg::Cmd;
use porter_engine::session::{IssueQuery, ItemQuery, ItemRow, Page, Paged};
use porter_model::{Issue, IssueKind, ItemStatus, Resolution, Severity};

pub const ITEMS_PAGE: u64 = 500;
pub const ISSUES_PAGE: u64 = 200;

pub const ALL_KINDS: [IssueKind; 12] = [
    IssueKind::UnresolvedLink,
    IssueKind::MissingMedia,
    IssueKind::UnmappedField,
    IssueKind::UnsupportedBlock,
    IssueKind::UnresolvedRelation,
    IssueKind::ParentMissing,
    IssueKind::RedirectChain,
    IssueKind::RedirectLoop,
    IssueKind::WriteFailed,
    IssueKind::AccessGap,
    IssueKind::ConverterMissing,
    IssueKind::SchemaMismatch,
];

pub fn kind_name(k: IssueKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// `f` cycles these on both panels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    All,
    Open,
    Errors,
}

impl Mode {
    pub fn next(self) -> Mode {
        match self {
            Mode::All => Mode::Open,
            Mode::Open => Mode::Errors,
            Mode::Errors => Mode::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::All => "all",
            Mode::Open => "open",
            Mode::Errors => "errors",
        }
    }
}

/// Server-side issue filter for a mode (and optionally one kind).
pub fn issue_filter(mode: Mode, kind: Option<IssueKind>) -> IssueQuery {
    IssueQuery {
        kind,
        resolution: (mode == Mode::Open).then_some(Resolution::Open),
        severity: (mode == Mode::Errors).then_some(Severity::Error),
        ..Default::default()
    }
}

/// Case-insensitive subsequence match.
pub fn fuzzy(needle: &str, hay: &str) -> bool {
    let mut h = hay.chars().flat_map(char::to_lowercase);
    needle
        .chars()
        .flat_map(char::to_lowercase)
        .all(|n| h.any(|c| c == n))
}

#[derive(Default)]
pub struct Filter {
    pub text: String,
    pub editing: bool,
    /// The text the server was last asked for (items only).
    pub server: Option<String>,
}

/// One loaded page of a list that may be far larger than memory should hold.
/// Only the page the selection is in is kept; moving past it asks for the next page.
pub struct Window<T> {
    pub rows: Vec<T>,
    pub offset: u64,
    pub total: u64,
    pub limit: u64,
    /// The page offset currently requested; responses for anything else are dropped.
    pub want: Option<u64>,
    /// Bumped by every new query so stale answers can be told apart.
    pub seq: u64,
}

impl<T> Window<T> {
    pub fn new(limit: u64) -> Self {
        Window {
            rows: vec![],
            offset: 0,
            total: 0,
            limit,
            want: None,
            seq: 0,
        }
    }

    /// Starts a fresh query: forget the page and ask for the first one.
    pub fn reset(&mut self, seq: u64) {
        self.rows.clear();
        self.offset = 0;
        self.total = 0;
        self.seq = seq;
        self.want = Some(0);
    }

    /// The offset to re-query to refresh what is on screen.
    pub fn refresh(&mut self) -> u64 {
        let off = self.want.unwrap_or(self.offset);
        self.want = Some(off);
        off
    }

    /// Asks for the page holding `sel`, unless it is loaded or already requested.
    pub fn move_to(&mut self, sel: u64) -> Option<u64> {
        let page = sel / self.limit * self.limit;
        if self.want == Some(page) {
            return None;
        }
        // Back inside the loaded page: any pending request for another page is stale.
        if page == self.offset && !self.rows.is_empty() {
            self.want = None;
            return None;
        }
        self.want = Some(page);
        Some(page)
    }

    /// Moves `sel` by `d`, clamped to the list; returns a page to request.
    pub fn step(&mut self, sel: &mut u64, d: i64) -> Option<u64> {
        if self.total == 0 {
            return None;
        }
        let n = if d < 0 {
            sel.saturating_sub(d.unsigned_abs())
        } else {
            *sel + d.unsigned_abs()
        };
        *sel = n.min(self.total - 1);
        self.move_to(*sel)
    }

    /// Takes a page if it is the one asked for. Returns whether it was used.
    pub fn apply(&mut self, seq: u64, offset: u64, p: Paged<T>) -> bool {
        if seq != self.seq || self.want != Some(offset) {
            return false;
        }
        self.want = None;
        self.rows = p.rows;
        self.offset = offset;
        self.total = p.total;
        true
    }

    pub fn at(&self, sel: u64) -> Option<&T> {
        sel.checked_sub(self.offset)
            .and_then(|i| self.rows.get(usize::try_from(i).ok()?))
    }
}

pub struct ItemsState {
    pub win: Window<ItemRow>,
    /// Global index into the query's result (unfiltered view).
    pub sel: u64,
    /// Index into `shown()` while a client-side filter is active.
    pub fsel: usize,
    pub mode: Mode,
    pub filter: Filter,
}

impl Default for ItemsState {
    fn default() -> Self {
        ItemsState {
            win: Window::new(ITEMS_PAGE),
            sel: 0,
            fsel: 0,
            mode: Mode::All,
            filter: Filter::default(),
        }
    }
}

fn in_mode(r: &ItemRow, m: Mode) -> bool {
    match m {
        Mode::All => true,
        Mode::Open => r.statuses.iter().any(|(_, s)| *s != ItemStatus::Done),
        Mode::Errors => r
            .statuses
            .iter()
            .any(|(_, s)| matches!(s, ItemStatus::Failed { .. })),
    }
}

impl ItemsState {
    /// Client-side filtering is active: navigation stays inside the loaded page.
    pub fn filtered(&self) -> bool {
        !self.filter.text.is_empty() || self.mode != Mode::All
    }

    /// The loaded page narrowed by the fuzzy text and the mode.
    pub fn shown(&self) -> Vec<&ItemRow> {
        self.win
            .rows
            .iter()
            .filter(|r| in_mode(r, self.mode))
            .filter(|r| {
                self.filter.text.is_empty()
                    || fuzzy(
                        &self.filter.text,
                        &format!(
                            "{} {} {}",
                            r.kind.as_str(),
                            r.label,
                            r.title.as_deref().unwrap_or("")
                        ),
                    )
            })
            .collect()
    }

    pub fn selected(&self) -> Option<&ItemRow> {
        if self.filtered() {
            self.shown().get(self.fsel).copied()
        } else {
            self.win.at(self.sel)
        }
    }

    pub fn query(&self) -> ItemQuery {
        ItemQuery {
            kind: None,
            text: self.filter.server.clone(),
        }
    }

    pub fn cmd(&self, offset: u64) -> Cmd {
        Cmd::Items {
            q: self.query(),
            page: Page {
                offset,
                limit: ITEMS_PAGE,
            },
            seq: self.win.seq,
        }
    }

    pub fn step(&mut self, d: i64) -> Vec<Cmd> {
        if self.filtered() {
            let n = self.shown().len();
            self.fsel = if d < 0 {
                self.fsel.saturating_sub(d.unsigned_abs() as usize)
            } else {
                self.fsel + d as usize
            }
            .min(n.saturating_sub(1));
            return vec![];
        }
        let mut sel = self.sel;
        let off = self.win.step(&mut sel, d);
        self.sel = sel;
        off.map(|o| self.cmd(o)).into_iter().collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: IssueKind,
    pub total: u64,
    pub worst: Severity,
}

pub struct IssuesState {
    pub groups: Vec<Group>,
    pub sel: usize,
    pub mode: Mode,
    pub filter: Filter,
    pub seq: u64,
}

impl Default for IssuesState {
    fn default() -> Self {
        IssuesState {
            groups: vec![],
            sel: 0,
            mode: Mode::All,
            filter: Filter::default(),
            seq: 0,
        }
    }
}

impl IssuesState {
    pub fn shown(&self) -> Vec<&Group> {
        self.groups
            .iter()
            .filter(|g| self.filter.text.is_empty() || fuzzy(&self.filter.text, &kind_name(g.kind)))
            .collect()
    }

    pub fn selected(&self) -> Option<&Group> {
        self.shown().get(self.sel).copied()
    }
}

/// An issue list opened from a kind, paged 200 at a time.
pub struct IssueList {
    pub kind: IssueKind,
    pub mode: Mode,
    pub win: Window<Issue>,
    pub sel: u64,
}

impl IssueList {
    pub fn cmd(&self, offset: u64) -> Cmd {
        Cmd::Issues {
            q: issue_filter(self.mode, Some(self.kind)),
            page: Page {
                offset,
                limit: ISSUES_PAGE,
            },
            seq: self.win.seq,
        }
    }

    pub fn step(&mut self, d: i64) -> Vec<Cmd> {
        let mut sel = self.sel;
        let off = self.win.step(&mut sel, d);
        self.sel = sel;
        off.map(|o| self.cmd(o)).into_iter().collect()
    }
}
