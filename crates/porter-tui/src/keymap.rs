//! The one key table. `update` dispatches through it; the footer and the help overlay are
//! generated from it, so none of them can drift from what a key really does.
use crate::app::{App, DetailView, Modal, Panel};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Where a binding applies. The active scopes are listed innermost first (see `active`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Anywhere,
    Stages,
    /// The Issues and Items panels.
    Lists,
    /// An issue list or a single issue open in Detail.
    DetailIssue,
    /// Anything open in Detail.
    Detail,
    Filter,
    ConfirmQuit,
    ConfirmApply,
    Resolve,
    Help,
    Setup,
    SetupEdit,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Anywhere => "anywhere",
            Scope::Stages => "Stages",
            Scope::Lists => "Issues, Items",
            Scope::DetailIssue => "open issue",
            Scope::Detail => "Detail",
            Scope::Filter => "filter box",
            Scope::ConfirmQuit => "quit prompt",
            Scope::ConfirmApply => "apply prompt",
            Scope::Resolve => "resolve box",
            Scope::Help => "help",
            Scope::Setup => "setup",
            Scope::SetupEdit => "setup field",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    FocusNext,
    FocusPrev,
    FocusNth,
    Up,
    Down,
    PageUp,
    PageDown,
    Open,
    Back,
    RunStage,
    RunAll,
    LoadDry,
    LoadApply,
    Verify,
    Cancel,
    CtrlC,
    Filter,
    CycleFilter,
    Resolve,
    Jump,
    Report,
    Edit,
    Copy,
    ToggleLog,
    Help,
    Quit,
    ConfirmYes,
    ApplyYes,
    CloseModal,
    ResolveNext,
    ResolvePrev,
    ResolveConfirm,
    ResolveBackspace,
    FilterCommit,
    FilterCancel,
    FilterBackspace,
    CloseHelp,
    SetupNext,
    SetupPrev,
    SetupEnter,
    SetupCycleNext,
    SetupCyclePrev,
    SetupDone,
    SetupBackspace,
    SetupSave,
}

#[derive(Clone, Copy)]
pub struct Key(pub KeyCode, pub bool);

const fn ch(c: char) -> Key {
    Key(KeyCode::Char(c), false)
}
const fn kc(k: KeyCode) -> Key {
    Key(k, false)
}

pub struct Binding {
    pub scope: Scope,
    pub keys: &'static [Key],
    /// How help and the footer print the key.
    pub shown: &'static str,
    pub action: Action,
    /// Help text.
    pub what: &'static str,
    /// Footer text; empty keeps the binding out of the footer.
    pub short: &'static str,
}

const fn b(
    scope: Scope,
    keys: &'static [Key],
    shown: &'static str,
    action: Action,
    what: &'static str,
    short: &'static str,
) -> Binding {
    Binding {
        scope,
        keys,
        shown,
        action,
        what,
        short,
    }
}

use Action as A;
use Scope as S;

