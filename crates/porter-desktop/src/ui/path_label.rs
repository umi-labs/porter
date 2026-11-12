//! Long paths truncate in the middle: the head shrinks, the last two segments stay whole,
//! and the full path is the tooltip.

use gpui::prelude::*;
use gpui::{
    App, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px,
};
use gpui_component::ActiveTheme;
use gpui_component::tooltip::Tooltip;

use crate::theme::tokens;

/// Splits at the separator before the last two segments: ("/Users/isj/Clients", "/Umi/porter").
/// A path with two or fewer segments has no head.
pub fn split_tail(path: &str) -> (&str, &str) {
    let p = path.trim_end_matches('/');
    match p.rmatch_indices('/').nth(1) {
        Some((i, _)) => p.split_at(i),
        None => ("", p),
    }
}

/// `path` in at most `max_chars` characters, middle-truncated. The last two segments are
/// kept whole; if even those do not fit, the second-last loses its head. A last segment
/// longer than `max_chars` is never cut, so the result can then exceed the budget.
pub fn middle_truncate(path: &str, max_chars: usize) -> String {
    let len = |s: &str| s.chars().count();
    if len(path) <= max_chars {
        return path.to_string();
    }
    let (head, tail) = split_tail(path);
    let room = max_chars.saturating_sub(len(tail) + 1);
    if len(tail) < max_chars {
        let kept: String = head.chars().take(room).collect();
        return format!("{kept}…{tail}");
    }
    // The tail alone is too long: keep the last segment, shorten the second-last from its head.
    let tail = tail.trim_start_matches('/');
    let (second, last) = tail.rsplit_once('/').unwrap_or(("", tail));
    let room = max_chars.saturating_sub(len(last) + 2);
    let skip = len(second).saturating_sub(room);
    let second: String = second.chars().skip(skip).collect();
    format!("…{second}/{last}")
}

/// A path that fits its row: mono, the head ellipsised by layout, the tail never.
#[derive(IntoElement)]
pub struct PathLabel {
    id: ElementId,
    path: SharedString,
    /// Drawn in the error colour (a folder that is gone).
    missing: bool,
}

impl PathLabel {
    pub fn new(id: impl Into<ElementId>, path: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            missing: false,
        }
    }

    pub fn missing(mut self, missing: bool) -> Self {
        self.missing = missing;
        self
    }
}

impl RenderOnce for PathLabel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = tokens(cx);
        let full = self.path.clone();
        let (head, tail) = split_tail(&self.path);
        let (head, tail) = (head.to_string(), tail.to_string());
        div()
            .id(self.id)
            .flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(px(11.))
            .text_color(if self.missing { t.err } else { t.muted })
            .when(!head.is_empty(), |d| {
                d.child(div().min_w_0().flex_shrink().truncate().child(head))
            })
            .child(div().flex_none().whitespace_nowrap().child(tail))
            .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: &str =
        "/Users/isj/Clients/Umi/Seaview Hotels/2026 rebuild/migration/seaview-porter";

    #[test]
    fn the_tail_is_the_last_two_segments() {
        assert_eq!(
            split_tail(LONG),
            (
                "/Users/isj/Clients/Umi/Seaview Hotels/2026 rebuild",
                "/migration/seaview-porter"
            )
        );
        assert_eq!(split_tail("/a/b"), ("", "/a/b"));
        assert_eq!(split_tail("/a"), ("", "/a"));
        assert_eq!(split_tail("a"), ("", "a"));
        assert_eq!(
            split_tail("/a/b/c/"),
            ("/a", "/b/c"),
            "a trailing slash is ignored"
        );
    }

    #[test]
    fn a_path_that_fits_is_untouched() {
        assert_eq!(middle_truncate("/a/b/c", 6), "/a/b/c");
        assert_eq!(middle_truncate(LONG, 200), LONG);
    }

    #[test]
    fn the_head_shrinks_and_the_last_two_segments_stay() {
        let out = middle_truncate(LONG, 40);
        assert_eq!(out.chars().count(), 40);
        assert!(out.ends_with("…/migration/seaview-porter"), "{out}");
        assert!(out.starts_with("/Users/isj/"), "{out}");
    }

    #[test]
    fn when_the_tail_does_not_fit_the_second_last_loses_its_head() {
        let out = middle_truncate(LONG, 20);
        assert!(out.chars().count() <= 20, "{out}");
        assert!(out.ends_with("/seaview-porter"), "{out}");
        assert!(out.starts_with('…'), "{out}");
        assert_eq!(out, "…tion/seaview-porter");
    }

    #[test]
    fn a_last_segment_too_long_is_kept_whole() {
        assert_eq!(middle_truncate("/a/bbbb/cccccccccc", 6), "…/cccccccccc");
    }

    #[test]
    fn counts_characters_not_bytes() {
        let out = middle_truncate("/Users/ísj/Cliéntes/Umi/porté", 18);
        assert!(out.chars().count() <= 18, "{out}");
        assert!(out.ends_with("/Umi/porté"), "{out}");
    }
}
