//! Mapping: the types, fields and blocks tables over porter.toml's `[mapping]`, each row
//! with the schema check's issues under it, and an editor for the selected row. Every
//! change saves at once (comments kept) and the check re-runs in the background.

use gpui::prelude::*;
use gpui::{
    AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme, Disableable, h_flex, v_flex};
use porter_engine::mapping::KeyType;
use porter_engine::mapping::edit::{BlockEdit, FieldMapEdit, MappingDoc};

use porter_engine::session::RunPlan;
use porter_model::{Severity, Stage};

use super::overview::error;
use crate::project_window::{ProjectWindow, dry};
use crate::theme::{ROW_TABLE, radius, space, text, tokens};
use crate::ui::buttons::{Kind, button};
use crate::ui::chip::{Chip, ChipState};
use crate::ui::table::{self, Col};
use crate::vm::mapping::{
    self, Mapping, Picker, RowIssue, move_transform, parse_pairs, parse_transform, transform_label,
};

/// The editor's text boxes, refilled when the selection changes or a save lands.
pub struct Inputs {
    type_to: Entity<InputState>,
    path: Entity<InputState>,
    key: Entity<InputState>,
    status: Entity<InputState>,
    type_reason: Entity<InputState>,
    field_to: Entity<InputState>,
    field_reason: Entity<InputState>,
    transform: Entity<InputState>,
    block_to: Entity<InputState>,
    block_fields: Entity<InputState>,
    raw_block: Entity<InputState>,
}

impl Inputs {
    pub fn new(window: &mut Window, cx: &mut Context<ProjectWindow>) -> Inputs {
        let mut one = || cx.new(|cx| InputState::new(window, cx));
        Inputs {
            type_to: one(),
            path: one(),
            key: one(),
            status: one(),
            type_reason: one(),
            field_to: one(),
            field_reason: one(),
            transform: one(),
            block_to: one(),
            block_fields: one(),
            raw_block: one(),
        }
    }

    /// Puts the selected rows' values in the boxes.
    pub fn fill(&self, m: &Mapping, window: &mut Window, cx: &mut Context<ProjectWindow>) {
        let ty = m
            .selected_type
            .as_ref()
            .and_then(|t| m.type_rows().into_iter().find(|r| &r.name == t))
            .unwrap_or_default();
        let field = m
            .selected_type
            .as_ref()
            .zip(m.selected_field.as_ref())
            .and_then(|(t, f)| m.field_rows(t).into_iter().find(|r| &r.path == f))
            .unwrap_or_default();
        let block = m
            .selected_block
            .and_then(|i| m.block_rows().into_iter().nth(i))
            .unwrap_or_default();
        let key = ty.key.split(' ').next().unwrap_or_default().to_string();
        let status = ty.status.replace(" → ", "=");
        for (input, v) in [
            (&self.type_to, ty.to.unwrap_or_default()),
            (&self.path, ty.path.unwrap_or_default()),
            (&self.key, key),
            (&self.status, status),
            (&self.type_reason, ty.drop_reason.unwrap_or_default()),
            (&self.field_to, field.to),
            (&self.field_reason, field.reason.unwrap_or_default()),
            (&self.transform, String::new()),
            (&self.block_to, block.to.unwrap_or_default()),
            (&self.block_fields, block.fields),
            (
                &self.raw_block,
                m.raw_block().unwrap_or_default().to_string(),
            ),
        ] {
            input.update(cx, |s, cx| s.set_value(v, window, cx));
        }
    }
}

fn value(e: &Entity<InputState>, cx: &Context<ProjectWindow>) -> String {
    e.read(cx).value().trim().to_string()
}

