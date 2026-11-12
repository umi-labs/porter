//! Shared components from the design system: state chips, severity, paths, buttons.
//! The pure rules (which glyph, word and look a state gets) are plain functions with
//! tests; the GPUI elements only paint them with the active theme tokens.

pub mod buttons;
pub mod chip;
pub mod path_label;
pub mod severity;