/// Within one scope the first match wins; scopes are tried innermost first.
pub const BINDINGS: &[Binding] = &[
    // Quit prompt (exclusive).
    b(
        S::ConfirmQuit,
        &[ch('y')],
        "y",
        A::ConfirmYes,
        "cancel the run and quit",
        "y quit",
    ),
    b(
        S::ConfirmQuit,
        &[ch('n'), kc(KeyCode::Esc)],
        "n/esc",
        A::CloseModal,
        "keep running",
        "n stay",
    ),
    // Apply prompt (exclusive). Any other key cancels, see `app::key`.
    b(
        S::ConfirmApply,
        &[ch('y')],
        "y",
        A::ApplyYes,
        "write to the live CMS",
        "y apply",
    ),
    b(
        S::ConfirmApply,
        &[ch('n'), kc(KeyCode::Esc)],
        "n/esc",
        A::CloseModal,
        "cancel (any other key too)",
        "n cancel",
    ),
    // Resolve modal (exclusive; typing edits the note).
    b(
        S::Resolve,
        &[kc(KeyCode::Down), kc(KeyCode::Tab)],
        "↓/tab",
        A::ResolveNext,
        "next choice",
        "↓ choose",
    ),
    b(
        S::Resolve,
        &[kc(KeyCode::Up), kc(KeyCode::BackTab)],
        "↑/S-tab",
        A::ResolvePrev,
        "previous choice",
        "",
    ),
    b(
        S::Resolve,
        &[kc(KeyCode::Enter)],
        "enter",
        A::ResolveConfirm,
        "confirm the resolution",
        "enter confirm",
    ),
    b(
        S::Resolve,
        &[kc(KeyCode::Backspace)],
        "backspace",
        A::ResolveBackspace,
        "delete a note character",
        "",
    ),
    b(
        S::Resolve,
        &[kc(KeyCode::Esc)],
        "esc",
        A::CloseModal,
        "cancel",
        "esc cancel",
    ),
    // Help overlay (exclusive).
    b(
        S::Help,
        &[ch('j'), kc(KeyCode::Down)],
        "j/↓",
        A::Down,
        "scroll down",
        "j/k scroll",
    ),
    b(
        S::Help,
        &[ch('k'), kc(KeyCode::Up)],
        "k/↑",
        A::Up,
        "scroll up",
        "",
    ),
    b(
        S::Help,
        &[ch('?'), kc(KeyCode::Esc)],
        "?/esc",
        A::CloseHelp,
        "close help",
        "esc close",
    ),
    b(
        S::Help,
        &[Key(KeyCode::Char('c'), true)],
        "ctrl-c",
        A::CtrlC,
        "close help, cancel the run; again to quit",
        "",
    ),
    // Filter box (exclusive; typing edits the text).
    b(
        S::Filter,
        &[kc(KeyCode::Enter)],
        "enter",
        A::FilterCommit,
        "apply the filter",
        "enter apply",
    ),
    b(
        S::Filter,
        &[kc(KeyCode::Esc)],
        "esc",
        A::FilterCancel,
        "clear the filter",
        "esc clear",
    ),
    b(
        S::Filter,
        &[kc(KeyCode::Backspace)],
        "backspace",
        A::FilterBackspace,
        "delete a character",
        "",
    ),
    // Detail.
    b(
        S::Detail,
        &[kc(KeyCode::Esc)],
        "esc",
        A::Back,
        "close Detail, go back",
        "esc back",
    ),
    b(
        S::DetailIssue,
        &[ch('j'), kc(KeyCode::Down)],
        "j/↓",
        A::Down,
        "move in the open issue list",
        "",
    ),
    b(
        S::DetailIssue,
        &[ch('k'), kc(KeyCode::Up)],
        "k/↑",
        A::Up,
        "move up in the open issue list",
        "",
    ),
    b(
        S::DetailIssue,
        &[kc(KeyCode::PageDown)],
        "pgdn",
        A::PageDown,
        "page down in the open list",
        "",
    ),
    b(
        S::DetailIssue,
        &[kc(KeyCode::PageUp)],
        "pgup",
        A::PageUp,
        "page up in the open list",
        "",
    ),
    b(
        S::DetailIssue,
        &[kc(KeyCode::Enter)],
        "enter",
        A::Open,
        "open the issue, or its item",
        "",
    ),
    b(
        S::DetailIssue,
        &[ch('x')],
        "x",
        A::Resolve,
        "resolve the issue",
        "x resolve",
    ),
    b(
        S::DetailIssue,
        &[ch('g')],
        "g",
        A::Jump,
        "jump to the issue's item",
        "g item",
    ),
    // Stages.
    b(
        S::Stages,
        &[ch('j'), kc(KeyCode::Down)],
        "j/↓",
        A::Down,
        "move down",
        "j/k move",
    ),
    b(
        S::Stages,
        &[ch('k'), kc(KeyCode::Up)],
        "k/↑",
        A::Up,
        "move up",
        "",
    ),
    b(
        S::Stages,
        &[ch(' ')],
        "space",
        A::RunStage,
        "run the selected stage",
        "space run",
    ),
    // Issues and Items.
    b(
        S::Lists,
        &[ch('j'), kc(KeyCode::Down)],
        "j/↓",
        A::Down,
        "move down",
        "j/k move",
    ),
    b(
        S::Lists,
        &[ch('k'), kc(KeyCode::Up)],
        "k/↑",
        A::Up,
        "move up",
        "",
    ),
    b(
        S::Lists,
        &[kc(KeyCode::PageDown)],
        "pgdn",
        A::PageDown,
        "move a page down",
        "",
    ),
    b(
        S::Lists,
        &[kc(KeyCode::PageUp)],
        "pgup",
        A::PageUp,
        "move a page up",
        "",
    ),
    b(
        S::Lists,
        &[kc(KeyCode::Enter)],
        "enter",
        A::Open,
        "open in Detail, drill in",
        "enter open",
    ),
    b(
        S::Lists,
        &[ch('/')],
        "/",
        A::Filter,
        "filter (fuzzy, incremental)",
        "/ filter",
    ),
    b(
        S::Lists,
        &[ch('f')],
        "f",
        A::CycleFilter,
        "cycle all, open, errors only",
        "f mode",
    ),
    b(
        S::Lists,
        &[kc(KeyCode::Esc)],
        "esc",
        A::Back,
        "clear the filter",
        "",
    ),
    b(
        S::Lists,
        &[ch('y')],
        "y",
        A::Copy,
        "copy the item or issue id",
        "",
    ),
    // Anywhere.
    b(
        S::Anywhere,
        &[kc(KeyCode::Tab)],
        "tab",
        A::FocusNext,
        "focus the next panel",
        "tab focus",
    ),
    b(
        S::Anywhere,
        &[kc(KeyCode::BackTab)],
        "S-tab",
        A::FocusPrev,
        "focus the previous panel",
        "",
    ),
    b(
        S::Anywhere,
        &[ch('1'), ch('2'), ch('3'), ch('4')],
        "1-4",
        A::FocusNth,
        "focus a panel by number",
        "",
    ),
    b(
        S::Anywhere,
        &[ch('R')],
        "R",
        A::RunAll,
        "run all stages",
        "R all",
    ),
    b(
        S::Anywhere,
        &[ch('l')],
        "l",
        A::LoadDry,
        "load, dry run",
        "",
    ),
    b(
        S::Anywhere,
        &[ch('L')],
        "L",
        A::LoadApply,
        "load, apply (asks first)",
        "",
    ),
    b(S::Anywhere, &[ch('v')], "v", A::Verify, "verify", ""),
    b(
        S::Anywhere,
        &[Key(KeyCode::Char('c'), true)],
        "ctrl-c",
        A::CtrlC,
        "cancel the run; again to quit",
        "",
    ),
    b(
        S::Anywhere,
        &[ch('c')],
        "c",
        A::Cancel,
        "cancel the running stage",
        "",
    ),
    b(
        S::Anywhere,
        &[ch('o')],
        "o",
        A::Report,
        "open the HTML report",
        "",
    ),
    b(
        S::Anywhere,
        &[ch('e')],
        "e",
        A::Edit,
        "edit porter.toml",
        "",
    ),
    b(
        S::Anywhere,
        &[ch('@')],
        "@",
        A::ToggleLog,
        "show or hide the log",
        "",
    ),
    b(S::Anywhere, &[ch('?')], "?", A::Help, "this help", "? help"),
    b(
        S::Anywhere,
        &[ch('q')],
        "q",
        A::Quit,
        "quit (asks first if a stage runs)",
        "q quit",
    ),
    // Setup form (no porter.toml yet). Typing goes into the field being edited.
    b(
        S::Setup,
        &[ch('j'), kc(KeyCode::Down), kc(KeyCode::Tab)],
        "j/↓/tab",
        A::SetupNext,
        "next field",
        "j/k move",
    ),
    b(
        S::Setup,
        &[ch('k'), kc(KeyCode::Up), kc(KeyCode::BackTab)],
        "k/↑/S-tab",
        A::SetupPrev,
        "previous field",
        "",
    ),
    b(
        S::Setup,
        &[kc(KeyCode::Enter)],
        "enter",
        A::SetupEnter,
        "type into the field, or pick the next choice",
        "enter edit",
    ),
    b(
        S::Setup,
        &[ch(' '), kc(KeyCode::Right)],
        "space/→",
        A::SetupCycleNext,
        "next choice",
        "space choose",
    ),
    b(
        S::Setup,
        &[kc(KeyCode::Left)],
        "←",
        A::SetupCyclePrev,
        "previous choice",
        "",
    ),
    b(
        S::Setup,
        &[Key(KeyCode::Char('s'), true)],
        "ctrl-s",
        A::SetupSave,
        "write porter.toml and open the project",
        "ctrl-s save",
    ),
    b(
        S::Setup,
        &[Key(KeyCode::Char('c'), true)],
        "ctrl-c",
        A::CtrlC,
        "quit",
        "",
    ),
    b(S::Setup, &[ch('q')], "q", A::Quit, "quit", "q quit"),
    b(
        S::SetupEdit,
        &[kc(KeyCode::Enter), kc(KeyCode::Esc), kc(KeyCode::Tab)],
        "enter/esc",
        A::SetupDone,
        "finish typing in the field",
        "enter done",
    ),
    b(
        S::SetupEdit,
        &[kc(KeyCode::Backspace)],
        "backspace",
        A::SetupBackspace,
        "delete a character",
        "",
    ),
    b(
        S::SetupEdit,
        &[Key(KeyCode::Char('s'), true)],
        "ctrl-s",
        A::SetupSave,
        "write porter.toml and open the project",
        "ctrl-s save",
    ),
    b(
        S::SetupEdit,
        &[Key(KeyCode::Char('c'), true)],
        "ctrl-c",
        A::CtrlC,
        "quit",
        "",
    ),
];

