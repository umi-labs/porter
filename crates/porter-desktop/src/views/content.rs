//! Content: the document tree (type, then parent hierarchy, loaded per node) and the
//! selected document's detail. Links that point nowhere are red and say so.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px, uniform_list,
};
use gpui_component::{ActiveTheme, h_flex, v_flex};

use super::overview::{error, severity_tone, tone_color};
use crate::project_window::ProjectWindow;
use crate::vm::content::{Detail, Line, Mark, NodeKey, Row, RowKind, status_text, status_tone};

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    let n = w.content.rows().len();
    let tree = uniform_list(
        "content-tree",
        n,
        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
            let rows = this.content.rows();
            range
                .filter_map(|i| Some(tree_row(i, rows.get(i)?.clone(), cx)))
                .collect::<Vec<_>>()
        }),
    )
    .size_full();
    h_flex()
        .size_full()
        .child(
            div()
                .w_96()
                .h_full()
                .p_2()
                .border_r_1()
                .border_color(theme.border)
                .child(tree),
        )
        .child(
            v_flex()
                .id("content-detail")
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_y_scroll()
                .p_4()
                .gap_4()
                .child(detail(w, cx)),
        )
}

fn tree_row(i: usize, row: Row, cx: &mut Context<ProjectWindow>) -> gpui::AnyElement {
    let theme = cx.theme();
    let base = h_flex()
        .id(("content-row", i))
        .w_full()
        .gap_1()
        .pl(px(row.depth as f32 * 16.0))
        .text_sm()
        .whitespace_nowrap()
        .overflow_hidden();
    let arrow = |open: bool| if open { "▾" } else { "▸" };
    match row.kind {
        RowKind::Type {
            name,
            count,
            expanded,
        } => {
            let key = NodeKey::Type(name.clone());
            base.child(format!("{} {name} ({count})", arrow(expanded)))
                .on_click(cx.listener(move |this, _, _, cx| this.content_toggle(key.clone(), cx)))
                .into_any_element()
        }
        RowKind::Doc {
            node,
            expanded,
            selected,
        } => {
            let id = node.id;
            let key = NodeKey::Doc(id);
            let toggle = div()
                .id(("content-toggle", i))
                .w_4()
                .child(if node.has_children {
                    arrow(expanded)
                } else {
                    ""
                });
            let toggle = if node.has_children {
                toggle.on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.content_toggle(key.clone(), cx)
                }))
            } else {
                toggle
            };
            let dot = node.worst_status.as_ref().map(|s| {
                div()
                    .text_color(tone_color(status_tone(s), theme))
                    .child("●")
            });
            let title = if node.title.is_empty() {
                node.path.clone()
            } else {
                node.title.clone()
            };
            base.when(selected, |b| b.bg(theme.accent))
                .child(toggle)
                .children(dot)
                .child(title)
                .child(div().text_color(theme.muted_foreground).child(node.path))
                .on_click(cx.listener(move |this, _, _, cx| this.content_select(id, cx)))
                .into_any_element()
        }
        RowKind::More {
            parent,
            shown,
            total,
        } => base
            .text_color(theme.info)
            .child(format!("Load more ({shown} of {total} shown)"))
            .on_click(cx.listener(move |this, _, _, cx| this.content_more(parent.clone(), cx)))
            .into_any_element(),
        RowKind::Loading => base
            .text_color(theme.muted_foreground)
            .child("Loading…")
            .into_any_element(),
        RowKind::Error(e) => base.text_color(theme.danger).child(e).into_any_element(),
        RowKind::Empty(why) => base
            .text_color(theme.muted_foreground)
            .child(why)
            .into_any_element(),
    }
}

fn heading(t: &'static str) -> impl IntoElement {
    div().text_lg().child(t)
}

