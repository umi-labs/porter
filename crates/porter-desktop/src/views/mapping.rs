//! Mapping: the types, fields and blocks tables over porter.toml's `[mapping]`, each row
//! with the schema check's issues under it, and an editor for the selected row. Every
//! change saves at once (comments kept) and the check re-runs in the background.

use gpui::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme, Disableable, Selectable, h_flex, v_flex};
use porter_engine::mapping::KeyType;
use porter_engine::mapping::edit::{BlockEdit, FieldMapEdit, MappingDoc};

use super::overview::error;
use crate::project_window::ProjectWindow;
use crate::vm::mapping::{
    Mapping, Picker, RowIssue, move_transform, parse_pairs, parse_transform, transform_label,
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
    let theme = cx.theme();
    if let Some(e) = m.error() {
        return v_flex()
            .p_4()
            .gap_2()
            .child(error(e, cx))
            .child(reload_button(cx))
            .into_any_element();
    }
    if m.loading() {
        return div()
            .p_4()
            .text_color(theme.muted_foreground)
            .child("Reading porter.toml…")
            .into_any_element();
    }
    let mut page = v_flex()
        .id("mapping")
        .size_full()
        .overflow_y_scroll()
        .p_4()
        .gap_4();
    let status = if m.saving() {
        "Saving and checking…"
    } else {
        "Every change is saved to porter.toml at once; your comments are kept."
    };
    page = page.child(
        div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(status),
    );
    if !m.messages().is_empty() {
        let mut box_ = v_flex()
            .gap_1()
            .child(div().text_color(theme.danger).child("Not saved:"))
            .children(
                m.messages()
                    .iter()
                    .map(|s| div().text_sm().text_color(theme.danger).child(s.clone())),
            );
        if m.needs_reload() {
            box_ = box_.child(reload_button(cx));
        }
        page = page.child(box_);
    }
    if !m.general_issues().is_empty() {
        page = page
            .child(heading("Schema check"))
            .child(issue_list(m.general_issues(), cx));
    }
    page = page.child(heading("Types")).child(types_table(m, cx));
    if let Some(ty) = m.selected_type.clone() {
        page = page
            .child(type_editor(w, &ty, cx))
            .child(heading(format!("Fields of {ty}")))
            .child(fields_table(m, &ty, cx));
        if let Some(f) = m.selected_field.clone() {
            page = page.child(field_editor(w, &ty, &f, cx));
        }
    }
    page = page.child(heading("Blocks")).child(blocks_table(m, cx));
    if let Some(i) = m.selected_block {
        page = page.child(block_editor(w, i, cx));
    }
    page.child(raw_block_editor(w, cx)).into_any_element()
}

fn reload_button(cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    Button::new("mapping-reload")
        .label("Reload porter.toml")
        .on_click(cx.listener(|this, _, _, cx| this.mapping_reload(cx)))
}

fn heading(t: impl Into<gpui::SharedString>) -> impl IntoElement {
    div().text_lg().child(t.into())
}

fn issue_list(issues: &[RowIssue], cx: &Context<ProjectWindow>) -> gpui::Div {
    let theme = cx.theme();
    v_flex().pl_4().children(issues.iter().map(|i| {
        div()
            .text_sm()
            .text_color(if i.error { theme.danger } else { theme.warning })
            .child(format!("{} {}", if i.error { "✕" } else { "!" }, i.message))
    }))
}

