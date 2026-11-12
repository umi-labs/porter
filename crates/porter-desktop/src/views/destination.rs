//! Destination: the schema check's results grouped by collection, and the schema modes.
//! Generate, Prompt and Spec run as normal runs and then open the folder they wrote to.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div,
};
use gpui_component::button::Button;
use gpui_component::{ActiveTheme, Disableable, h_flex, v_flex};

use super::overview::{error, severity_tone, tone_color};
use crate::project_window::ProjectWindow;
use crate::vm::destination::{Mode, groups};

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    let busy = w.pipeline.busy();
    let buttons = h_flex().gap_2().children(Mode::ALL.into_iter().map(|m| {
        Button::new(m.label())
            .label(m.label())
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| this.run_schema(m, cx)))
    }));
    let help = div().text_sm().text_color(theme.muted_foreground).child(
        "Check compares the mapping with the Payload config. Generate writes collection \
             files to generated/payload/; Prompt and Spec write reports/schema-prompt.md and \
             reports/schema-spec.{md,html}. Nothing is written to the Payload site.",
    );
    let body: gpui::AnyElement = if let Some(e) = w.issues.error() {
        error(e, cx).into_any_element()
    } else if !w.issues.loaded() {
        div().child("Loading…").into_any_element()
    } else {
        let g = groups(w.issues.all());
        if g.is_empty() {
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No open schema check issues. Run Check schema to compare again.")
                .into_any_element()
        } else {
            v_flex()
                .gap_4()
                .children(g.into_iter().map(|g| {
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_3()
                                .child(div().text_lg().child(g.collection))
                                .child(div().text_sm().text_color(theme.muted_foreground).child(
                                    format!(
                                        "{} errors · {} warnings · {} info",
                                        g.errors, g.warnings, g.infos
                                    ),
                                )),
                        )
                        .children(g.messages.into_iter().map(|(sev, m)| {
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    div()
                                        .text_color(tone_color(severity_tone(sev), theme))
                                        .child("●"),
                                )
                                .child(div().min_w_0().child(m))
                        }))
                }))
                .into_any_element()
        }
    };
    v_flex()
        .id("destination")
        .size_full()
        .overflow_y_scroll()
        .p_4()
        .gap_4()
        .child(buttons)
        .child(help)
        .children(w.dest_note.clone().map(|n| div().text_sm().child(n)))
        .child(div().text_lg().child("Schema check results"))
        .child(body)
}
