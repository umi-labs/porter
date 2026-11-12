//! The project window's left sidebar: the eight sections of the spec.

use gpui::{Context, IntoElement, ParentElement, Styled, div};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Selectable, v_flex};

use crate::project_window::ProjectWindow;

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
    Runs,
    /// A section that is not built yet; says so rather than showing an empty pane.
    NotBuilt(&'static str),
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
            Section::Runs => View::Runs,
            s => View::NotBuilt(s.label()),
        }
    }
}

pub fn render(selected: Section, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .w_48()
        .h_full()
        .p_2()
        .gap_1()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(theme.border)
        .children(Section::ALL.into_iter().map(|s| {
            Button::new(s.label())
                .ghost()
                .w_full()
                .selected(s == selected)
                .child(div().w_full().child(s.label()))
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
        assert_eq!(sel.view(), View::NotBuilt("Mapping"));
        assert_eq!(Section::Content.view(), View::Content);
        // Every section maps to some view, and only Overview, Content, Media, URLs and Runs are built.
        let built = Section::ALL
            .iter()
            .filter(|s| !matches!(s.view(), View::NotBuilt(_)))
            .count();
        assert_eq!(built, 5);
        assert_eq!(Section::Media.view(), View::Media);
        assert_eq!(Section::Urls.view(), View::Urls);
    }
}
