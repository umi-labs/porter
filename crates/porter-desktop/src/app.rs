//! App start-up and window management.

use std::sync::Arc;

use gpui::{
    App, AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled, TitlebarOptions,
    Window, WindowBounds, WindowOptions, div, px, size,
};
use gpui_component::{ActiveTheme, Root, v_flex};
use porter_engine::session::Session;

use crate::launcher::Launcher;

pub fn open_launcher(cx: &mut App) {
    open_window(cx, "Porter", 520., 560., |_, cx| {
        cx.new(|_| Launcher::new()).into()
    });
}

/// The project window. A placeholder until the full project window lands (task 2).
pub fn open_project_window(session: Arc<Session>, cx: &mut App) {
    let title = session.project().dir().display().to_string();
    open_window(cx, &title, 1200., 800., move |_, cx| {
        let session = session.clone();
        cx.new(|_| ProjectWindow { session }).into()
    });
}

fn open_window(
    cx: &mut App,
    title: &str,
    w: f32,
    h: f32,
    view: impl FnOnce(&mut Window, &mut App) -> gpui::AnyView + 'static,
) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(w), px(h)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_string().into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |window, cx| {
        let view = view(window, cx);
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        eprintln!("porter-desktop: could not open window: {e:#}");
    }
}

struct ProjectWindow {
    session: Arc<Session>,
}

impl Render for ProjectWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .p_6()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .text_xl()
                    .child(self.session.project().dir().display().to_string()),
            )
            .children(Root::render_dialog_layer(window, cx))
    }
}
