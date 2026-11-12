//! Porter design tokens (docs/design/tokens) as a GPUI global that follows the system
//! appearance, and the mapping of those tokens onto gpui-component's own theme so stock
//! widgets match.

use gpui::{App, Global, Hsla, Pixels, Window, WindowAppearance, px, rgb};
use gpui_component::{Theme, ThemeMode};

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub bg: Hsla,
    pub surface: Hsla,
    pub panel: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_bg: Hsla,
    pub on_accent: Hsla,
    /// Only for "clean and actually written". Never for a dry run.
    pub ok: Hsla,
    pub ok_bg: Hsla,
    pub on_ok: Hsla,
    pub warn: Hsla,
    pub warn_bg: Hsla,
    pub err: Hsla,
    pub err_bg: Hsla,
    pub err_solid: Hsla,
    pub on_err: Hsla,
    /// Interrupted (exit 3).
    pub int: Hsla,
    pub int_bg: Hsla,
    pub dry: Hsla,
    pub dry_bg: Hsla,
    pub dry_hatch: Hsla,
    pub dry_border: Hsla,
    pub stale_bg: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Tokens {
    pub fn light() -> Self {
        Self {
            bg: c(0xFFFFFF),
            surface: c(0xF7F7F8),
            panel: c(0xEFEFF2),
            hover: c(0xE9E9EE),
            border: c(0xE3E3E8),
            border_strong: c(0xCFCFD6),
            text: c(0x18181B),
            muted: c(0x565662),
            faint: c(0x6E6E7A),
            accent: c(0x3E4FD6),
            accent_bg: c(0xECEEFC),
            on_accent: c(0xFFFFFF),
            ok: c(0x17803D),
            ok_bg: c(0xE6F5EB),
            on_ok: c(0xFFFFFF),
            warn: c(0x9A5800),
            warn_bg: c(0xFFF3DC),
            err: c(0xC42B2B),
            err_bg: c(0xFDECEC),
            err_solid: c(0xC42B2B),
            on_err: c(0xFFFFFF),
            int: c(0x7339BD),
            int_bg: c(0xF2EBFC),
            dry: c(0x3B4A6B),
            dry_bg: c(0xEEF1F7),
            dry_hatch: c(0xE2E7F0),
            dry_border: c(0x8391B0),
            stale_bg: c(0xF1F1F4),
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: c(0x111113),
            surface: c(0x17171A),
            panel: c(0x1E1E22),
            hover: c(0x26262B),
            border: c(0x2A2A30),
            border_strong: c(0x3A3A42),
            text: c(0xEDEDF0),
            muted: c(0xA3A3AE),
            faint: c(0x8C8C98),
            accent: c(0x8C98FF),
            accent_bg: c(0x1E2142),
            on_accent: c(0x0E1030),
            ok: c(0x4ADE80),
            ok_bg: c(0x0F2A19),
            on_ok: c(0x0B1F12),
            warn: c(0xF5B83D),
            warn_bg: c(0x2B210C),
            err: c(0xFF7A7A),
            err_bg: c(0x301515),
            err_solid: c(0xB42A2A),
            on_err: c(0x2A0F0F),
            int: c(0xC3A3FF),
            int_bg: c(0x231B33),
            dry: c(0xAEBBDA),
            dry_bg: c(0x1A1F2B),
            dry_hatch: c(0x222939),
            dry_border: c(0x56648A),
            stale_bg: c(0x1C1C20),
        }
    }
}

pub mod radius {
    use gpui::{Pixels, px};
    pub const XS: Pixels = px(4.); // chips
    pub const SM: Pixels = px(6.); // inputs, buttons
    pub const MD: Pixels = px(8.); // panels, stage cards
    pub const XL: Pixels = px(14.); // sheets
}

// Not used by a view yet (D3: mapping, URLs).
#[allow(dead_code)]
pub mod space {
    use super::*;
    pub const S1: Pixels = px(4.);
    pub const S2: Pixels = px(8.);
    pub const S3: Pixels = px(12.);
    pub const S4: Pixels = px(16.);
    pub const S6: Pixels = px(24.);
    pub const S8: Pixels = px(32.);
}

pub mod text {
    use super::*;
    pub const TITLE: Pixels = px(20.);
    pub const HEADING: Pixels = px(15.);
    pub const BODY: Pixels = px(13.);
    pub const DENSE: Pixels = px(12.);
    pub const MICRO: Pixels = px(11.);
}

// For the tables and sidebar restyle (D3).
#[allow(dead_code)]
pub const ROW_TABLE: Pixels = px(28.);
#[allow(dead_code)]
pub const ROW_SIDEBAR: Pixels = px(32.);

// Dry run: dashed `dry_border` + `dry_bg`, plus gpui::pattern_slash(dry_hatch, ...)
// where available. Running: no animation; the ring glyph is static and the %
// text repaints on progress events.

/// The active palette, kept next to gpui-component's `Theme` and replaced together with it.
struct Palette(Tokens);

impl Global for Palette {}

/// The tokens for the current appearance. Light until `sync` has run.
pub fn tokens(cx: &App) -> Tokens {
    cx.try_global::<Palette>()
        .map(|p| p.0)
        .unwrap_or_else(Tokens::light)
}

pub fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

/// Follows the system appearance: picks light or dark tokens, then sets gpui-component's
/// theme colours from them. Call at start-up (`window: None`) and from each window's
/// appearance observer.
pub fn sync(window: Option<&mut Window>, cx: &mut App) {
    let appearance = window
        .as_ref()
        .map(|w| w.appearance())
        .unwrap_or_else(|| cx.window_appearance());
    let dark = is_dark(appearance);
    let t = if dark {
        Tokens::dark()
    } else {
        Tokens::light()
    };
    // `change` loads the stock palette for the mode; the tokens then overwrite it.
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    apply_to_components(&t, Theme::global_mut(cx));
    cx.set_global(Palette(t));
    if let Some(w) = window {
        w.refresh();
    }
}