/// The scopes whose bindings apply right now, innermost first.
pub fn active(app: &App) -> Vec<Scope> {
    match &app.modal {
        Some(Modal::ConfirmQuit) => return vec![S::ConfirmQuit],
        Some(Modal::ConfirmApply(_)) => return vec![S::ConfirmApply],
        Some(Modal::Resolve(_)) => return vec![S::Resolve],
        None => {}
    }
    if let Some(st) = &app.setup {
        return vec![if st.editing { S::SetupEdit } else { S::Setup }];
    }
    if app.help {
        return vec![S::Help];
    }
    let filter = match app.focus {
        Panel::Items => Some(&app.panels.items.filter),
        Panel::Issues => Some(&app.panels.issues.filter),
        _ => None,
    };
    if filter.is_some_and(|f| f.editing) {
        return vec![S::Filter];
    }
    let mut v = vec![];
    match app.detail.last() {
        Some(DetailView::Issues(_) | DetailView::Issue(_)) => v.extend([S::DetailIssue, S::Detail]),
        Some(DetailView::Item(_) | DetailView::Notice(_)) => v.push(S::Detail),
        None => {}
    }
    match app.focus {
        Panel::Stages => v.push(S::Stages),
        Panel::Issues | Panel::Items => v.push(S::Lists),
        Panel::Project => {}
    }
    v.push(S::Anywhere);
    v
}

pub fn lookup(scopes: &[Scope], k: &KeyEvent) -> Option<Action> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    scopes.iter().find_map(|s| {
        BINDINGS
            .iter()
            .filter(|b| b.scope == *s)
            .find(|b| {
                b.keys
                    .iter()
                    .any(|Key(c, want)| *c == k.code && *want == ctrl)
            })
            .map(|b| b.action)
    })
}

/// The footer line for the current state.
pub fn footer(app: &App) -> String {
    let scopes = active(app);
    scopes
        .iter()
        .flat_map(|s| BINDINGS.iter().filter(move |b| b.scope == *s))
        .filter(|b| !b.short.is_empty())
        .map(|b| b.short)
        .collect::<Vec<_>>()
        .join(" · ")
}
