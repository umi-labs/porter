//! The state chip: glyph plus word, never colour alone. One look per state in the
//! design's table (docs/design/README.md).

use gpui::{
    App, FontWeight, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
    div, pattern_slash, px,
};
use porter_engine::session::StageState;

use crate::theme::{Tokens, radius, tokens};

/// Every state the design can show, from the engine's `StageState` (see
/// `From<&StageState>`). Stale is a modifier on a previous result (`Look::stale`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipState {
    NotRun,
    Running,
    Clean,
    Warnings,
    Errors,
    /// Could not run (exit 1).
    Failed,
    Interrupted,
    /// An upstream stage or the mapping changed since. Prefer `Look::stale` on the
    /// previous result; this is the bare chip for when there is none.
    #[allow(dead_code)]
    Stale,
    DryRun,
    Skipped,
}

impl From<&StageState> for ChipState {
    fn from(s: &StageState) -> Self {
        match s {
            StageState::Never => Self::NotRun,
            StageState::Running => Self::Running,
            StageState::Clean => Self::Clean,
            StageState::Warnings => Self::Warnings,
            StageState::Errors => Self::Errors,
            StageState::Interrupted => Self::Interrupted,
            StageState::CouldNotRun(_) => Self::Failed,
            StageState::DryRun => Self::DryRun,
            StageState::Skipped => Self::Skipped,
        }
    }
}

/// Which colours a chip uses; resolved against the tokens at paint time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Accent,
    /// Written and clean. The only green.
    Ok,
    Warn,
    Err,
    /// Solid red, white text: could not run.
    ErrSolid,
    Int,
    Stale,
    Dry,
    Faint,
}

/// The glyph's shape, so states survive greyscale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Ring,
    /// Dry run.
    DashedRing,
    /// Running. GPUI has no dotted border, so this paints dashed.
    DottedRing,
    Disc,
    Square,
    /// No border: just the mark (stale ↻).
    Bare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// The mark inside the shape; empty for ring-only states.
    pub glyph: &'static str,
    pub word: &'static str,
    pub tone: Tone,
    pub shape: Shape,
    /// Painted with the dry-run slash hatch.
    pub hatched: bool,
    /// Dashed chip border.
    pub dashed: bool,
    /// Drawn at 62%: a previous result that no longer holds.
    pub dimmed: bool,
}

impl Look {
    /// Stale is a modifier on the previous result, not a replacement: the same chip at 62%.
    pub fn stale(self) -> Self {
        Self {
            dimmed: true,
            ..self
        }
    }
}

pub const STALE_OPACITY: f32 = 0.62;

pub fn stage_look(state: ChipState) -> Look {
    let l = |glyph, word, tone, shape| Look {
        glyph,
        word,
        tone,
        shape,
        hatched: false,
        dashed: false,
        dimmed: false,
    };
    match state {
        ChipState::NotRun => Look {
            dashed: true,
            ..l("", "Not run", Tone::Muted, Shape::Ring)
        },
        ChipState::Running => l("", "Running", Tone::Accent, Shape::DottedRing),
        ChipState::Clean => l("✓", "Clean", Tone::Ok, Shape::Disc),
        ChipState::Warnings => l("!", "Warnings", Tone::Warn, Shape::Square),
        ChipState::Errors => l("!", "Errors", Tone::Err, Shape::Disc),
        ChipState::Failed => l("✕", "Failed", Tone::ErrSolid, Shape::Disc),
        ChipState::Interrupted => l("‖", "Interrupted", Tone::Int, Shape::Ring),
        ChipState::Stale => l("↻", "Stale", Tone::Stale, Shape::Bare),
        ChipState::DryRun => Look {
            hatched: true,
            dashed: true,
            ..l("", "Dry run", Tone::Dry, Shape::DashedRing)
        },
        ChipState::Skipped => l("–", "Skipped", Tone::Faint, Shape::Ring),
    }
}

struct Colors {
    fg: Hsla,
    bg: Hsla,
    /// The chip border; none for the filled tones.
    border: Option<Hsla>,
}

