use super::{footer, panel, tint, window};
use crate::app::App;
use crate::setup::{FIELDS, Kind, SECTIONS, Setup, env_is_set};
use porter_engine::config::is_env_var_name;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

const LABEL: usize = 14;

/// The single-screen setup form. Secrets are not inputs: each connector that reads one
/// shows the env var it reads and whether it is set.
pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let Some(s) = &app.setup else { return };
    let problems = s.failure.general.len().min(3) as u16;
    let [body, probs, foot] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(problems),
        Constraint::Length(1),
    ])
    .areas(area);
    let title = if s.saving {
        "Setup: writing porter.toml…"
    } else {
        "Setup: no porter.toml here (credentials are never typed here)"
    };
    let block = panel(title, true);
    let inner = block.inner(body);
    f.render_widget(block, body);
    let (lines, at) = lines(s);
    let rows = window(at, lines.len(), inner.height as usize);
    f.render_widget(Paragraph::new(lines[rows].to_vec()), inner);
    let red = tint(Some(Color::Red));
    let msgs: Vec<Line> = s
        .failure
        .general
        .iter()
        .take(3)
        .map(|m| Line::styled(m.clone(), red))
        .collect();
    f.render_widget(Paragraph::new(msgs), probs);
    footer::render(app, f, foot);
}

/// Every line of the form, and the line the focused field is on.
fn lines(s: &Setup) -> (Vec<Line<'static>>, usize) {
    let secrets = s.secrets(&env_is_set);
    let (dim, bold) = (
        Style::default().add_modifier(Modifier::DIM),
        Style::default().add_modifier(Modifier::BOLD),
    );
    let red = tint(Some(Color::Red));
    let mut out = vec![];
    let mut at = 0;
    for sec in SECTIONS {
        out.push(Line::styled(sec.title.to_string(), bold));
        for (i, fld) in FIELDS
            .iter()
            .enumerate()
            .filter(|(_, f)| f.section == sec.id)
        {
            if !s.shown(i) {
                continue;
            }
            let focused = s.focus == i;
            if focused {
                at = out.len();
            }
            let label = format!("{}{}", fld.label, if fld.required { "*" } else { "" });
            let mut spans = vec![Span::raw(format!(
                "{} {label:<LABEL$} ",
                if focused { "▸" } else { " " }
            ))];
            let masked = fld.id == "destination.api_key_env"
                && !s.values[i].trim().is_empty()
                && !is_env_var_name(s.values[i].trim());
            let v = &if masked {
                "(hidden: invalid name)".to_string()
            } else {
                s.values[i].clone()
            };
            match fld.kind {
                Kind::Choice(_) => {
                    let shown = if v.is_empty() { "(none)" } else { v.as_str() };
                    spans.push(Span::raw(format!("‹{shown}›")));
                }
                _ if focused && s.editing => spans.push(Span::raw(format!("{v}_"))),
                _ if v.is_empty() => spans.push(Span::styled(fld.hint.to_string(), dim)),
                _ => spans.push(Span::raw(v.clone())),
            }
            let line = Line::from(spans);
            out.push(if focused { line.style(bold) } else { line });
            for (_, m) in s.failure.fields.iter().filter(|(j, _)| *j == i) {
                out.push(Line::styled(format!("    ✗ {m}"), red));
            }
        }
        for sc in secrets.iter().filter(|x| x.section == sec.id) {
            let mark = if sc.set { "set ✓" } else { "not set ✗" };
            out.push(Line::styled(format!("    reads {} — {mark}", sc.var), dim));
        }
    }
    // Whatever was typed as the env var name might be a pasted key: say so, never print it.
    let name = s.get("destination.api_key_env").trim();
    if !name.is_empty() && !is_env_var_name(name) {
        let at_dest = out
            .iter()
            .rposition(|l| l.to_string().contains("env var NAME"));
        if let Some(n) = at_dest {
            out.insert(
                n + 1,
                Line::styled("    invalid env var name".to_string(), red),
            );
        }
    }
    (out, at)
}