fn opt(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

/// Applies `f` to the document and sends it to be saved; a no-op while a save is in flight.
fn edit(
    this: &mut ProjectWindow,
    cx: &mut Context<ProjectWindow>,
    f: impl FnOnce(&mut MappingDoc),
) {
    this.mapping_edit(f);
    cx.notify();
}

pub fn render(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let m = &w.mapping;
    if let Some(e) = m.error() {
        return v_flex()
            .p(space::S4)
            .gap(space::S2)
            .child(error(e, cx))
            .child(reload_button(cx))
            .into_any_element();
    }
    if m.loading() {
        return table::loading("Reading porter.toml…", cx).into_any_element();
    }
    if m.type_rows().is_empty() {
        return v_flex()
            .size_full()
            .justify_center()
            .child(
                table::empty(
                    "Nothing to map yet",
                    "Analyse finds the source types and fields, then seeds [mapping] in \
                     porter.toml with defaults for you to edit here.",
                    cx,
                )
                .child(
                    button("map-analyse", Kind::Primary, "Run analyse", cx).on_click(cx.listener(
                        |this, _, _, cx| this.start(RunPlan::Stage(Stage::Analyse, dry()), cx),
                    )),
                ),
            )
            .into_any_element();
    }
    let t = tokens(cx);
    let (errors, warnings) = m.counts();
    let status = if m.saving() {
        "Saving and checking…"
    } else {
        "Saved to porter.toml as you edit; your comments are kept."
    };
    let problems = h_flex()
        .gap(space::S3)
        .items_center()
        .text_size(text::DENSE)
        .when(errors + warnings == 0, |d| {
            d.child(div().text_color(t.muted).child("No problems"))
        })
        .when(errors > 0, |d| {
            d.child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .text_color(t.err)
                    .child(table::mark(Severity::Error, cx))
                    .child(format!("{errors} error{}", plural(errors))),
            )
        })
        .when(warnings > 0, |d| {
            d.child(
                h_flex()
                    .gap(px(6.))
                    .items_center()
                    .text_color(t.warn)
                    .child(table::mark(Severity::Warning, cx))
                    .child(format!("{warnings} warning{}", plural(warnings))),
            )
        });
    let head = h_flex()
        .w_full()
        .flex_none()
        .gap(space::S3)
        .items_center()
        .px(space::S4)
        .pt(space::S3)
        .child(
            div()
                .text_size(text::HEADING)
                .font_weight(FontWeight::SEMIBOLD)
                .child("Mapping"),
        )
        .child(problems)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(text::DENSE)
                .text_color(t.muted)
                .child(status),
        )
        .children(w.pipeline.load_stale().map(|why| {
            // The mapping is what the dry run was of: say it no longer holds, and why.
            h_flex()
                .flex_none()
                .gap(space::S2)
                .items_center()
                .text_size(text::DENSE)
                .text_color(t.muted)
                .child(Chip::new(ChipState::Stale).large())
                .child(format!("Load dry run: {why}"))
        }))
        .child(
            button("map-next", Kind::Secondary, "Next problem", cx)
                .opacity(if errors + warnings == 0 { 0.5 } else { 1.0 })
                .on_click(cx.listener(|this, _, window, cx| this.mapping_next_problem(window, cx))),
        );
    let mut notes = v_flex().gap(space::S2).px(space::S4);
    if !m.messages().is_empty() {
        let mut box_ = v_flex()
            .gap_1()
            .child(
                div()
                    .text_color(t.err)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Not saved:"),
            )
            .children(m.messages().iter().map(|s| {
                div()
                    .text_size(text::DENSE)
                    .text_color(t.err)
                    .child(s.clone())
            }));
        if m.needs_reload() {
            box_ = box_.child(reload_button(cx));
        }
        notes = notes.child(box_);
    }
    notes = notes.children(blocked(m, cx));
    if !m.general_issues().is_empty() {
        notes = notes
            .child(heading("Schema check"))
            .child(issue_list(m.general_issues(), cx));
    }
    let mut right = v_flex()
        .id("mapping-detail")
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_y_scroll()
        .p(space::S4)
        .gap(space::S4);
    if let Some(ty) = m.selected_type.clone() {
        right = right
            .child(type_editor(w, &ty, cx))
            .child(heading(format!("Fields of {ty}")))
            .child(fields_table(m, &ty, cx));
        if let Some(f) = m.selected_field.clone() {
            right = right.child(field_editor(w, &ty, &f, cx));
        }
    } else {
        right = right.child(table::empty(
            "Select a type",
            "Its fields, and what is wrong with them, show here.",
            cx,
        ));
    }
    right = right.child(heading("Blocks")).child(blocks_table(m, cx));
    if let Some(i) = m.selected_block {
        right = right.child(block_editor(w, i, cx));
    }
    right = right.child(raw_block_editor(w, cx));
    let left = v_flex()
        .id("mapping-types")
        .w(px(300.))
        .flex_none()
        .h_full()
        .overflow_y_scroll()
        .border_r_1()
        .border_color(t.border)
        .child(table::header(
            &[("Source type \u{2192} collection", Col::Flex)],
            cx,
        ))
        .child(types_table(m, cx));
    v_flex()
        .size_full()
        .gap(space::S3)
        .child(head)
        .child(notes)
        .child(h_flex().flex_1().min_h_0().child(left).child(right))
        .into_any_element()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Why Apply cannot go ahead: every error that sits on a row, each a link to its row.
