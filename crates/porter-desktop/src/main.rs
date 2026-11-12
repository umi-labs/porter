#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod app;
mod bridge;
mod launcher;
mod project_window;
mod recent;
mod sidebar;
mod theme;
mod thumbs;
mod ui;
mod views;
mod vm;

fn main() {
    gpui::Application::new().run(|cx| {
        gpui_component::init(cx);
        theme::sync(None, cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        app::open_launcher(cx);
        cx.activate(true);
    });
}
