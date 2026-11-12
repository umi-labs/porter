use super::{centered, panel};
use crate::app::ApplyModal;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Clear, Paragraph, Wrap},
};

/// The apply prompt: names the destination and what it would write before anything runs.
pub fn apply(m: &ApplyModal, f: &mut Frame, area: Rect) {
    let r = centered(area, 64, 17);
    f.render_widget(Clear, r);
    let url = if m.url.is_empty() {
        "(no destination.url set)"
    } else {
        &m.url
    };
    let body = match &m.preview {
        Some(p) => {
            let total: u64 = p.counts.values().sum();
            let rows: String = p.counts.iter().map(|(k, n)| format!("  {k}: {n}\n")).collect();
            let when = format!(
                "dry run from {}",
                crate::actions::age(p.at, std::time::SystemTime::now())
            );
            match &p.stale {
                Some(why) => format!(
                    "{when}; {why}.\n\ndry run is stale — press l to refresh\n\ny refuses · any other key cancels"
                ),
                None => format!(
                    "{when}\nItems the load would send ({total}):\n{rows}\ny writes · any other key cancels"
                ),
            }
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
