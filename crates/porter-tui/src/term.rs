use crossterm::{
    cursor::Show,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use std::io::stdout;

/// Leaves the alternate screen, raw mode and restores the cursor. Safe to call twice.
pub fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen, Show);
}

/// Re-enters raw mode and the alternate screen after `restore`, e.g. when an editor exits.
pub fn resume() -> std::io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)
}

/// Restores the terminal when dropped, so every exit path (return, `?`, unwind) cleans up.
pub struct Guard;

impl Guard {
    pub fn enter() -> std::io::Result<Guard> {
        enable_raw_mode()?;
        let g = Guard; // from here Drop restores, even if the next line fails
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(g)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

/// Restores the terminal before the previous hook prints, so the message is readable.
/// Call it on the thread that draws the UI.
pub fn install_panic_hook() {
    install_panic_hook_with(restore);
}

/// Only a panic on the installing (UI) thread restores: a background thread's panic must
/// not tear the terminal down under a UI that keeps drawing. Every panic still reaches
/// the previous hook.
pub fn install_panic_hook_with(restore: impl Fn() + Send + Sync + 'static) {
    let ui = std::thread::current().id();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui {
            restore();
        }
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// One test: the hook is process-wide, so two tests swapping it would race.
    #[test]
    fn panic_restores_terminal_only_on_the_ui_thread() {
        let called = Arc::new(AtomicBool::new(false));
        let c = called.clone();
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // keep test output quiet
        install_panic_hook_with(move || c.store(true, Ordering::SeqCst));
        let _ = std::thread::spawn(|| panic!("background boom")).join();
        let background = called.load(Ordering::SeqCst);
        let _ = std::panic::catch_unwind(|| panic!("boom"));
        std::panic::set_hook(original);
        assert!(
            !background,
            "a background thread's panic restored the terminal"
        );
        assert!(called.load(Ordering::SeqCst));
    }
}
