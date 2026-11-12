//! App start-up and window management.

use std::sync::Arc;

use gpui::{
    App, AppContext, Bounds, TitlebarOptions, Window, WindowBounds, WindowOptions, px, size,
};
use gpui_component::Root;
use porter_engine::session::Session;

use crate::launcher::Launcher;
use crate::project_window::ProjectWindow;

pub fn open_launcher(cx: &mut App) {
    open_window(cx, "Porter", 520., 560., |_, cx| {
        cx.new(|_| Launcher::new()).into()
    });
}

pub fn open_project_window(session: Arc<Session>, cx: &mut App) {
    let title = session.project().dir().display().to_string();
    open_window(cx, &title, 1200., 800., move |window, cx| {
        let session = session.clone();
        cx.new(|cx| ProjectWindow::new(session, window, cx)).into()
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
