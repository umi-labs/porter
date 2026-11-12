//! The launcher window: recent projects and "Open folder…".

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    AppContext, Context, IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    Styled, Window, div,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Root, WindowExt, h_flex, v_flex};
use porter_engine::session::Session;

use crate::recent::{self, RecentProject};

pub struct Launcher {
    recent: Vec<RecentProject>,
}

impl Launcher {
    pub fn new() -> Self {
        Self {
            recent: recent::load(),
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
        let opening = cx.background_spawn({
            let path = path.clone();
            async move { Session::open(&path) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = opening.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(session) => {
                    recent::push(&path);
                    this.recent = recent::load();
                    cx.notify();
                    crate::app::open_project_window(Arc::new(session), cx);
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
        let theme = cx.theme();
        let rows = self.recent.iter().enumerate().map(|(i, r)| {
            let path = r.path.clone();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Button::new(("recent", i))
                .ghost()
                .w_full()
                .child(
                    v_flex().items_start().child(name).child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(path.display().to_string()),
                    ),
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open(path.clone(), window, cx)),
                )
        });

        v_flex()
            .size_full()
            .p_6()
            .gap_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_xl().child("Porter"))
                    .child(
                        Button::new("open-folder")
                            .primary()
                            .label("Open folder…")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
                            ),
                    ),
            )
            .child(div().text_sm().text_color(theme.muted_foreground).child(
                if self.recent.is_empty() {
                    "No recent projects"
                } else {
                    "Recent projects"
                },
            ))
            .child(v_flex().gap_1().children(rows))
            .children(Root::render_dialog_layer(window, cx))
    }
}
