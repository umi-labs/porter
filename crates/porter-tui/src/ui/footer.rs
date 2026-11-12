use crate::app::App;
use crate::keymap;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
};

/// The keys that apply to the focused panel, generated from the keymap.
pub fn render(app: &App, f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new(keymap::footer(app)).style(Style::default().add_modifier(Modifier::DIM)),
        area,
    );
}
