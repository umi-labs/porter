//! Issues: the filterable table with a selection, bulk resolve (handled manually needs a
//! note) and a jump to the item each issue is about.

use gpui::{Context, InteractiveElement, IntoElement, ParentElement, Styled, div, uniform_list};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{ActiveTheme, Disableable, Selectable, h_flex, v_flex};
use porter_model::{IssueKind, Resolution, Severity};

use super::overview::{error, severity_tone, tone_color};
use crate::project_window::ProjectWindow;
use crate::vm::issues::{Choice, Status, kind_name};

const KINDS: [IssueKind; 12] = [
    IssueKind::UnresolvedLink,
    IssueKind::MissingMedia,
    IssueKind::UnmappedField,
    IssueKind::UnsupportedBlock,
    IssueKind::UnresolvedRelation,
    IssueKind::ParentMissing,
    IssueKind::RedirectChain,
    IssueKind::RedirectLoop,
    IssueKind::WriteFailed,
    IssueKind::AccessGap,
    IssueKind::ConverterMissing,
    IssueKind::SchemaMismatch,
];

const CHOICES: [(Choice, &str); 3] = [
    (Choice::HandledManually, "Handled manually (needs a note)"),
    (Choice::WontFix, "Won't fix"),
    (Choice::Reopen, "Reopen"),
];

fn resolution_text(r: &Resolution) -> String {
    match r {
        Resolution::Open => "open".into(),
        Resolution::FixedByRerun => "fixed by rerun".into(),
        Resolution::HandledManually { note } => format!("handled: {note}"),
        Resolution::WontFix => "won't fix".into(),
    }
}

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    let s = &w.issues;
    let status = h_flex().gap_1().items_center().children(
        [
            (Status::Open, "Open"),
            (Status::Closed, "Resolved"),
            (Status::All, "All"),
        ]
        .into_iter()
        .map(|(v, label)| {
            Button::new(label)
                .ghost()
                .label(label)
                .selected(s.status == v)
                .on_click(
                    cx.listener(move |this, _, _, cx| this.issues_filters(|i| i.status = v, cx)),
                )
        }),
    );
    let severity = h_flex().gap_1().items_center().children(
        [
            (None, "Any severity"),
            (Some(Severity::Error), "Errors"),
            (Some(Severity::Warning), "Warnings"),
            (Some(Severity::Info), "Info"),
        ]
        .into_iter()
        .map(|(v, label)| {
            Button::new(label)
                .ghost()
                .label(label)
                .selected(s.severity == v)
                .on_click(
                    cx.listener(move |this, _, _, cx| this.issues_filters(|i| i.severity = v, cx)),
                )
        }),
    );
    let kinds = h_flex()
        .gap_1()
        .flex_wrap()
        .child(
            Button::new("kind-any")
                .ghost()
                .label("Any kind")
                .selected(s.kind.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.issues_filters(|i| i.kind = None, cx))),
        )
        .children(KINDS.into_iter().map(|k| {
            Button::new(("kind", k as usize))
                .ghost()
                .label(kind_name(k))
                .selected(s.kind == Some(k))
                .on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.issues_filters(|i| i.kind = Some(k), cx)
                    }),
                )
        }));
    let resolve = h_flex()
        .gap_2()
        .items_center()
        .child(
            Button::new("select-all")
                .label("Select all / none")
                .on_click(
                    cx.listener(|this, _, _, cx| this.issues_filters(|i| i.toggle_all(), cx)),
                ),
        )
        .children(CHOICES.into_iter().map(|(c, label)| {
            Button::new(label)
                .ghost()
                .label(label)
                .selected(w.resolve_choice == c)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.resolve_choice = c;
                    cx.notify();
                }))
        }))
        .child(div().w_64().child(Input::new(&w.resolve_note)))
        .child(
            Button::new("resolve")
                .label("Apply to selected")
                .disabled(s.selected_count() == 0)
                .on_click(cx.listener(|this, _, _, cx| this.issues_resolve(cx))),
        );
    let header = h_flex()
        .w_full()
        .gap_2()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(theme.muted_foreground)
        .border_b_1()
        .border_color(theme.border)
        .child(div().w_8())
        .child(div().w_4())
        .child(div().w_40().child("Kind"))
        .child(div().flex_1().child("Message"))
        .child(div().w_48().child("Status"))
        .child(div().w_24());
    let n = s.visible().len();
    let body: gpui::AnyElement = if let Some(e) = s.error() {
        error(e, cx).into_any_element()
    } else if !s.loaded() {
        div().p_2().child("Loading…").into_any_element()
    } else if n == 0 {
        div()
            .p_2()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(if s.all().is_empty() {
                "No issues."
            } else {
                "No issue matches these filters."
            })
            .into_any_element()
    } else {
        // ponytail: filters the whole list per paint; cache the visible ids if a project
        // holds 100k+ issues.
        uniform_list(
            "issues-table",
            n,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let theme = cx.theme();
                let visible = this.issues.visible();
                range
                    .filter_map(|i| {
                        let it = *visible.get(i)?;
                        let (id, item) = (it.id, it.item);
                        let picked = this.issues.is_selected(id);
                        Some(
                            h_flex()
                                .id(("issue-row", i))
                                .w_full()
                                .gap_2()
                                .px_2()
                                .items_center()
                                .text_sm()
                                .whitespace_nowrap()
                                .child(
                                    div().w_8().child(
                                        Button::new(("pick", i))
                                            .ghost()
                                            .label(if picked { "☑" } else { "☐" })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.issues_filters(|s| s.toggle(id), cx)
                                            })),
                                    ),
                                )
                                .child(
                                    div()
                                        .w_4()
                                        .text_color(tone_color(severity_tone(it.severity), theme))
                                        .child("●"),
                                )
                                .child(div().w_40().child(kind_name(it.kind)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .child(it.message.clone()),
                                )
                                .child(
                                    div()
                                        .w_48()
                                        .overflow_hidden()
                                        .child(resolution_text(&it.resolution)),
                                )
                                .child(div().w_24().children(item.map(|item| {
                                    Button::new(("jump", i))
                                        .ghost()
                                        .label("Go to item")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.issues_jump(item, cx)
                                        }))
                                }))),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .into_any_element()
    };
    v_flex()
        .size_full()
        .p_2()
        .gap_2()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(status)
                .child(severity)
                .child(div().w_64().child(Input::new(&w.issues_filter)))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(s.summary()),
                ),
        )
        .child(kinds)
        .child(resolve)
        .children(s.note.clone().map(|n| div().text_sm().child(n)))
        .child(header)
        .child(div().flex_1().min_h_0().child(body))
}