fn blocked(m: &Mapping, cx: &mut Context<ProjectWindow>) -> Option<gpui::Div> {
    let errors: Vec<_> = m.problems().into_iter().filter(|p| p.error).collect();
    // Row-less errors block Apply too; they have no row to jump to.
    let general: Vec<_> = m.general_issues().iter().filter(|i| i.error).collect();
    let total = errors.len() + general.len();
    if total == 0 {
        return None;
    }
    let t = tokens(cx);
    let more = errors.len().saturating_sub(MAX_BLOCKED);
    Some(
        v_flex()
            .gap(px(2.))
            .p(space::S3)
            .rounded(radius::MD)
            .border_1()
            .border_color(t.err)
            .bg(t.err_bg)
            .child(
                h_flex()
                    .gap(space::S2)
                    .items_center()
                    .pb(px(4.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.err)
                    .child(table::mark(Severity::Error, cx))
                    .child(format!(
                        "Apply is blocked: {} mapping error{}",
                        total,
                        plural(total)
                    )),
            )
            .children(
                errors
                    .into_iter()
                    .take(MAX_BLOCKED)
                    .enumerate()
                    .map(|(i, p)| {
                        let to = p.target.clone();
                        h_flex()
                            .id(("blocked", i))
                            .h(ROW_TABLE)
                            .items_center()
                            .gap(space::S3)
                            .text_size(text::DENSE)
                            .cursor_pointer()
                            .hover(|d| d.bg(t.hover))
                            .child(table::cell(("blocked-what", i), p.what, Col::Fixed(180.)))
                            .child(
                                table::cell(("blocked-why", i), p.why, Col::Flex)
                                    .text_color(t.muted),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.mapping_goto(to.clone(), window, cx)
                            }))
                    }),
            )
            .children(general.into_iter().enumerate().map(|(i, g)| {
                h_flex()
                    .h(ROW_TABLE)
                    .items_center()
                    .gap(space::S3)
                    .text_size(text::DENSE)
                    .child(table::cell(
                        ("blocked-gen", i),
                        "Schema check",
                        Col::Fixed(180.),
                    ))
                    .child(
                        table::cell(("blocked-gen-why", i), g.message.clone(), Col::Flex)
                            .text_color(t.muted),
                    )
            }))
            .children((more > 0).then(|| {
                div()
                    .text_size(text::DENSE)
                    .text_color(t.muted)
                    .child(format!("and {more} more. Next problem steps through them."))
            }))
            .child(
                div()
                    .text_size(text::DENSE)
                    .text_color(t.muted)
                    .child("Each line with a row jumps to it; the rest are under Schema check."),
            ),
    )
}

/// Longest list of blocking errors shown at once.
const MAX_BLOCKED: usize = 8;

fn reload_button(cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    Button::new("mapping-reload")
        .label("Reload porter.toml")
        .on_click(cx.listener(|this, _, _, cx| this.mapping_reload(cx)))
}

fn heading(t: impl Into<gpui::SharedString>) -> impl IntoElement {
    div()
        .text_size(text::HEADING)
        .font_weight(FontWeight::SEMIBOLD)
        .child(t.into())
}

/// The reasons under a row: each in its own line, in the row's colours.
fn issue_list(issues: &[RowIssue], cx: &Context<ProjectWindow>) -> gpui::Div {
    v_flex().children(issues.iter().map(|i| {
        table::reason(
            if i.error {
                Severity::Error
            } else {
                Severity::Warning
            },
            i.message.clone(),
            cx,
        )
    }))
}

