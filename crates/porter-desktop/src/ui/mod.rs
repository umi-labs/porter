//! Shared components from the design system: state chips, severity, paths, buttons.
//! The pure rules (which glyph, word and look a state gets) are plain functions with
//! tests; the GPUI elements only paint them with the active theme tokens.

// The launcher uses a part of this; the overview, mapping and apply views (D2, D3) use the
// rest. Remove the allow once they have.
#![allow(dead_code)]

pub mod buttons;
pub mod chip;
pub mod path_label;
pub mod severity;
