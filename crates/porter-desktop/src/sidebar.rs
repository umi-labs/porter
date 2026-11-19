//! The project window's left sidebar: the eight sections of the spec.

use gpui::prelude::*;
use gpui::{Context, FontWeight, IntoElement, ParentElement, Styled, div, px};
use gpui_component::{h_flex, v_flex};

use crate::project_window::ProjectWindow;
use crate::theme::{ROW_SIDEBAR, radius, space, text, tokens};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Overview,
    Content,
    Media,
    Urls,
    Issues,
    Mapping,
    Destination,
    Runs,
}

/// What the main pane shows for a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Overview,
    Content,
    Media,
    Urls,
    Mapping,
    Issues,
    Destination,
    Runs,
}

impl Section {
    pub const ALL: [Section; 8] = [
        Section::Overview,
        Section::Content,
        Section::Media,
        Section::Urls,
        Section::Issues,
        Section::Mapping,
        Section::Destination,
        Section::Runs,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::Content => "Content",
            Section::Media => "Media",
            Section::Urls => "URLs",
            Section::Issues => "Issues",
            Section::Mapping => "Mapping",
            Section::Destination => "Destination",
            Section::Runs => "Runs",
        }
    }

    pub fn view(self) -> View {
        match self {
            Section::Overview => View::Overview,
            Section::Content => View::Content,
            Section::Media => View::Media,
            Section::Urls => View::Urls,
            Section::Mapping => View::Mapping,
            Section::Issues => View::Issues,
            Section::Destination => View::Destination,
            Section::Runs => View::Runs,
        }
    }
}

/// The count beside a section's name: what needs attention there. Nothing when it has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Badge {
    pub count: u64,
    /// Some of what it counts is an error.
    pub error: bool,
}

/// Issues counts the open errors and warnings; Mapping counts the problems on its rows
/// (None until the mapping has been read).
pub fn badge(
    s: Section,
    issues: Option<(u64, u64)>,
    mapping: Option<(usize, usize)>,
) -> Option<Badge> {
    let (errors, warnings) = match s {
        Section::Issues => issues?,
        Section::Mapping => mapping.map(|(e, w)| (e as u64, w as u64))?,
        _ => return None,
    };
    (errors + warnings > 0).then_some(Badge {
        count: errors + warnings,
        error: errors > 0,
    })
}

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let t = tokens(cx);
    let issues = w
        .pipeline
        .overview
        .as_ref()
        .map(|o| (o.open_errors, o.open_warnings));
    let mapping = w.mapping.problem_counts();
    v_flex()
        .w_48()
        .flex_none()
        .h_full()
        .p(space::S2)
        .gap(px(2.))
        .bg(t.surface)
        .border_r_1()
        .border_color(t.border)
        .children(Section::ALL.into_iter().enumerate().map(|(i, s)| {
            let on = s == w.section;
            let b = badge(s, issues, mapping);
            h_flex()
                .id(("nav", i))
                .h(ROW_SIDEBAR)
                .flex_none()
                .items_center()
                .gap(space::S2)
                .px(space::S3)
                .rounded(radius::SM)
                .text_size(text::BODY)
                .text_color(t.text)
                .cursor_pointer()
                .when(on, |d| d.bg(t.accent_bg).font_weight(FontWeight::SEMIBOLD))
                .when(!on, |d| d.hover(|d| d.bg(t.hover)))
                .child(div().flex_1().min_w_0().truncate().child(s.label()))
                .children(b.map(|b| {
                    // Not colour alone: the number is the count, and the tint says errors.
                    div()
                        .flex_none()
                        .px(px(6.))
                        .rounded(radius::XS)
                        .text_size(text::MICRO)
                        .font_weight(FontWeight::SEMIBOLD)
                        .bg(if b.error { t.err_bg } else { t.warn_bg })
                        .text_color(if b.error { t.err } else { t.warn })
                        .child(b.count.to_string())
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.select(s, cx)))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_selection_switches_view() {
        let mut sel = Section::default();
        assert_eq!(sel.view(), View::Overview);
        sel = Section::Runs;
        assert_eq!(sel.view(), View::Runs);
        sel = Section::Mapping;
        assert_eq!(sel.view(), View::Mapping);
        assert_eq!(Section::Issues.view(), View::Issues);
        assert_eq!(Section::Destination.view(), View::Destination);
        assert_eq!(Section::Content.view(), View::Content);
        assert_eq!(Section::Media.view(), View::Media);
        assert_eq!(Section::Urls.view(), View::Urls);
    }

    #[test]
    fn badges_count_what_needs_attention_and_nothing_else() {
        let b = |s, i, m| badge(s, i, m);
        assert_eq!(
            b(Section::Issues, Some((2, 3)), None),
            Some(Badge {
                count: 5,
                error: true
            })
        );
        assert_eq!(
            b(Section::Issues, Some((0, 3)), None),
            Some(Badge {
                count: 3,
                error: false
            })
        );
        assert_eq!(b(Section::Issues, Some((0, 0)), None), None);
        assert_eq!(b(Section::Issues, None, None), None, "not read yet");
        assert_eq!(
            b(Section::Mapping, None, Some((4, 1))),
            Some(Badge {
                count: 5,
                error: true
            })
        );
        assert_eq!(b(Section::Mapping, None, None), None);
        for s in [
            Section::Overview,
            Section::Content,
            Section::Urls,
            Section::Runs,
        ] {
            assert_eq!(b(s, Some((9, 9)), Some((9, 9))), None);
        }
    }
}
