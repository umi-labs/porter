use super::{issues::sev_glyph, items, panel, stages, window};
use crate::app::{App, DetailView, Panel};
use crate::lists::kind_name;
use porter_engine::session::{ItemDetail, StageState};
use porter_model::{Issue, Resolution};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Paragraph, Wrap},
};

const JSON_LINES: usize = 40;

/// Detail for the focused list's selection or the view entered with Enter; engine errors
/// are shown verbatim.
pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let block = panel("Detail", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(text(app, inner.height as usize)).wrap(Wrap { trim: false }),
        inner,
    );
}

fn text(app: &App, h: usize) -> String {
    match app.detail.last() {
        Some(DetailView::Issues(l)) => issue_list_text(l, h),
        Some(DetailView::Issue(i)) => issue_text(i, h),
        Some(DetailView::Item(d)) => item_text(d, h),
        Some(DetailView::Notice(t)) => format!("porter.toml\n\n{t}"),
        None => match app.focus {
            Panel::Stages => stage_text(app),
            Panel::Project => project_text(app),
            Panel::Issues => match app.panels.issues.selected() {
                Some(g) => format!(
                    "{}\n\n{} issues, worst: {:?}\n\nenter lists them",
                    kind_name(g.kind),
                    g.total,
                    g.worst
                ),
                None => String::new(),
            },
            Panel::Items => match app.panels.items.selected() {
                Some(r) => format!(
                    "{}\n{} {}\n\nenter shows the item",
                    r.title.as_deref().unwrap_or("(untitled)"),
                    r.kind.as_str(),
                    r.label
                ),
                None => String::new(),
            },
        },
    }
}

pub fn resolution_text(r: &Resolution) -> String {
    match r {
        Resolution::Open => "open".into(),
        Resolution::FixedByRerun => "fixed by rerun".into(),
        Resolution::HandledManually { note } => format!("handled manually: {note}"),
        Resolution::WontFix => "won't fix".into(),
    }
}

/// Up to ~40 JSON lines, fewer when the pane is short, so the truncation note stays visible.
fn budget(h: usize, used: usize) -> usize {
    h.saturating_sub(used + 1).clamp(3, JSON_LINES)
}

/// The first `n` lines of the pretty-printed JSON, then how many were left out.
pub fn json_slice(v: &serde_json::Value, n: usize) -> String {
    let pretty = serde_json::to_string_pretty(v).unwrap_or_default();
    let lines: Vec<&str> = pretty.lines().collect();
    let mut out = lines[..lines.len().min(n)].join("\n");
    if lines.len() > n {
        out += &format!("\n… {} more lines", lines.len() - n);
    }
    out
}

fn issue_list_text(l: &crate::lists::IssueList, h: usize) -> String {
    let mut out = format!(
        "{} · {} · {}/{}\n",
        kind_name(l.kind),
        l.mode.label(),
        if l.win.total == 0 { 0 } else { l.sel + 1 },
        l.win.total
    );
    if l.win.rows.is_empty() {
        return out + "\nloading or none\n";
    }
    let local = usize::try_from(l.sel.saturating_sub(l.win.offset)).unwrap_or(0);
    let range = window(local, l.win.rows.len(), h.saturating_sub(1));
    let start = range.start;
    for (i, is) in l.win.rows[range].iter().enumerate() {
        let (g, _) = sev_glyph(is.severity);
        out += &format!(
            "{} {g} {} [{}] {:?}\n",
            if i + start == local { ">" } else { " " },
            is.message,
            resolution_text(&is.resolution),
            is.stage
        );
    }
    out
}

fn issue_text(i: &Issue, h: usize) -> String {
    let (g, _) = sev_glyph(i.severity);
    let mut out = format!(
        "{g} {} ({:?}) · {:?}\n{}\n\nresolution: {}\n",
        kind_name(i.kind),
        i.severity,
        i.stage,
        i.message,
        resolution_text(&i.resolution)
    );
    match i.item {
        Some(id) => out += &format!("item: {id}  (g jumps to it)\n"),
        None => out += "item: none\n",
    }
    out += "\ndetails:\n";
    let used = out.lines().count();
    out + &json_slice(&i.details, budget(h, used))
}