fn types_table(m: &Mapping, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let t = tokens(cx);
    let mut list = v_flex();
    for (i, r) in m.type_rows().into_iter().enumerate() {
        let (to, to_color) = match (&r.to, &r.drop_reason, r.unmapped) {
            (_, _, true) => ("(unmapped)".to_string(), t.muted),
            (Some(to), _, _) => (to.clone(), t.text),
            (None, _, _) => ("dropped".into(), t.muted),
        };
        // What the row says besides its problems: the path pattern, or why it is dropped.
        let note = match (&r.to, &r.drop_reason, &r.path) {
            (None, Some(why), _) if !r.unmapped => Some(format!("Reason: {why}")),
            (None, None, _) if !r.unmapped => Some("Dropped with no reason recorded".to_string()),
            (Some(_), _, Some(p)) => Some(p.clone()),
            _ => None,
        };
        let selected = m.selected_type.as_deref() == Some(&r.name);
        let name = r.name.clone();
        list = list.child(
            v_flex()
                .child(
                    table::row(("map-type", i), mapping::level(&r.issues), selected, cx)
                        .child(table::cell(("map-type-name", i), r.name.clone(), Col::Flex))
                        .child(div().flex_none().text_color(t.faint).child("\u{2192}"))
                        .child(
                            table::cell(("map-type-to", i), to, Col::Fixed(110.))
                                .text_color(to_color),
                        )
                        .child(
                            div()
                                .flex_none()
                                .w(px(44.))
                                .text_right()
                                .text_color(t.muted)
                                .child(r.count.to_string()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.mapping_select(Some(name.clone()), None, None, window, cx)
                        })),
                )
                .children(note.map(|n| {
                    table::cell(("map-type-note", i), n, Col::Flex)
                        .pl(px(11.))
                        .pb(px(3.))
                        .text_size(text::MICRO)
                        .text_color(t.muted)
                }))
                .child(issue_list(&r.issues, cx)),
        );
    }
    list
}

fn fields_table(m: &Mapping, ty: &str, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let mut t = v_flex().child(table::header(
        &[
            ("Source field", Col::Fixed(170.)),
            ("Sample values", Col::Flex),
            ("Filled", Col::Fixed(48.)),
            ("Target field", Col::Fixed(150.)),
            ("Transforms", Col::Fixed(150.)),
        ],
        cx,
    ));
    let tk = tokens(cx);
    for (i, r) in m.field_rows(ty).into_iter().enumerate() {
        let (to, to_color) = if r.unmapped {
            ("(not mapped)".to_string(), tk.muted)
        } else if r.drop {
            ("(dropped)".to_string(), tk.muted)
        } else {
            (r.to.clone(), tk.text)
        };
        // A dropped field says why it was dropped, right under it.
        let dropped_why = r.drop.then(|| r.reason.clone().unwrap_or_default());
        let transforms = r
            .transforms
            .iter()
            .map(transform_label)
            .collect::<Vec<_>>()
            .join(" \u{2192} ");
        let selected = m.selected_field.as_deref() == Some(&r.path);
        let (ty, path) = (ty.to_string(), r.path.clone());
        t = t.child(
            v_flex()
                .child(
                    table::row(("map-field", i), mapping::level(&r.issues), selected, cx)
                        .child(
                            table::cell(("map-field-path", i), r.path.clone(), Col::Fixed(170.))
                                .when(r.drop, |d| d.line_through().text_color(tk.muted)),
                        )
                        .child(
                            table::cell(("map-field-samples", i), r.samples.clone(), Col::Flex)
                                .text_color(tk.muted),
                        )
                        .child(table::cell(
                            ("map-field-fill", i),
                            r.fill.clone(),
                            Col::Fixed(48.),
                        ))
                        .child(
                            table::cell(("map-field-to", i), to, Col::Fixed(150.))
                                .text_color(to_color),
                        )
                        .child(table::cell(
                            ("map-field-tx", i),
                            transforms,
                            Col::Fixed(150.),
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.mapping_select(
                                Some(ty.clone()),
                                Some(path.clone()),
                                None,
                                window,
                                cx,
                            )
                        })),
                )
                .children(dropped_why.map(|why| {
                    div()
                        .pl(px(11.))
                        .pb(px(3.))
                        .text_size(text::MICRO)
                        .text_color(tk.muted)
                        .child(if why.is_empty() {
                            "Dropped with no reason recorded".to_string()
                        } else {
                            format!("Dropped: {why}")
                        })
                }))
                .child(issue_list(&r.issues, cx)),
        );
    }
    t
}