fn header(cols: &[(&'static str, bool)], cx: &Context<ProjectWindow>) -> gpui::Div {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .gap_2()
        .px_2()
        .text_sm()
        .text_color(theme.muted_foreground)
        .border_b_1()
        .border_color(theme.border)
        .children(cols.iter().map(|(c, wide)| cell(*c, *wide)))
}

fn cell(t: impl Into<gpui::SharedString>, wide: bool) -> gpui::Div {
    let d = div().min_w_0().overflow_hidden().whitespace_nowrap();
    if wide { d.flex_1() } else { d.w_32() }.child(t.into())
}

fn types_table(m: &Mapping, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let mut t = v_flex().child(header(
        &[
            ("Source type", false),
            ("Docs", false),
            ("To", false),
            ("Path", true),
            ("Key", false),
            ("Status rules", true),
        ],
        cx,
    ));
    for (i, r) in m.type_rows().into_iter().enumerate() {
        let to = match (&r.to, &r.drop_reason, r.unmapped) {
            (_, _, true) => "(unmapped)".to_string(),
            (Some(to), _, _) => to.clone(),
            (None, Some(why), _) => format!("dropped: {why}"),
            (None, None, _) => "dropped".into(),
        };
        let selected = m.selected_type.as_deref() == Some(&r.name);
        let name = r.name.clone();
        t = t.child(
            v_flex()
                .child(
                    Button::new(("map-type", i))
                        .ghost()
                        .w_full()
                        .selected(selected)
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .text_sm()
                                .child(cell(r.name.clone(), false))
                                .child(cell(r.count.to_string(), false))
                                .child(cell(to, false))
                                .child(cell(r.path.clone().unwrap_or_default(), true))
                                .child(cell(r.key.clone(), false))
                                .child(cell(r.status.clone(), true)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.mapping_select(Some(name.clone()), None, None, window, cx)
                        })),
                )
                .child(issue_list(&r.issues, cx)),
        );
    }
    t
}

fn fields_table(m: &Mapping, ty: &str, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let mut t = v_flex().child(header(
        &[
            ("Field", false),
            ("Samples", true),
            ("Filled", false),
            ("To", false),
            ("Transforms", true),
        ],
        cx,
    ));
    for (i, r) in m.field_rows(ty).into_iter().enumerate() {
        let to = if r.unmapped {
            "(unmapped)".to_string()
        } else if r.drop {
            match &r.reason {
                Some(why) => format!("dropped: {why}"),
                None => "dropped".into(),
            }
        } else {
            r.to.clone()
        };
        let transforms = r
            .transforms
            .iter()
            .map(transform_label)
            .collect::<Vec<_>>()
            .join(" → ");
        let selected = m.selected_field.as_deref() == Some(&r.path);
        let (ty, path) = (ty.to_string(), r.path.clone());
        t = t.child(
            v_flex()
                .child(
                    Button::new(("map-field", i))
                        .ghost()
                        .w_full()
                        .selected(selected)
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .text_sm()
                                .child(cell(r.path.clone(), false))
                                .child(cell(r.samples.clone(), true))
                                .child(cell(r.fill.clone(), false))
                                .child(cell(to, false))
                                .child(cell(transforms, true)),
                        )
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
                .child(issue_list(&r.issues, cx)),
        );
    }
    t
}

fn blocks_table(m: &Mapping, cx: &mut Context<ProjectWindow>) -> impl IntoElement {
    let mut t = v_flex().child(header(
        &[
            ("Kind", false),
            ("Module", false),
            ("Seen", false),
            ("To", false),
            ("Fields", true),
        ],
        cx,
    ));
    let rows = m.block_rows();
    if rows.is_empty() {
        t = t.child(
            div()
                .p_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No layout blocks seen yet (run normalise).".to_string()),
        );
    }
    for (i, r) in rows.into_iter().enumerate() {
        let selected = m.selected_block == Some(i);
        let ty = m.selected_type.clone();
        t = t.child(
            v_flex()
                .child(
                    Button::new(("map-block", i))
                        .ghost()
                        .w_full()
                        .selected(selected)
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .text_sm()
                                .child(cell(r.kind.clone(), false))
                                .child(cell(r.module.clone().unwrap_or_default(), false))
                                .child(cell(r.count.to_string(), false))
                                .child(cell(
                                    r.to.clone().unwrap_or_else(|| "(raw block)".into()),
                                    false,
                                ))
                                .child(cell(r.fields.clone(), true)),
                        )
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