fn colors(t: &Tokens, tone: Tone) -> Colors {
    let none = gpui::transparent_black();
    let (fg, bg, border) = match tone {
        Tone::Muted => (t.muted, none, Some(t.border_strong)),
        Tone::Accent => (t.accent, t.accent_bg, None),
        Tone::Ok => (t.ok, t.ok_bg, None),
        Tone::Warn => (t.warn, t.warn_bg, None),
        Tone::Err => (t.err, t.err_bg, Some(t.err)),
        Tone::ErrSolid => (gpui::white(), t.err_solid, None),
        Tone::Int => (t.int, t.int_bg, None),
        Tone::Stale => (t.muted, t.stale_bg, Some(t.border_strong)),
        Tone::Dry => (t.dry, t.dry_bg, Some(t.dry_border)),
        Tone::Faint => (t.faint, none, Some(t.border)),
    };
    Colors { fg, bg, border }
}

/// The glyph: `size` px, ring or filled, with its mark. `ink` is the mark on a filled disc.
pub fn glyph(look: Look, size: f32, t: &Tokens) -> impl IntoElement {
    let c = colors(t, look.tone);
    let el = div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size))
        .text_size(px(size * 0.75))
        .line_height(px(size))
        .font_weight(FontWeight::EXTRA_BOLD);
    // Disc fills with the tone colour and the mark takes the "on" colour.
    let (fill, ink) = match look.tone {
        Tone::Ok => (t.ok, t.on_ok),
        Tone::Err => (t.err, t.on_err),
        Tone::ErrSolid => (gpui::white(), t.err_solid),
        _ => (gpui::transparent_black(), c.fg),
    };
    let ring_color = match look.tone {
        Tone::Muted => t.faint,
        _ => c.fg,
    };
    let el = el.text_color(ink).child(look.glyph);
    match look.shape {
        Shape::Disc => el.rounded_full().bg(fill),
        Shape::Ring => el.rounded_full().border(px(1.5)).border_color(ring_color),
        Shape::DashedRing | Shape::DottedRing => el
            .rounded_full()
            .border(px(1.5))
            .border_dashed()
            .border_color(ring_color),
        Shape::Square => el.rounded(px(3.)).border(px(1.5)).border_color(ring_color),
        Shape::Bare => el,
    }
}

/// A state chip. `label` replaces the generic word ("52 errors", "Applied · verified").
#[derive(IntoElement)]
pub struct Chip {
    look: Look,
    label: Option<SharedString>,
    /// 12px type and a 22px chip, for headers; the default is the 11px/20px dense chip.
    large: bool,
}

impl Chip {
    pub fn new(state: ChipState) -> Self {
        Self::from_look(stage_look(state))
    }

    pub fn from_look(look: Look) -> Self {
        Self {
            look,
            label: None,
            large: false,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[allow(dead_code)] // for headers (D3)
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }

    /// Draw as a previous result that no longer holds.
    pub fn stale(mut self) -> Self {
        self.look = self.look.stale();
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = tokens(cx);
        let look = self.look;
        let c = colors(&t, look.tone);
        let (h, text, g) = if self.large {
            (22., 12., 13.)
        } else {
            (20., 11., 12.)
        };
        let word = self.label.unwrap_or_else(|| look.word.into());
        let mut chip = div()
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(h))
            .pl(px(5.))
            .pr(px(7.))
            .rounded(radius::XS)
            .overflow_hidden()
            .bg(c.bg)
            .text_size(px(text))
            .font_weight(FontWeight::MEDIUM)
            .text_color(c.fg)
            .whitespace_nowrap();
        if let Some(border) = c.border {
            chip = chip.border_1().border_color(border);
            if look.dashed {
                chip = chip.border_dashed();
            }
        }
        if look.dimmed {
            chip = chip.opacity(STALE_OPACITY);
        }
        if look.hatched {
            // Dry run: slash hatch over the dry background, under the content.
            chip = chip.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(pattern_slash(t.dry_hatch, 1.5, 4.5)),
            );
        }
        chip.child(glyph(look, g, &t)).child(word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ChipState; 10] = [
        ChipState::NotRun,
        ChipState::Running,
        ChipState::Clean,
        ChipState::Warnings,
        ChipState::Errors,
        ChipState::Failed,
        ChipState::Interrupted,
        ChipState::Stale,
        ChipState::DryRun,
        ChipState::Skipped,
    ];

