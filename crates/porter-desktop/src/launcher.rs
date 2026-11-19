//! The launcher window: recent projects and "Open folder…".

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    Context, FontWeight, IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    Styled, Window, div, px,
};
use gpui_component::{Root, WindowExt, h_flex, v_flex};
use porter_engine::session::Session;

use crate::recent::{self, RecentProject};
use crate::theme::{self, radius, text};
use crate::ui::buttons::{Kind, button};
use crate::ui::chip::{Chip, ChipState};
use crate::ui::path_label::PathLabel;

pub struct Launcher {
    /// Each recent project and whether its folder is gone.
    recent: Vec<(RecentProject, bool)>,
}

fn load_recent() -> Vec<(RecentProject, bool)> {
    recent::load()
        .into_iter()
        .map(|r| {
            let missing = !r.path.exists();
            (r, missing)
        })
        .collect()
}

impl Launcher {
    pub fn new() -> Self {
        Self {
            recent: load_recent(),
        }
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.open(path, window, cx));
        })
        .detach();
    }

    /// Opens the session off the UI thread (it touches SQLite), then the project window.
    fn open(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // The same project is never opened twice: a second window would be a second
        // writer on the same database.
        let path = path.canonicalize().unwrap_or(path);
        if crate::app::activate_project_window(&path, cx) {
            return;
        }
        let opening = cx.background_spawn({
            let path = path.clone();
            async move { Session::open(&path) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = opening.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(session) => {
                    recent::push(&path);
                    this.recent = load_recent();
                    cx.notify();
                    // A second click can finish opening while the first window is up.
                    if !crate::app::activate_project_window(&path, cx) {
                        crate::app::open_project_window(Arc::new(session), cx);
                    }
                }
                Err(e) => {
                    let msg = SharedString::from(format!("{e:#}"));
                    window.open_dialog(cx, move |dialog, _, _| {
                        dialog
                            .title("Could not open project")
                            .child(msg.clone())
                            .alert()
                    });
                }
            });
        })
        .detach();
    }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx);
        let count = self.recent.len();
        let rows = self.recent.iter().enumerate().map(|(i, (r, missing))| {
            let path = r.path.clone();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let initial = name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            // A plain row, not a Button: a Button has a fixed height that clips two lines.
            h_flex()
                .id(("recent", i))
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .when(i + 1 < count, |d| d.border_b_1().border_color(t.border))
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(28.))
                        .rounded(radius::SM)
                        .bg(t.panel)
                        .text_size(text::DENSE)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.muted)
                        .child(initial),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name),
                        )
                        .child(
                            PathLabel::new(("recent-path", i), path.display().to_string())
                                .missing(*missing),
                        ),
                )
                .when(*missing, |d| {
                    d.child(Chip::new(ChipState::Failed).label("Folder not found"))
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open(path.clone(), window, cx)),
                )
        });

        let sidebar = v_flex()
            .flex_none()
            .w(px(260.))
            .h_full()
            .p_6()
            .gap_5()
            .bg(t.surface)
            .border_r_1()
            .border_color(t.border)
            .child(
                h_flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(36.))
                            .rounded(px(9.))
                            .bg(t.text)
                            .text_color(t.bg)
                            .text_size(text::HEADING)
                            .font_weight(FontWeight::BOLD)
                            .child("P"),
                    )
                    .child(
                        v_flex()
                            .child(
                                div()
                                    .text_size(text::HEADING)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Porter"),
                            )
                            .child(
                                div()
                                    .text_size(text::MICRO)
                                    .text_color(t.muted)
                                    .child(env!("CARGO_PKG_VERSION")),
                            ),
                    ),
            )
            .child(
                button("open-folder", Kind::Primary, "Open folder…", cx)
                    .on_click(cx.listener(|this, _, window, cx| this.pick_folder(window, cx))),
            )
            .child(
                div()
                    .mt_auto()
                    .text_size(text::DENSE)
                    .text_color(t.muted)
                    .child("Each project is a folder with porter.toml and project.db."),
            );

        let list = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .px_6()
            .py_5()
            .gap_3()
            .child(
                div()
                    .text_size(text::HEADING)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Recent projects"),
            )
            .child(if self.recent.is_empty() {
                div()
                    .text_color(t.muted)
                    .child("No recent projects. Open a folder to start.")
                    .into_any_element()
            } else {
                v_flex()
                    .w_full()
                    .min_w_0()
                    .overflow_hidden()
                    .border_1()
                    .border_color(t.border)
                    .rounded(radius::MD)
                    .children(rows)
                    .into_any_element()
            })
            .child(
                div()
                    .text_size(text::MICRO)
                    .text_color(t.muted)
                    .child("Paths truncate in the middle; hover for the full path."),
            );

        h_flex()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .text_size(text::BODY)
            .child(sidebar)
            .child(list)
            .children(Root::render_dialog_layer(window, cx))
    }
}