/// Sets every colour gpui-component's buttons, inputs, lists, tables, tabs and menus read.
fn apply_to_components(t: &Tokens, th: &mut Theme) {
    th.font_size = text::BODY;
    th.radius = radius::SM;
    th.radius_lg = radius::MD;
    th.background = t.bg;
    th.foreground = t.text;
    th.border = t.border;
    th.input = t.border_strong;
    th.caret = t.accent;
    th.ring = t.accent;
    th.selection = t.accent_bg;
    th.link = t.accent;
    th.link_hover = t.accent;
    th.link_active = t.accent;
    th.muted = t.panel;
    th.muted_foreground = t.muted;
    th.accent = t.hover;
    th.accent_foreground = t.text;
    th.primary = t.accent;
    th.primary_hover = t.accent.opacity(0.9);
    th.primary_active = t.accent.opacity(0.8);
    th.primary_foreground = t.on_accent;
    th.secondary = t.panel;
    th.secondary_hover = t.hover;
    th.secondary_active = t.border;
    th.secondary_foreground = t.text;
    th.danger = t.err_solid;
    th.danger_hover = t.err_solid.opacity(0.9);
    th.danger_active = t.err_solid.opacity(0.8);
    th.danger_foreground = gpui::white();
    th.success = t.ok;
    th.success_hover = t.ok.opacity(0.9);
    th.success_active = t.ok.opacity(0.8);
    th.success_foreground = t.on_ok;
    th.warning = t.warn;
    th.warning_hover = t.warn.opacity(0.9);
    th.warning_active = t.warn.opacity(0.8);
    th.warning_foreground = t.warn_bg;
    th.info = t.accent;
    th.info_hover = t.accent.opacity(0.9);
    th.info_active = t.accent.opacity(0.8);
    th.info_foreground = t.on_accent;
    th.popover = t.bg;
    th.popover_foreground = t.text;
    th.list = t.bg;
    th.list_even = t.bg;
    th.list_head = t.surface;
    th.list_hover = t.hover;
    th.list_active = t.accent_bg;
    th.list_active_border = t.accent;
    th.table = t.bg;
    th.table_even = t.bg;
    th.table_head = t.surface;
    th.table_head_foreground = t.muted;
    th.table_hover = t.hover;
    th.table_active = t.accent_bg;
    th.table_active_border = t.accent;
    th.table_row_border = t.border;
    th.sidebar = t.surface;
    th.sidebar_foreground = t.text;
    th.sidebar_border = t.border;
    th.sidebar_accent = t.hover;
    th.sidebar_accent_foreground = t.text;
    th.sidebar_primary = t.accent;
    th.sidebar_primary_foreground = t.on_accent;
    th.tab = t.bg;
    th.tab_bar = t.surface;
    th.tab_bar_segmented = t.panel;
    th.tab_active = t.bg;
    th.tab_active_foreground = t.text;
    th.tab_foreground = t.muted;
    th.title_bar = t.surface;
    th.title_bar_border = t.border;
    th.window_border = t.border;
    th.group_box = t.surface;
    th.accordion = t.bg;
    th.accordion_hover = t.hover;
    th.skeleton = t.panel;
    th.progress_bar = t.accent;
    th.scrollbar = t.bg.opacity(0.);
    th.scrollbar_thumb = t.border_strong;
    th.scrollbar_thumb_hover = t.faint;
    th.switch = t.border_strong;
    th.switch_thumb = t.bg;
    th.slider_bar = t.accent;
    th.slider_thumb = t.bg;
    th.drag_border = t.accent;
    th.drop_target = t.accent_bg;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_picks_the_theme() {
        assert!(!is_dark(WindowAppearance::Light));
        assert!(!is_dark(WindowAppearance::VibrantLight));
        assert!(is_dark(WindowAppearance::Dark));
        assert!(is_dark(WindowAppearance::VibrantDark));
    }

    #[test]
    fn the_two_palettes_differ_where_it_matters() {
        let (l, d) = (Tokens::light(), Tokens::dark());
        assert_ne!(l.bg, d.bg);
        assert_ne!(l.ok, d.ok);
        // Dry run and clean never share a colour, in either theme.
        for t in [l, d] {
            assert_ne!(t.dry, t.ok);
            assert_ne!(t.dry_bg, t.ok_bg);
        }
    }

    /// The ported values match the source of truth, so a token change cannot drift.
    #[test]
    fn tokens_match_tokens_json() {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/design/tokens/tokens.json")).unwrap();
        for (mode, t) in [("light", Tokens::light()), ("dark", Tokens::dark())] {
            let hex = |k: &str| {
                let s = json["color"][mode][k]
                    .as_str()
                    .unwrap_or_else(|| panic!("{mode}.{k}"));
                c(u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap())
            };
            for (k, v) in [
                ("bg", t.bg),
                ("text", t.text),
                ("muted", t.muted),
                ("accent", t.accent),
                ("ok", t.ok),
                ("warn", t.warn),
                ("err", t.err),
                ("err-solid", t.err_solid),
                ("int", t.int),
                ("dry", t.dry),
                ("dry-border", t.dry_border),
                ("stale-bg", t.stale_bg),
            ] {
                assert_eq!(v, hex(k), "{mode}.{k}");
            }
        }
    }
}
