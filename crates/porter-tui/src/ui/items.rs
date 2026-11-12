use super::{filter, panel, tint, window};
use crate::app::{App, Panel};
use porter_engine::session::ItemRow;
use porter_model::ItemStatus;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub fn status_text(s: &ItemStatus) -> String {
    match s {
        ItemStatus::Done => "done".into(),
        ItemStatus::Partial { reason } => format!("partial: {reason}"),
        ItemStatus::Skipped { reason } => format!("skipped: {reason}"),
        ItemStatus::Failed { error } => format!("failed: {error}"),
    }
}

pub fn status_glyph(s: &ItemStatus) -> (&'static str, Option<Color>) {
    match s {
        ItemStatus::Done => ("✓", Some(Color::Green)),
        ItemStatus::Partial { .. } => ("⚠", Some(Color::Yellow)),
        ItemStatus::Skipped { .. } => ("-", None),
        ItemStatus::Failed { .. } => ("✗", Some(Color::Red)),
    }
}

/// The glyph of the worst status across stages (failed > partial > skipped > done).
fn row_glyph(r: &ItemRow) -> (&'static str, Option<Color>) {
    let rank = |s: &ItemStatus| match s {
        ItemStatus::Failed { .. } => 3,
        ItemStatus::Partial { .. } => 2,
        ItemStatus::Skipped { .. } => 1,
        ItemStatus::Done => 0,
    };
    r.statuses
        .iter()
        .max_by_key(|(_, s)| rank(s))
        .map_or(("·", None), |(_, s)| status_glyph(s))
}

pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let st = &app.panels.items;
    let focused = app.focus == Panel::Items;
    let shown = st.shown();
    let (pos, local) = if st.filtered() {
        (st.fsel, st.fsel)
    } else {
        (
            usize::try_from(st.sel).unwrap_or(usize::MAX),
            usize::try_from(st.sel.saturating_sub(st.win.offset)).unwrap_or(0),
        )
    };
    let base = if st.filtered() {
        format!(
            "Items {}/{} on this page",
            shown.len().min(pos + 1),
            shown.len()
        )
    } else if st.win.total == 0 {
        "Items".to_string()
    } else {
        format!("Items {}/{}", pos + 1, st.win.total)
    };
    let title = filter::title(&base, st.mode, &st.filter);
    let block = panel(&title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if shown.is_empty() {
        f.render_widget(
            Paragraph::new("no items").style(Style::default().add_modifier(Modifier::DIM)),
            inner,
        );
        return;
    }
    let range = window(local, shown.len(), inner.height as usize);
    let start = range.start;
    let lines: Vec<Line> = shown[range]
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let sel = focused && i + start == local;
            let (g, c) = row_glyph(r);
            let mut line = Line::from(vec![
                Span::raw(if sel { "> " } else { "  " }),
                Span::styled(g, tint(c)),
                Span::raw(format!(
                    " {:<8} {}  {}",
                    r.kind.as_str(),
                    r.label,
                    r.title.as_deref().unwrap_or("")
                )),
            ]);
            if sel {
                line = line.style(super::accent());
            }
            line
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
