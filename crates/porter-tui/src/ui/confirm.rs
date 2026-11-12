use super::{centered, panel};
use crate::app::ApplyModal;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Clear, Paragraph, Wrap},
};

/// The apply prompt: names the destination and what it would write before anything runs.
pub fn apply(m: &ApplyModal, f: &mut Frame, area: Rect) {
    let r = centered(area, 64, 16);
    f.render_widget(Clear, r);
    let url = if m.url.is_empty() {
        "(no destination.url set)"
    } else {
        &m.url
    };
    let body = match &m.counts {
        Some(c) => {
            let total: u64 = c.values().sum();
            let rows: String = c.iter().map(|(k, n)| format!("  {k}: {n}\n")).collect();
            format!("Items to write ({total}), from the last dry run:\n{rows}\ny writes · any other key cancels")
        }
        None => "no dry run yet — press l first\n\nApply is refused until a dry run has shown what it would write.\n\ny refuses · any other key cancels".into(),
    };
    let text = format!("This writes to a live CMS.\n\nDestination: {url}\n\n{body}");
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(panel("Apply load", true)),
        r,
    );
}