fn detail(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> gpui::AnyElement {
    let theme = cx.theme();
    let muted = |t: &str| {
        div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(t.to_string())
    };
    let d: &Detail = match w.content.detail() {
        None if w.content.selected().is_none() => {
            return muted("Select a document to see where it came from and where it went.")
                .into_any_element();
        }
        None => return muted("Loading…").into_any_element(),
        Some(Err(e)) => return error(e, cx).into_any_element(),
        Some(Ok(d)) => d,
    };
    let labels = w.content.labels();
    let kv = |k: &str, v: String| {
        h_flex()
            .gap_2()
            .text_sm()
            .child(
                div()
                    .w_32()
                    .text_color(theme.muted_foreground)
                    .child(k.to_string()),
            )
            .child(v)
    };
    let mut out = v_flex().gap_4();
    let src = &d.item.source;
    let mut head = v_flex().gap_1().child(
        div().text_xl().child(
            d.item
                .row
                .title
                .clone()
                .unwrap_or_else(|| "(untitled)".into()),
        ),
    );
    head = head.child(kv(
        "Old URL",
        src.url.clone().unwrap_or_else(|| "(none)".into()),
    ));
    match &d.doc {
        Ok(doc) => {
            head = head
                .child(kv("New path", doc.path.clone()))
                .child(kv("Type", doc.doc_type.clone()))
                .child(kv("Status", format!("{:?}", doc.status).to_lowercase()));
            if let Some(p) = doc.parent {
                let v = match labels.get(&p) {
                    Some(Some(t)) => format!("→ {t}"),
                    Some(None) => format!("unresolved: no document {p}"),
                    None => "→ resolving…".into(),
                };
                head = head.child(kv("Parent", v));
            }
        }
        Err(e) => head = head.child(div().text_sm().text_color(theme.danger).child(e.clone())),
    }
    head = head.child(kv("Source", format!("{} {}", src.connector, src.source_id)));
    out = out.child(head);
    if let Some(e) = &w.content.labels_error {
        out = out.child(
            div()
                .text_sm()
                .text_color(theme.danger)
                .child(format!("Could not look up link targets: {e}")),
        );
    }

    let statuses = d.statuses();
    out = out
        .child(heading("Statuses"))
        .child(if statuses.is_empty() {
            muted("No stage has given it a status yet.").into_any_element()
        } else {
            v_flex()
                .gap_1()
                .children(statuses.into_iter().map(|(stage, st)| {
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(div().w_32().child(stage))
                        .child(
                            div()
                                .text_color(tone_color(status_tone(st), theme))
                                .child(status_text(st)),
                        )
                }))
                .into_any_element()
        });

    let fields = d.fields(labels);
    out = out.child(heading("Fields")).child(if fields.is_empty() {
        muted("No fields.").into_any_element()
    } else {
        v_flex()
            .children(fields.into_iter().map(|f| {
                h_flex()
                    .gap_2()
                    .py_1()
                    .text_sm()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().w_48().flex_none().child(f.name))
                    .child(
                        div()
                            .min_w_0()
                            .when(f.unresolved, |d| d.text_color(theme.danger))
                            .child(f.value),
                    )
            }))
            .into_any_element()
    });

    for (label, lines) in d.rich_texts(labels) {
        out = out
            .child(div().text_lg().child(format!("Rich text: {label}")))
            .child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .children(lines.into_iter().map(|l| rt_line(l, cx))),
            );
    }

    let outline = d.outline();
    if !outline.is_empty() {
        out = out
            .child(heading("Layout"))
            .child(v_flex().children(outline.into_iter().map(|(depth, label)| {
                div()
                    .text_sm()
                    .font_family("monospace")
                    .pl(px(depth as f32 * 16.0))
                    .child(label)
            })));
    }

    out = out
        .child(heading("Issues"))
        .child(issues_list(&d.item.issues, cx));
    out.into_any_element()
}

fn rt_line(l: Line, cx: &Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    let row = h_flex()
        .flex_wrap()
        .gap_x_1()
        .text_sm()
        .child(div().font_family("monospace").child(l.prefix));
    let row = if l.heading { row.text_base() } else { row };
    row.children(l.spans.into_iter().map(|s| {
        let color = match s.mark {
            Mark::Plain => None,
            Mark::Resolved | Mark::External => Some(theme.info),
            Mark::Pending => Some(theme.muted_foreground),
            Mark::Unresolved => Some(theme.danger),
        };
        let text = match &s.note {
            Some(n) => format!("{} [{n}]", s.text),
            None => s.text,
        };
        let el = div().child(text);
        match color {
            Some(c) => el.text_color(c).underline(),
            None => el,
        }
    }))
}

/// An item's issues, or that it has none.
pub fn issues_list(
    issues: &[porter_model::Issue],
    cx: &Context<ProjectWindow>,
) -> gpui::AnyElement {
    let theme = cx.theme();
    if issues.is_empty() {
        return div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("No issues.")
            .into_any_element();
    }
    v_flex()
        .gap_1()
        .children(issues.iter().map(|i| {
            let kind = serde_json::to_value(i.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .w_32()
                        .text_color(tone_color(severity_tone(i.severity), theme))
                        .child(kind),
                )
                .child(div().min_w_0().child(i.message.clone()))
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("{:?}", i.resolution).to_lowercase()),
                )
        }))
        .into_any_element()
}
