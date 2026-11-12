use crate::lists::{Filter, Mode};

/// Panel title carrying the active mode and `/` text, e.g. `Items 3/50000 [open] /abc_`.
pub fn title(base: &str, mode: Mode, f: &Filter) -> String {
    let mut t = base.to_string();
    if mode != Mode::All {
        t += &format!(" [{}]", mode.label());
    }
    if f.editing {
        t += &format!(" /{}_", f.text);
    } else if !f.text.is_empty() {
        t += &format!(" /{}", f.text);
    }
    t
}
