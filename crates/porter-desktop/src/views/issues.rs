//! Issues: the filterable table with a selection, bulk resolve (handled manually needs a
//! note) and a jump to the item each issue is about.

use gpui::prelude::*;
use gpui::{Context, IntoElement, ParentElement, Styled, div, px, uniform_list};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::Input;
use gpui_component::{Disableable, Selectable, h_flex, v_flex};
use porter_model::{IssueKind, Resolution, Severity};

use super::overview::error;
use crate::project_window::ProjectWindow;
use crate::theme::{space, text, tokens};
use crate::ui::table::{self, Col, toggle};
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
    let t = tokens(cx);
    let s = &w.issues;
    let status = h_flex().gap(space::S2).children(
        [
            (Status::Open, "Open"),
            (Status::Closed, "Resolved"),
            (Status::All, "All"),
        ]
        .into_iter()
        .map(|(v, label)| {
            toggle(label, label, None, s.status == v, None, cx).on_click(
                cx.listener(move |this, _, _, cx| this.issues_filters(|i| i.status = v, cx)),
            )
        }),
    );
    // Every chip's number is the number of rows it filters to.
    let severity = h_flex().gap(space::S2).children(
        [
            (Severity::Error, "Errors"),
            (Severity::Warning, "Warnings"),
            (Severity::Info, "Info"),
        ]
        .into_iter()
        .map(|(v, label)| {
            let n = s.severity_count(v);
            let on = s.severity == Some(v);
            toggle(
                label,
                label,
                Some(n.to_string()),
                on,
                (n > 0).then_some(v),
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.issues_filters(|i| i.severity = if on { None } else { Some(v) }, cx)
            }))
        }),
    );
    let kinds = h_flex()
        .gap(space::S2)
        .flex_wrap()
        .children(KINDS.into_iter().filter_map(|k| {
            let n = s.kind_count(k);
            let on = s.kind == Some(k);
            // A kind with nothing in it is left out, unless it is the one being filtered to.
            (n > 0 || on).then(|| {
                toggle(
                    ("kind", k as usize),
                    kind_name(k),
                    Some(n.to_string()),
                    on,
                    None,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.issues_filters(|i| i.kind = if on { None } else { Some(k) }, cx)
                }))
            })
        }));
    let resolve = h_flex()
        .gap(space::S2)
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
    let n = s.visible().len();
    let body: gpui::AnyElement = if let Some(e) = s.error() {
        error(e, cx).into_any_element()
    } else if !s.loaded() {
        table::loading("Reading issues\u{2026}", cx).into_any_element()
    } else if let Some((title, why)) = s.empty() {
        table::empty(title, why, cx).into_any_element()
    } else {
        // ponytail: filters the whole list per paint; cache the visible ids if a project
        // holds 100k+ issues.
        uniform_list(
            "issues-table",
            n,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let t = tokens(cx);
                let visible = this.issues.visible();
                range
                    .filter_map(|i| {
                        let it = *visible.get(i)?;
                        let (id, item) = (it.id, it.item);
                        let picked = this.issues.is_selected(id);
                        let open = it.resolution == Resolution::Open;
                        Some(
                            table::row(("issue-row", i), open.then_some(it.severity), false, cx)
                                .child(
                                    div()
                                        .id(("pick", i))
                                        .w(px(24.))
                                        .flex_none()
                                        .cursor_pointer()
                                        .child(if picked { "\u{2611}" } else { "\u{2610}" })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.issues_filters(|s| s.toggle(id), cx)
                                        })),
                                )
                                .child(table::mark(it.severity, cx))
                                .child(table::cell(
                                    ("issue-kind", i),
                                    kind_name(it.kind),
                                    Col::Fixed(150.),
                                ))
                                .child(table::cell(("issue-msg", i), it.message.clone(), Col::Flex))
                                .child(
                                    table::cell(
                                        ("issue-res", i),
                                        resolution_text(&it.resolution),
                                        Col::Fixed(150.),
                                    )
                                    .text_color(t.muted),
                                )
                                .child(div().w(px(80.)).flex_none().children(item.map(|item| {
                                    div()
                                        .id(("jump", i))
                                        .text_color(t.accent)
                                        .cursor_pointer()
                                        .child("Go to item")
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
        .p(space::S4)
        .gap(space::S3)
        .child(table::page_head("Issues", s.summary(), cx))
        .child(
            h_flex()
                .gap(space::S3)
                .items_center()
                .flex_wrap()
                .child(status)
                .child(severity)
                .child(div().w_64().child(Input::new(&w.issues_filter))),
        )
        .child(kinds)
        .child(resolve)
        .children(
            s.note
                .clone()
                .map(|n| div().text_size(text::DENSE).text_color(t.muted).child(n)),
        )
        .child(table::header(
            &[
                ("", Col::Fixed(24.)),
                ("", Col::Fixed(14.)),
                ("Kind", Col::Fixed(150.)),
                ("Message", Col::Flex),
                ("Status", Col::Fixed(150.)),
                ("", Col::Fixed(80.)),
            ],
            cx,
        ))
        .child(div().flex_1().min_h_0().child(body))
}
