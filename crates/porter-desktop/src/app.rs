//! App start-up and window management.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Global, TitlebarOptions, Window, WindowBounds,
    WindowOptions, px, size,
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

/// Open project windows by canonical project path, so a project opens in one window only.
/// Generic over the handle so the logic is testable without a display.
pub struct OpenWindows<H> {
    by_path: HashMap<PathBuf, H>,
}

impl<H> Default for OpenWindows<H> {
    fn default() -> Self {
        Self {
            by_path: HashMap::new(),
        }
    }
}

impl<H: Clone> OpenWindows<H> {
    /// The window already showing `path`, if it is still open (`is_open`); a closed
    /// window's entry is dropped.
    pub fn existing(&mut self, path: &Path, is_open: impl Fn(&H) -> bool) -> Option<H> {
        let h = self.by_path.get(path)?;
        if is_open(h) {
            return Some(h.clone());
        }
        self.by_path.remove(path);
        None
    }

    pub fn insert(&mut self, path: PathBuf, handle: H) {
        self.by_path.insert(path, handle);
    }
}

impl Global for OpenWindows<AnyWindowHandle> {}

/// Brings the window already showing `path` to the front. False when there is none (or it
/// has just closed), and the caller should open one.
pub fn activate_project_window(path: &Path, cx: &mut App) -> bool {
    let open = cx.windows();
    let found = cx
        .default_global::<OpenWindows<AnyWindowHandle>>()
        .existing(path, |h| open.contains(h));
    let Some(handle) = found else {
        return false;
    };
    handle
        .update(cx, |_, window, _| window.activate_window())
        .is_ok()
}

pub fn open_project_window(session: Arc<Session>, cx: &mut App) {
    let dir = session.project().dir().to_path_buf();
    let title = dir.display().to_string();
    let handle = open_window(cx, &title, 1200., 800., move |window, cx| {
        let session = session.clone();
        cx.new(|cx| ProjectWindow::new(session, window, cx)).into()
    });
    if let Some(h) = handle {
        cx.default_global::<OpenWindows<AnyWindowHandle>>()
            .insert(dir.canonicalize().unwrap_or(dir), h);
    }
}

fn open_window(
    cx: &mut App,
    title: &str,
    w: f32,
    h: f32,
    view: impl FnOnce(&mut Window, &mut App) -> gpui::AnyView + 'static,
) -> Option<AnyWindowHandle> {
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
    match cx.open_window(options, |window, cx| {
        let view = view(window, cx);
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(h) => Some(h.into()),
        Err(e) => {
            eprintln!("porter-desktop: could not open window: {e:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_has_one_window_until_it_closes() {
        let mut w: OpenWindows<u32> = OpenWindows::default();
        let (a, b) = (Path::new("/p/a"), Path::new("/p/b"));
        assert_eq!(w.existing(a, |_| true), None);
        w.insert(a.into(), 1);
        w.insert(b.into(), 2);
        assert_eq!(w.existing(a, |_| true), Some(1));
        assert_eq!(w.existing(b, |_| true), Some(2));
        // Window 1 closes: its entry goes, the other stays.
        assert_eq!(w.existing(a, |h| *h != 1), None);
        assert_eq!(w.existing(a, |_| true), None, "the stale entry was dropped");
        assert_eq!(w.existing(b, |h| *h != 1), Some(2));
        // Reopening registers the new window.
        w.insert(a.into(), 3);
        assert_eq!(w.existing(a, |_| true), Some(3));
    }
}
