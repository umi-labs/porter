//! Media: a virtualised thumbnail grid (only the rows in view are built; thumbnails are
//! decoded off-thread and shown when ready) and the selected item's variants, usages and
//! issues. A file that is missing or will not decode shows why instead of a picture.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, img, prelude::FluentBuilder, px, uniform_list,
};
use gpui_component::{ActiveTheme, h_flex, v_flex};

use super::content::issues_list;
use super::overview::error;
use crate::project_window::ProjectWindow;
use crate::thumbs::Thumb;
use crate::vm::media::usage_line;

/// Grid columns.
// ponytail: fixed; derive from the pane width if narrow windows clip.
const COLS: usize = 5;
const CELL: f32 = 136.0;

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let theme = cx.theme();
    let rows = w.media.grid_rows(COLS);
    let grid = uniform_list(
        "media-grid",
        rows,
        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
            if let Some(last) = range.end.checked_sub(1) {
                this.media_pump(last, COLS);
            }
            range.map(|r| grid_row(this, r, cx)).collect::<Vec<_>>()
        }),
    )
    .size_full();
    let footer = match w.media.error() {
        Some(e) => error(e, cx).into_any_element(),
        None => div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(w.media.summary())
            .into_any_element(),
    };
    h_flex()
        .size_full()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .p_2()
                .gap_2()
                .child(div().flex_1().min_h_0().child(grid))
                .child(footer),
        )
        .child(
            v_flex()
                .id("media-detail")
                .w_96()
                .h_full()
                .overflow_y_scroll()
                .p_4()
                .gap_4()
                .border_l_1()
                .border_color(theme.border)
                .child(detail(w, cx)),
        )
}

fn grid_row(w: &mut ProjectWindow, r: usize, cx: &mut Context<ProjectWindow>) -> gpui::AnyElement {
    let items: Vec<_> = w.media.grid_row(r, COLS).to_vec();
    let selected = w.media.selected();
    let cells: Vec<gpui::AnyElement> = items
        .into_iter()
        .enumerate()
        .map(|(c, row)| {
            let thumb = w.thumb(&row);
            let theme = cx.theme();
            let id = row.id;
            let picture = match thumb {
                Some(Thumb::Ready(image)) => img(image)
                    .max_w(px(CELL - 8.0))
                    .max_h(px(CELL - 40.0))
                    .into_any_element(),
                Some(Thumb::Placeholder(why)) => div()
                    .p_1()
                    .text_xs()
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child(why)
                    .into_any_element(),
                None => div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("…")
                    .into_any_element(),
            };
            v_flex()
                .id(("media-cell", r * COLS + c))
                .w(px(CELL))
                .h(px(CELL))
                .p_1()
                .gap_1()
                .border_1()
                .border_color(if selected == Some(id) {
                    theme.accent_foreground
                } else {
                    theme.border
                })
                .when(selected == Some(id), |d| d.bg(theme.accent))
                .child(
                    div()
                        .h(px(CELL - 40.0))
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .child(picture),
                )
                .child(
                    div()
                        .text_xs()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(row.filename),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.media_select(id, cx)))
                .into_any_element()
        })
        .collect();
    h_flex()
        .w_full()
        .h(px(CELL + 4.0))
        .gap_1()
        .children(cells)
        .into_any_element()
}

fn detail(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> gpui::AnyElement {
    let theme = cx.theme();
    let muted = |t: &str| {
        div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(t.to_string())
    };
    let d = match w.media.detail() {
        None if w.media.selected().is_none() => {
            return muted("Select a media item to see where it is used.").into_any_element();
        }
        None => return muted("Loading…").into_any_element(),
        Some(Err(e)) => return error(e, cx).into_any_element(),
        Some(Ok(d)) => d,
    };
    let kv = |k: &str, v: String| {
        h_flex()
            .gap_2()
            .text_sm()
            .child(
                div()
                    .w_24()
                    .flex_none()
                    .text_color(theme.muted_foreground)
                    .child(k.to_string()),
            )
            .child(div().min_w_0().child(v))
    };
    let heading = |t: &'static str| div().text_lg().child(t);
    let mut out = v_flex().gap_4();
    match &d.media {
        Ok(m) => {
            let size = match (m.width, m.height) {
                (Some(w), Some(h)) => format!("{w} × {h}"),
                _ => "unknown".into(),
            };
            out = out
                .child(div().text_xl().child(m.filename.clone()))
                .child(
                    v_flex()
                        .gap_1()
                        .child(kv("Type", m.mime.clone()))
                        .child(kv("Size", size))
                        .child(kv("Alt", m.alt.clone().unwrap_or_else(|| "(none)".into())))
                        .child(kv("Hash", m.hash.clone()))
                        .child(kv(
                            "Source",
                            format!("{} {}", d.source.connector, d.source.source_id),
                        )),
                )
                .child(heading("Variants"))
                .child(if m.variants.is_empty() {
                    muted("No old URL.").into_any_element()
                } else {
                    v_flex()
                        .gap_1()
                        .children(m.variants.iter().map(|v| div().text_sm().child(v.clone())))
                        .into_any_element()
                })
                .child(heading("Used by"))
                .child(if m.usages.is_empty() {
                    muted("Not used by any document.").into_any_element()
                } else {
                    v_flex()
                        .gap_1()
                        .children(m.usages.iter().map(|u| {
                            let (line, bad) = usage_line(u, w.media.labels());
                            div()
                                .text_sm()
                                .when(bad, |d| d.text_color(theme.danger))
                                .child(line)
                        }))
                        .into_any_element()
                });
        }
        Err(e) => out = out.child(div().text_sm().text_color(theme.danger).child(e.clone())),
    }
    if let Some(e) = &w.media.labels_error {
        out = out.child(error(&format!("Could not look up documents: {e}"), cx));
    }
    out = out
        .child(heading("Issues"))
        .child(issues_list(&d.issues, cx));
    out.into_any_element()
}
