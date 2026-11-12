use super::{centered, panel};
use crate::app::ResolveModal;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Clear, Paragraph, Wrap},
};

pub fn resolve(m: &ResolveModal, f: &mut Frame, area: Rect) {
    let r = centered(area, 64, 14);
    f.render_widget(Clear, r);
    let mark = |i: usize| if m.choice == i { "(•)" } else { "( )" };
    let note = if m.choice == 0 {
        format!("\n      note: {}_", m.note)
    } else {
        String::new()
    };
    let err = m
        .error
        .as_deref()
        .map(|e| format!("\n{e}"))
        .unwrap_or_default();
    let text = format!(
        "{}\n\n{} handled manually{note}\n{} won't fix\n{} reopen\n{err}\n\nup/down choose · enter confirm · esc cancel",
        m.title,
        mark(0),
        mark(1),
        mark(2),
    );
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel("Resolve issue", true)),
        r,
    );
}