fn item_text(d: &ItemDetail, h: usize) -> String {
    let get = |k: &str| d.data.get(k).and_then(|v| v.as_str());
    let mut out = format!("{}\n", d.row.title.as_deref().unwrap_or("(untitled)"));
    out += &format!(
        "type: {}{}\n",
        d.row.kind.as_str(),
        get("type").map(|t| format!(" ({t})")).unwrap_or_default()
    );
    out += &format!(
        "path: {}\n",
        get("path")
            .or(d.source.url.as_deref())
            .unwrap_or(&d.source.source_id)
    );
    out += &format!("parent: {}\n", get("parent").unwrap_or("none"));
    out += &format!(
        "source: {} {}\n\nstatus:\n",
        d.source.connector, d.source.source_id
    );
    if d.row.statuses.is_empty() {
        out += "  not processed yet\n";
    }
    for (stage, s) in &d.row.statuses {
        let (g, _) = items::status_glyph(s);
        out += &format!(
            "  {g} {:<10} {}\n",
            stages::name(*stage),
            items::status_text(s)
        );
    }
    out += &format!("\nissues ({}):\n", d.issues.len());
    for i in &d.issues {
        let (g, _) = sev_glyph(i.severity);
        out += &format!(
            "  {g} {} {} [{}]\n",
            kind_name(i.kind),
            i.message,
            resolution_text(&i.resolution)
        );
    }
    out += "\nnormalised JSON:\n";
    let used = out.lines().count();
    out + &json_slice(&d.data, budget(h, used))
}

fn stage_text(app: &App) -> String {
    let sel = app.panels.stages.selected();
    let Some(r) = stages::rows(app).into_iter().find(|r| r.stage == sel) else {
        return String::new();
    };
    let (g, _) = stages::glyph(&r.state);
    let mut out = format!("{} {}\n", g, stages::name(sel));
    match &r.state {
        StageState::CouldNotRun(e) => out += &format!("\ncould not run:\n{e}\n"),
        StageState::Running => {
            let l = &app.panels.stages.live;
            if let Some(p) = &r.progress {
                out += &format!("\n{}\n", stages::progress_text(p));
            }
            out += &format!(
                "\nthis run: {} items ({} failed), {} errors, {} warnings\n",
                l.items, l.failed, l.errors, l.warnings
            );
            if app.cancelling {
                out += "\ncancelling…\n";
            }
        }
        StageState::Never => out += "\nnot run yet. space runs it.\n",
        StageState::Skipped => out += "\nskipped: the last load was a dry run.\n",
        _ => {}
    }
    if let Some(why) = &r.stale {
        out += &format!("\nstale: {why}. The result below is from before that.\n");
    }
    if let Some(run) = &r.last_run {
        out += &format!(
            "\nlast run (porter {}):\n  started {}\n",
            run.porter_version, run.started_at
        );
        match (&run.ended_at, &run.summary) {
            (Some(e), _) => out += &format!("  ended   {e}\n"),
            (None, _) => out += "  never ended (interrupted before it could record a summary)\n",
        }
        if run.dry_run {
            out += "  dry run: nothing was written to the destination\n";
        }
        if let Some(s) = &run.summary {
            out += &format!(
                "  done {} · partial {} · skipped {} · failed {}\n  issues: {} errors, {} warnings, {} info{}\n",
                s.done,
                s.partial,
                s.skipped,
                s.failed,
                s.errors,
                s.warnings,
                s.infos,
                if s.interrupted {
                    "\n  interrupted: resumable"
                } else {
                    ""
                }
            );
        }
    }
    out
}

fn project_text(app: &App) -> String {
    let Some(s) = &app.session else {
        return String::new();
    };
    let c = s.config();
    format!(
        "{}\nsource: {}\ndestination: {} ({})\n\nR runs analyse → a dry-run load (no verify);\nL applies the load, then v verifies.",
        c.project.name, c.source.adapter, c.destination.adapter, c.destination.url
    )
}