    #[test]
    fn every_state_has_a_word_and_a_distinct_look() {
        let looks: Vec<Look> = ALL.iter().map(|s| stage_look(*s)).collect();
        for l in &looks {
            assert!(!l.word.is_empty(), "every state is glyph plus a word");
        }
        // No two states share a colour, shape and mark: nothing relies on colour alone.
        for (i, a) in looks.iter().enumerate() {
            for b in &looks[i + 1..] {
                assert_ne!(
                    (a.tone, a.shape, a.glyph),
                    (b.tone, b.shape, b.glyph),
                    "{} vs {}",
                    a.word,
                    b.word
                );
            }
        }
    }

    #[test]
    fn green_means_written() {
        let green: Vec<_> = ALL
            .iter()
            .filter(|s| stage_look(**s).tone == Tone::Ok)
            .collect();
        assert_eq!(green, [&ChipState::Clean]);
    }

    #[test]
    fn dry_run_is_hatched_and_dashed_and_nothing_else_is_hatched() {
        let dry = stage_look(ChipState::DryRun);
        assert!(dry.hatched && dry.dashed);
        assert_ne!(dry.tone, stage_look(ChipState::Clean).tone);
        for s in ALL.into_iter().filter(|s| *s != ChipState::DryRun) {
            assert!(!stage_look(s).hatched, "{s:?}");
        }
        // Skipped is hollow and solid; not-run is hollow and dashed; neither is the dry look.
        let skipped = stage_look(ChipState::Skipped);
        assert!(!skipped.hatched && !skipped.dashed);
        assert!(stage_look(ChipState::NotRun).dashed);
        assert_ne!(skipped.shape, dry.shape);
    }

    #[test]
    fn warnings_are_square_and_errors_are_round() {
        assert_eq!(stage_look(ChipState::Warnings).shape, Shape::Square);
        assert_eq!(stage_look(ChipState::Errors).shape, Shape::Disc);
        assert_eq!(stage_look(ChipState::Failed).glyph, "✕");
        assert_eq!(stage_look(ChipState::Interrupted).glyph, "‖");
    }

    #[test]
    fn stale_dims_the_previous_result_and_only_dims() {
        for s in ALL {
            let l = stage_look(s);
            assert!(!l.dimmed);
            let stale = l.stale();
            assert!(stale.dimmed);
            assert_eq!(
                (stale.glyph, stale.word, stale.tone),
                (l.glyph, l.word, l.tone)
            );
        }
        assert_eq!(stage_look(ChipState::Stale).glyph, "↻");
    }

    #[test]
    fn engine_states_map_onto_the_table() {
        use ChipState as C;
        let m = |s: StageState| C::from(&s);
        assert_eq!(m(StageState::Never), C::NotRun);
        assert_eq!(m(StageState::Running), C::Running);
        assert_eq!(m(StageState::Clean), C::Clean);
        assert_eq!(m(StageState::Warnings), C::Warnings);
        assert_eq!(m(StageState::Errors), C::Errors);
        assert_eq!(m(StageState::Interrupted), C::Interrupted);
        assert_eq!(m(StageState::CouldNotRun("boom".into())), C::Failed);
        assert_eq!(m(StageState::DryRun), C::DryRun);
        assert_eq!(m(StageState::Skipped), C::Skipped);
    }

    /// Dry run, clean and skipped share no tone, shape or mark, each on its own.
    #[test]
    fn dry_clean_skipped_differ_in_every_way() {
        let l = [ChipState::Clean, ChipState::DryRun, ChipState::Skipped].map(stage_look);
        for (i, a) in l.iter().enumerate() {
            for b in &l[i + 1..] {
                assert_ne!(a.tone, b.tone, "{} vs {}", a.word, b.word);
                assert_ne!(a.shape, b.shape, "{} vs {}", a.word, b.word);
                assert_ne!(a.glyph, b.glyph, "{} vs {}", a.word, b.word);
            }
        }
    }
}