fn blocks_table(m: &Mapping, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let mut t = v_flex().child(table::header(
        &[
            ("Kind", Col::Fixed(140.)),
            ("Module", Col::Fixed(140.)),
            ("Seen", Col::Fixed(48.)),
            ("Block", Col::Fixed(130.)),
            ("Fields", Col::Flex),
        ],
        cx,
    ));
    let tk = tokens(cx);
    let rows = m.block_rows();
    if rows.is_empty() {
        t = t.child(table::empty(
            "No layout blocks seen yet",
            "Normalise finds them. Run it and they are listed here.",
            cx,
        ));
    }
    for (i, r) in rows.into_iter().enumerate() {
        let selected = m.selected_block == Some(i);
        let ty = m.selected_type.clone();
        let (to, to_color) = match &r.to {
            Some(to) => (to.clone(), tk.text),
            None => ("(raw block)".to_string(), tk.muted),
        };
        t = t.child(
            v_flex()
                .child(
                    table::row(("map-block", i), mapping::level(&r.issues), selected, cx)
                        .child(table::cell(
                            ("map-block-kind", i),
                            r.kind.clone(),
                            Col::Fixed(140.),
                        ))
                        .child(table::cell(
                            ("map-block-mod", i),
                            r.module.clone().unwrap_or_default(),
                            Col::Fixed(140.),
                        ))
                        .child(table::cell(
                            ("map-block-n", i),
                            r.count.to_string(),
                            Col::Fixed(48.),
                        ))
                        .child(
                            table::cell(("map-block-to", i), to, Col::Fixed(130.))
                                .text_color(to_color),
                        )
                        .child(table::cell(
                            ("map-block-fields", i),
                            r.fields.clone(),
                            Col::Flex,
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.mapping_select(ty.clone(), None, Some(i), window, cx)
                        })),
                )
                .child(issue_list(&r.issues, cx)),
        );
    }
    t
}

/// Choices as buttons that set the value at once, or the note why it's free text.
fn picker(
    id: &'static str,
    p: Picker,
    cx: &Context<ProjectWindow>,
    pick: impl Fn(&mut ProjectWindow, &str, &mut Context<ProjectWindow>) + Clone + 'static,
) -> gpui::AnyElement {
    match p {
        Picker::FreeText(why) => div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(why)
            .into_any_element(),
        Picker::Choices(cs) => h_flex()
            .flex_wrap()
            .gap_1()
            .children(cs.into_iter().enumerate().map(|(i, c)| {
                let pick = pick.clone();
                Button::new((id, i))
                    .ghost()
                    .label(c.clone())
                    .on_click(cx.listener(move |this, _, _, cx| pick(this, &c, cx)))
            }))
            .into_any_element(),
    }
}

fn labelled(label: &'static str, input: &Entity<InputState>) -> gpui::Div {
    h_flex()
        .gap_2()
        .items_center()
        .child(div().w_32().text_sm().child(label))
        .child(div().flex_1().child(Input::new(input)))
}

fn card() -> gpui::Div {
    v_flex().gap_2().p_3().border_1().rounded_md()
}

