use super::{centered, panel};
use crate::app::ApplyModal;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Clear, Paragraph, Wrap},
};

const WIDTH: u16 = 64;

/// The apply prompt: names the destination and what it would write before anything runs.
/// Sized to its content within the screen; collections that do not fit are summarised so
/// the last line (what `y` does) is always visible.
pub fn apply(m: &ApplyModal, f: &mut Frame, area: Rect) {
    let inner_w = usize::from(WIDTH.min(area.width).saturating_sub(2)).max(1);
    let max = usize::from(area.height.saturating_sub(2));
    let url = if m.url.is_empty() {
        "(no destination.url set)"
    } else {
        &m.url
    };
    let mut head = vec![
        "This writes to a live CMS.".to_string(),
        String::new(),
        format!("Destination: {url}"),
        String::new(),
    ];
    let mut rows = vec![];
    let tail: Vec<String> = match &m.preview {
        Some(p) => {
            let when = format!(
                "dry run from {}",
                crate::actions::age(p.at, std::time::SystemTime::now())
            );
            match &p.stale {
                Some(why) => {
                    head.push(format!("{when}; {why}."));
                    vec![
                        String::new(),
                        "dry run is stale — press l to refresh".into(),
                        String::new(),
                        "y refuses · any other key cancels".into(),
                    ]
                }
                None => {
                    let total: u64 = p.counts.values().sum();
                    head.push(when);
                    head.push(format!("Items the load would send ({total}):"));
                    rows = p
                        .counts
                        .iter()
                        .map(|(k, n)| format!("  {k}: {n}"))
                        .collect();
                    vec!["y writes · any other key cancels".into()]
                }
            }
        }
        None => vec![
            "no dry run yet — press l first".into(),
            String::new(),
            "Apply is refused until a dry run has shown what it would write.".into(),
            String::new(),
            "y refuses · any other key cancels".into(),
        ],
    };
    let height = |ls: &[String]| -> usize {
        ls.iter()
            .map(|l| l.chars().count().div_ceil(inner_w).max(1))
            .sum()
    };
    let fixed = height(&head) + height(&tail);
    if fixed + height(&rows) > max && !rows.is_empty() {
        // Room for some rows plus the "+N more" line.
        let keep = max.saturating_sub(fixed + 1).min(rows.len() - 1);
        let more = rows.len() - keep;
        rows.truncate(keep);
        rows.push(format!("  +{more} more collections"));
    }
    let lines: Vec<String> = head.into_iter().chain(rows).chain(tail).collect();
    let total = height(&lines);
    let r = centered(area, WIDTH, u16::try_from(total + 2).unwrap_or(u16::MAX));
    f.render_widget(Clear, r);
    // Still too tall (a tiny terminal): show the bottom, where the `y` line is.
    let scroll = u16::try_from(total.saturating_sub(max)).unwrap_or(0);
    f.render_widget(
        Paragraph::new(lines.join("\n"))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(panel("Apply load", true)),
        r,
    );
}
