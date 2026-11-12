use super::{panel, stages};
use crate::app::{App, Panel};
use porter_engine::session::StageState;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Paragraph, Wrap},
};

/// Detail for the focused list's selection; engine errors are shown verbatim.
pub fn render(app: &App, f: &mut Frame, area: Rect) {
    let block = panel("Detail", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(text(app)).wrap(Wrap { trim: false }), inner);
}

fn text(app: &App) -> String {
    match app.focus {
        Panel::Stages => stage_text(app),
        Panel::Project => project_text(app),
        _ => String::new(),
    }
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
        _ => {}
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
        "{}\nsource: {}\ndestination: {} ({})\n\nR runs every stage, v verifies.",
        c.project.name, c.source.adapter, c.destination.adapter, c.destination.url
    )
}