fn type_editor(w: &ProjectWindow, ty: &str, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let inp = &w.mapping_inputs;
    let busy = w.mapping.saving();
    let t = ty.to_string();
    let pick_to = {
        let t = t.clone();
        move |this: &mut ProjectWindow, c: &str, cx: &mut Context<ProjectWindow>| {
            let c = c.to_string();
            edit(this, cx, |d| d.set_type_to(&t, Some(&c)));
        }
    };
    let coll_picker = picker("pick-coll", w.mapping.collection_picker(), cx, pick_to);
    let set = |label: &'static str,
               id: &'static str,
               f: fn(&mut ProjectWindow, &str, &mut Context<ProjectWindow>)| {
        let t = t.clone();
        Button::new(id)
            .label(label)
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| f(this, &t, cx)))
    };
    card()
        .border_color(cx.theme().border)
        .child(div().child(format!("Type {ty}")))
        .child(labelled("To (collection)", &inp.type_to))
        .child(coll_picker)
        .child(labelled("Path pattern", &inp.path))
        .child(labelled("Key field", &inp.key))
        .child(labelled("Status rules", &inp.status))
        .child(labelled("Drop reason", &inp.type_reason))
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(set("Set to", "type-set-to", |this, t, cx| {
                    let v = value(&this.mapping_inputs.type_to, cx);
                    edit(this, cx, |d| d.set_type_to(t, opt(&v)));
                }))
                .child(set("Set path", "type-set-path", |this, t, cx| {
                    let v = value(&this.mapping_inputs.path, cx);
                    edit(this, cx, |d| d.set_path(t, opt(&v)));
                }))
                .child(set("Key: text", "type-key-text", |this, t, cx| {
                    let v = value(&this.mapping_inputs.key, cx);
                    let v = if v.is_empty() { "legacyId".into() } else { v };
                    edit(this, cx, |d| d.set_key(t, &v, KeyType::Text));
                }))
                .child(set("Key: number", "type-key-number", |this, t, cx| {
                    let v = value(&this.mapping_inputs.key, cx);
                    let v = if v.is_empty() { "legacyId".into() } else { v };
                    edit(this, cx, |d| d.set_key(t, &v, KeyType::Number));
                }))
                .child(set("Set status rules", "type-set-status", |this, t, cx| {
                    let v = value(&this.mapping_inputs.status, cx);
                    let rules = match parse_pairs(&v) {
                        Ok(r) => r,
                        Err(e) => return this.mapping_note(e, cx),
                    };
                    let old = this.mapping.status_keys(t);
                    edit(this, cx, |d| {
                        for k in old.iter().filter(|k| !rules.contains_key(*k)) {
                            d.set_status(t, k, None);
                        }
                        for (k, v) in &rules {
                            d.set_status(t, k, Some(v));
                        }
                    });
                }))
                .child(set("Drop type", "type-drop", |this, t, cx| {
                    let v = value(&this.mapping_inputs.type_reason, cx);
                    edit(this, cx, |d| d.drop_type(t, opt(&v)));
                })),
        )
}

fn field_editor(
    w: &ProjectWindow,
    ty: &str,
    field: &str,
    cx: &mut Context<ProjectWindow>,
) -> impl IntoElement {
    let inp = &w.mapping_inputs;
    let busy = w.mapping.saving();
    let row = w
        .mapping
        .field_rows(ty)
        .into_iter()
        .find(|r| r.path == field)
        .unwrap_or_default();
    let (t, f) = (ty.to_string(), field.to_string());
    let pick_to = {
        let (t, f, ts) = (t.clone(), f.clone(), row.transforms.clone());
        move |this: &mut ProjectWindow, c: &str, cx: &mut Context<ProjectWindow>| {
            let e = FieldMapEdit {
                to: c.to_string(),
                transform: ts.clone(),
            };
            edit(this, cx, |d| d.set_field(&t, &f, e));
        }
    };
    let field_picker = picker("pick-field", w.mapping.field_picker(ty, field), cx, pick_to);
    let set = |label: &'static str,
               id: &'static str,
               g: fn(&mut ProjectWindow, &str, &str, &mut Context<ProjectWindow>)| {
        let (t, f) = (t.clone(), f.clone());
        Button::new(id)
            .label(label)
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| g(this, &t, &f, cx)))
    };
    let mut list = v_flex().gap_1();
    for (i, tr) in row.transforms.iter().enumerate() {
        let mv = |label: &'static str, id: &'static str, op: fn(&[_], usize) -> Vec<_>| {
            let (t, f, to, ts) = (t.clone(), f.clone(), row.to.clone(), row.transforms.clone());
            Button::new((id, i))
                .ghost()
                .label(label)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let e = FieldMapEdit {
                        to: to.clone(),
                        transform: op(&ts, i),
                    };
                    edit(this, cx, |d| d.set_field(&t, &f, e));
                }))
        };
        list = list.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .child(format!("{}. {}", i + 1, transform_label(tr))),
                )
                .child(mv("↑", "tr-up", |ts, i| move_transform(ts, i, true)))
                .child(mv("↓", "tr-down", |ts, i| move_transform(ts, i, false)))
                .child(mv("✕", "tr-del", |ts, i| {
                    let mut v = ts.to_vec();
                    v.remove(i);
                    v
                })),
        );
    }
    card()
        .border_color(cx.theme().border)
        .child(div().child(format!("Field {ty}.{field}")))
        .child(labelled("To (field)", &inp.field_to))
        .child(field_picker)
        .child(div().text_sm().child("Transforms, in order:"))
        .child(list)
        .child(labelled("Add transform", &inp.transform))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("e.g. trim, or { op = \"regex\", pattern = \"^x\", replace = \"y\" }"),
        )
        .child(labelled("Drop reason", &inp.field_reason))
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(set("Set to", "field-set-to", |this, t, f, cx| {
                    let to = value(&this.mapping_inputs.field_to, cx);
                    let transform = this.mapping.field_transforms(t, f);
                    edit(this, cx, |d| {
                        d.set_field(t, f, FieldMapEdit { to, transform })
                    });
                }))
                .child(set("Add transform", "field-add-tr", |this, t, f, cx| {
                    let v = value(&this.mapping_inputs.transform, cx);
                    let tr = match parse_transform(&v) {
                        Ok(tr) => tr,
                        Err(e) => return this.mapping_note(e, cx),
                    };
                    let to = value(&this.mapping_inputs.field_to, cx);
                    let mut transform = this.mapping.field_transforms(t, f);
                    transform.push(tr);
                    edit(this, cx, |d| {
                        d.set_field(t, f, FieldMapEdit { to, transform })
                    });
                }))
                .child(set("Drop field", "field-drop", |this, t, f, cx| {
                    let v = value(&this.mapping_inputs.field_reason, cx);
                    edit(this, cx, |d| d.drop_field(t, f, opt(&v)));
                }))
                .child(set("Unmap", "field-unmap", |this, t, f, cx| {
                    edit(this, cx, |d| d.remove_field(t, f));
                })),
        )
}

