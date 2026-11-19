use ratatui::layout::{Constraint, Layout, Rect};

pub const MIN_SIZE: (u16, u16) = (80, 24);
/// At this width and above the four panels and Detail sit side by side.
pub const WIDE: u16 = 100;

pub struct Areas {
    /// The four panels: Project, Stages, Issues, Items. None while Detail replaces them.
    pub panels: Option<[Rect; 4]>,
    pub detail: Option<Rect>,
    pub log: Option<Rect>,
    pub footer: Rect,
}

/// None when the terminal is too small to draw anything useful.
pub fn areas(area: Rect, show_log: bool, detail_open: bool) -> Option<Areas> {
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        return None;
    }
    let log_h = if !show_log {
        0
    } else if area.height >= 40 {
        8
    } else {
        5
    };
    let [main, log, footer] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(log_h),
        Constraint::Length(1),
    ])
    .areas(area);
    let panels = |r: Rect| {
        let p: [Rect; 4] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .areas(r);
        p
    };
    let (panels, detail) = if area.width >= WIDE {
        let [left, detail] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas(main);
        (Some(panels(left)), Some(detail))
    } else if detail_open {
        (None, Some(main))
    } else {
        (Some(panels(main)), None)
    };
    Some(Areas {
        panels,
        detail,
        log: (log_h > 0).then_some(log),
        footer,
    })
}