fn block_editor(w: &ProjectWindow, i: usize, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let Some(row) = w.mapping.block_rows().into_iter().nth(i) else {
        return div().into_any_element();
    };
    let busy = w.mapping.saving();
    let m = row.r#match.clone();
    let pick_to = {
        let (m, fields) = (m.clone(), row.fields.clone());
        move |this: &mut ProjectWindow, c: &str, cx: &mut Context<ProjectWindow>| {
            let e = BlockEdit {
                to: c.to_string(),
                fields: parse_pairs(&fields).unwrap_or_default(),
            };
            edit(this, cx, |d| d.set_block(&m, Some(e)));
        }
    };
    let what = match (&row.r#match.kind, &row.r#match.module) {
        (_, Some(md)) => format!("Blocks from module {md}"),
        (Some(k), None) => format!("Blocks of kind {k}"),
        _ => "Blocks".into(),
    };
    let (m1, m2) = (m.clone(), m);
    card()
        .border_color(cx.theme().border)
        .child(div().child(what))
        .child(labelled("To (block)", &w.mapping_inputs.block_to))
        .child(picker("pick-block", w.mapping.block_picker(), cx, pick_to))
        .child(labelled(
            "Fields (prop=field)",
            &w.mapping_inputs.block_fields,
        ))
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("block-set")
                        .label("Set")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let to = value(&this.mapping_inputs.block_to, cx);
                            let fields =
                                match parse_pairs(&value(&this.mapping_inputs.block_fields, cx)) {
                                    Ok(f) => f,
                                    Err(e) => return this.mapping_note(e, cx),
                                };
                            if to.is_empty() {
                                return this.mapping_note(
                                    "a block map needs a destination block".into(),
                                    cx,
                                );
                            }
                            let m = m1.clone();
                            edit(this, cx, |d| {
                                d.set_block(&m, Some(BlockEdit { to, fields }))
                            });
                        })),
                )
                .child(
                    Button::new("block-remove")
                        .label("Remove (use the raw block)")
                        .disabled(busy || row.to.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let m = m2.clone();
                            edit(this, cx, |d| d.set_block(&m, None));
                        })),
                ),
        )
        .into_any_element()
}

fn raw_block_editor(w: &ProjectWindow, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let busy = w.mapping.saving();
    card()
        .border_color(cx.theme().border)
        .child(
            div().child(
                "Raw block (content with no native shape; its `html` field gets the markup)",
            ),
        )
        .child(labelled("Raw block", &w.mapping_inputs.raw_block))
        .child(picker(
            "pick-raw",
            w.mapping.block_picker(),
            cx,
            |this, c, cx| {
                let c = c.to_string();
                edit(this, cx, |d| d.set_raw_block(Some(&c)));
            },
        ))
        .child(
            Button::new("raw-set")
                .label("Set")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    let v = value(&this.mapping_inputs.raw_block, cx);
                    edit(this, cx, |d| d.set_raw_block(opt(&v)));
                })),
        )
        .child(issue_list(&w.mapping.raw_block_issues(), cx))
}
