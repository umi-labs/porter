//! Gravity Forms and Ninja Forms definitions and entries in the neutral form model. Field
//! names come from the source's own keys or labels; merge tags that name fields become
//! `{{name}}`; everything else in a notification (addresses, `{admin_email}`…) is kept as the
//! site had it.

use super::load::Raw;
use super::normalise::{Out, canon};
use crate::html::{HtmlCtx, decode_entities, html_to_richtext};
use crate::mapping::transforms::slugify;
use anyhow::Result;
use chrono::NaiveDateTime;
use indexmap::IndexMap;
use porter_model::{
    Form, FormField, IssueKind, ItemId, ItemKind, ItemStatus, Severity, Submission,
};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// A form plus how its stored answers map onto field names.
pub(crate) struct Mapped {
    pub form: Form,
    /// Source input id (Gravity `"3"`, `"3.6"`; Ninja field id) → field name.
    pub inputs: HashMap<String, String>,
}

fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(x) => matches!(x.as_str(), "1" | "true" | "on" | "yes"),
        _ => false,
    }
}

static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("valid regex"));

/// Label HTML as plain text.
fn plain(html: &str) -> String {
    decode_entities(&TAGS.replace_all(html, " "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A unique field name from `base` (falls back to `field-<n>`).
fn name_for(base: &str, taken: &mut HashSet<String>) -> String {
    let base = slugify(base).unwrap_or_else(|_| format!("field-{}", taken.len() + 1));
    let mut name = base.clone();
    let mut n = 2;
    while !taken.insert(name.clone()) {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

fn field(name: &str, label: &str, kind: &str, required: bool) -> FormField {
    FormField {
        name: name.into(),
        label: label.into(),
        kind: kind.into(),
        required,
        options: vec![],
        conditions: Value::Null,
    }
}

fn arr(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => o.values().collect(),
        _ => vec![],
    }
}

/// Address-like option values route a notification: `[{field, answer, send_to}]`.
fn routing(to: &str, fields: &[FormField]) -> Vec<Value> {
    static REF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\{\{([\w-]+)\}\}").expect("valid regex"));
    let mut out = vec![];
    for c in REF.captures_iter(to) {
        if let Some(f) = fields.iter().find(|f| f.name == c[1]) {
            for (value, label) in &f.options {
                if value.contains('@') && !value.contains(char::is_whitespace) {
                    out.push(json!({ "field": f.name, "answer": label, "send_to": value }));
                }
            }
        }
    }
    out
}

/// A field's `conditionalLogic`; Gravity stores `""` (or nothing) when there is none.
fn logic(x: &Value) -> Value {
    match &x["conditionalLogic"] {
        v @ Value::Object(_) => v.clone(),
        _ => Value::Null,
    }
}

/// Gravity Forms: fields, conditional logic, active notifications, default confirmation.
pub(crate) fn gravity(f: &Value, html: &mut HtmlCtx) -> Mapped {
    let mut taken = HashSet::new();
    let mut fields = vec![];
    let mut inputs = HashMap::new();
    let mut by_id: HashMap<String, String> = HashMap::new();
    for x in arr(&f["fields"]) {
        let ty = s(x, "type");
        let label = plain(&s(x, "label"));
        let id = s(x, "id");
        let required = truthy(&x["isRequired"]);
        let shown: Vec<&Value> = arr(&x["inputs"])
            .into_iter()
            .filter(|i| !truthy(&i["isHidden"]))
            .collect();
        let options: Vec<(String, String)> = arr(&x["choices"])
            .into_iter()
            .map(|c| {
                let text = plain(&s(c, "text"));
                let value = match &c["value"] {
                    Value::Null => text.clone(),
                    _ => s(c, "value"),
                };
                (value, text)
            })
            .collect();
        let kind = match ty.as_str() {
            "captcha" | "page" => continue,
            "section" | "html" => {
                let words = plain(&(s(x, "content") + " " + &label));
                let name = name_for(&format!("message-{id}"), &mut taken);
                let mut m = field(&name, &words, "message", false);
                m.conditions = logic(x);
                fields.push(m);
                continue;
            }
            "name" if !shown.is_empty() => {
                for i in &shown {
                    let sub = format!("{label} {}", plain(&s(i, "label")));
                    let name = name_for(&sub, &mut taken);
                    inputs.insert(s(i, "id"), name.clone());
                    fields.push(field(&name, &sub, "text", required));
                }
                continue;
            }
            "address" => "textarea",
            "email" | "number" | "textarea" | "date" | "select" | "radio" | "hidden" => ty.as_str(),
            "phone" => "tel",
            "checkbox" | "multiselect" => "checkboxes",
            "consent" => "checkbox",
            "fileupload" => "file",
            _ => "text",
        };
        let name = name_for(&label, &mut taken);
        by_id.insert(id.clone(), name.clone());
        inputs.insert(id.clone(), name.clone());
        for i in arr(&x["inputs"]) {
            inputs.insert(s(i, "id"), name.clone());
        }
        let mut fd = field(&name, &label, kind, required);
        fd.options = options;
        fd.conditions = logic(x);
        fields.push(fd);
    }
    // Conditions refer to fields by id; point them at names.
    for fd in &mut fields {
        if let Some(rules) = fd.conditions.get_mut("rules").and_then(Value::as_array_mut) {
            for r in rules {
                let id = s(r, "fieldId");
                let target = id.split('.').next().unwrap_or("").to_string();
                if let Some(n) = by_id.get(&target)
                    && let Some(o) = r.as_object_mut()
                {
                    o.insert("field".into(), json!(n));
                }
            }
        }
    }
    static MERGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\{[^{}:]*:(\d+(?:\.\d+)?)(?::[^{}]*)?\}").expect("valid regex")
    });
    let swap = |t: &str| -> String {
        let t = MERGE.replace_all(t, |c: &regex::Captures| {
            let id = &c[1];
            let base = id.split('.').next().unwrap_or(id);
            inputs
                .get(id)
                .or_else(|| inputs.get(base))
                .map_or_else(|| c[0].to_string(), |n| format!("{{{{{n}}}}}"))
        });
        t.replace("{all_fields}", "{{all_fields}}")
    };
    let mut notifications = vec![];
    for n in arr(&f["notifications"]) {
        if n["isActive"] == false {
            continue;
        }
        let to = swap(&s(n, "to"));
        let mut out = json!({
            "name": s(n, "name"), "event": s(n, "event"),
            "to": to, "to_type": s(n, "toType"),
            "cc": swap(&s(n, "cc")), "bcc": swap(&s(n, "bcc")), "reply_to": swap(&s(n, "replyTo")),
            "from": swap(&s(n, "from")), "from_name": swap(&s(n, "fromName")),
            "subject": swap(&s(n, "subject")), "message": swap(&s(n, "message")),
        });
        // Gravity's own routing rules, fields by name.
        if s(n, "toType") == "routing" {
            out["routing"] = json!(
                arr(&n["routing"])
                    .into_iter()
                    .map(|r| json!({
                        "field": by_id.get(&s(r, "fieldId")).cloned().unwrap_or_default(),
                        "operator": s(r, "operator"), "answer": s(r, "value"), "send_to": s(r, "email"),
                    }))
                    .collect::<Vec<_>>()
            );
        } else {
            out["routing"] = json!(routing(&to, &fields));
        }
        notifications.push(out);
    }
    let confs = arr(&f["confirmations"]);
    let conf = confs
        .iter()
        .find(|c| truthy(&c["isDefault"]))
        .or_else(|| confs.first());
    let conf_html = match conf {
        Some(c)
            if matches!(s(c, "type").as_str(), "redirect" | "page") && !s(c, "url").is_empty() =>
        {
            let u = s(c, "url");
            format!("<p><a href=\"{u}\">{u}</a></p>")
        }
        Some(c) => swap(&s(c, "message")),
        None => String::new(),
    };
    let form = Form {
        title: plain(&s(f, "title")),
        fields,
        notifications,
        confirmation: html_to_richtext(&conf_html, html),
        retired: truthy(&f["is_trash"]) || f["is_active"] == false || f["is_active"] == "0",
    };
    Mapped { form, inputs }
}

/// Ninja Forms: fields in order, `show_field` conditions, active email actions, success message.
pub(crate) fn ninja(f: &Value, html: &mut HtmlCtx) -> Mapped {
    static STAMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_\d{9,}$").expect("valid regex"));
    let settings = &f["settings"];
    let mut sorted: Vec<&Value> = arr(&f["fields"]);
    let order = |x: &Value| {
        let x = if x["settings"].is_object() {
            &x["settings"]
        } else {
            x
        };
        x["order"]
            .as_f64()
            .or_else(|| s(x, "order").parse().ok())
            .unwrap_or(0.0)
    };
    sorted.sort_by(|a, b| order(a).total_cmp(&order(b)));
    let mut taken = HashSet::new();
    let mut fields: Vec<FormField> = vec![];
    let mut inputs = HashMap::new();
    let mut by_key: HashMap<String, String> = HashMap::new();
    for raw in sorted {
        let x = if raw["settings"].is_object() {
            &raw["settings"]
        } else {
            raw
        };
        let id = match &raw["id"] {
            Value::Null => s(x, "id"),
            _ => s(raw, "id"),
        };
        let key = s(x, "key");
        let label = plain(&s(x, "label"));
        let kind = match s(x, "type").as_str() {
            "submit" | "recaptcha" | "spam" | "hr" => continue,
            "html" => "message",
            "email" => "email",
            "textarea" => "textarea",
            "number" | "quantity" => "number",
            "date" => "date",
            "checkbox" => "checkbox",
            "listselect" | "listcountry" | "liststate" => "select",
            "listradio" => "radio",
            "listcheckbox" | "listmultiselect" => "checkboxes",
            "phone" => "tel",
            "hidden" => "hidden",
            _ => "text",
        };
        let stripped = STAMP.replace(&key, "").to_string();
        let base = if !stripped.is_empty() && stripped.len() <= 30 {
            stripped
        } else {
            label.clone()
        };
        let name = name_for(&base, &mut taken);
        let label = if kind == "message" {
            plain(&s(x, "default"))
        } else {
            label
        };
        by_key.insert(key, name.clone());
        inputs.insert(id, name.clone());
        let mut fd = field(&name, &label, kind, truthy(&x["required"]));
        fd.options = arr(&x["options"])
            .into_iter()
            .map(|o| (s(o, "value").trim().to_string(), plain(&s(o, "label"))))
            .collect();
        fields.push(fd);
    }
    // Conditions: each `then` target gets the `when` list, keys swapped for names.
    for c in arr(&settings["conditions"]) {
        let when: Vec<Value> = arr(&c["when"])
            .into_iter()
            .map(|w| {
                let k = s(w, "key");
                json!({
                    "field": by_key.get(&k).cloned().unwrap_or(k),
                    "comparator": s(w, "comparator"), "value": s(w, "value"),
                })
            })
            .collect();
        for t in arr(&c["then"]) {
            let Some(name) = by_key.get(&s(t, "key")) else {
                continue;
            };
            if let Some(fd) = fields.iter_mut().find(|f| &f.name == name) {
                let rule = json!({ "when": when, "trigger": s(t, "trigger") });
                match &mut fd.conditions {
                    Value::Array(a) => a.push(rule),
                    other => *other = json!([rule]),
                }
            }
        }
    }
    static FIELD_TAG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\{field:([\w-]+)\}").expect("valid regex"));
    let swap = |t: &str| -> String {
        FIELD_TAG
            .replace_all(t, |c: &regex::Captures| {
                by_key
                    .get(&c[1])
                    .map_or_else(|| c[0].to_string(), |n| format!("{{{{{n}}}}}"))
            })
            .replace("{field:all_fields}", "{{all_fields}}")
    };
    let actions: Vec<&Value> = arr(&f["actions"])
        .into_iter()
        .map(|a| {
            if a["settings"].is_object() {
                &a["settings"]
            } else {
                a
            }
        })
        .filter(|a| truthy(&a["active"]))
        .collect();
    let mut notifications = vec![];
    for a in actions.iter().filter(|a| s(a, "type") == "email") {
        let to = swap(&s(a, "to"));
        notifications.push(json!({
            "name": s(a, "label"),
            "to": to, "cc": swap(&s(a, "cc")), "bcc": swap(&s(a, "bcc")),
            "reply_to": swap(&s(a, "reply_to")),
            "from": swap(&s(a, "from_address")), "from_name": swap(&s(a, "from_name")),
            "subject": swap(&s(a, "email_subject")), "message": swap(&s(a, "email_message")),
            "routing": routing(&to, &fields),
        }));
    }
    let redirect = actions
        .iter()
        .find(|a| s(a, "type") == "redirect" && !s(a, "redirect_url").trim().is_empty());
    let conf_html = match redirect {
        Some(r) => {
            let u = s(r, "redirect_url");
            format!("<p><a href=\"{u}\">{u}</a></p>")
        }
        None => actions
            .iter()
            .find(|a| s(a, "type") == "successmessage")
            .map(|a| swap(&s(a, "success_msg")))
            .unwrap_or_default(),
    };
    let title = plain(&s(settings, "title"));
    let form = Form {
        title: if title.is_empty() {
            format!("Form {}", s(f, "id"))
        } else {
            title
        },
        fields,
        notifications,
        confirmation: html_to_richtext(&conf_html, html),
        retired: false,
    };
    Mapped { form, inputs }
}

/// One stored entry (`{source, id, form_id, date, fields}` from the wp-db connector) with
/// answers keyed by field name; answers the form doesn't know keep their source key.
pub(crate) fn submission(v: &Value, form: ItemId, inputs: &HashMap<String, String>) -> Submission {
    let created = NaiveDateTime::parse_from_str(&s(v, "date"), "%Y-%m-%d %H:%M:%S")
        .map(|d| d.and_utc())
        .unwrap_or_default();
    let mut values: IndexMap<String, String> = IndexMap::new();
    if let Value::Object(fields) = &v["fields"] {
        for (k, x) in fields {
            let id = k.strip_prefix("_field_").unwrap_or(k);
            let name = inputs
                .get(id)
                .or_else(|| inputs.get(id.split('.').next().unwrap_or(id)))
                .cloned()
                .unwrap_or_else(|| k.clone());
            let text = match x {
                Value::String(t) => t.clone(),
                Value::Array(a) => a
                    .iter()
                    .map(|i| i.as_str().map_or_else(|| i.to_string(), str::to_string))
                    .collect::<Vec<_>>()
                    .join(", "),
                Value::Null => continue,
                other => other.to_string(),
            };
            if text.is_empty() {
                continue;
            }
            // Multi-input fields (checkboxes, address parts) share one name.
            match values.get_mut(&name) {
                Some(prev) => {
                    prev.push_str(", ");
                    prev.push_str(&text);
                }
                None => {
                    values.insert(name, text);
                }
            }
        }
    }
    Submission {
        form,
        created,
        values,
    }
}

/// Writes every form, then every entry. False when cancelled.
pub(crate) fn write(out: &mut Out, raw: &Raw) -> Result<bool> {
    let id_text = |x: &Value| {
        x.as_u64()
            .map_or_else(|| x.as_str().unwrap_or("").to_string(), |n| n.to_string())
    };
    let mut inputs: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (kind, list) in [("gf", &raw.gf), ("nf", &raw.nf)] {
        for (v, rows) in list {
            let fid = id_text(&v["id"]);
            let c = canon(ItemKind::Form, &format!("form:{kind}:{fid}"), None);
            let mut issues = vec![];
            let mut html = HtmlCtx {
                store: out.store,
                site_hosts: raw.site.hosts.clone(),
                issues: &mut issues,
                media_alternates: super::media::alternates,
            };
            let mapped = if kind == "gf" {
                gravity(v, &mut html)
            } else {
                ninja(v, &mut html)
            };
            out.put(ItemKind::Form, &c, &mapped.form)?;
            out.pending(c.0, issues)?;
            out.status(ItemKind::Form, c.0, ItemStatus::Done, rows)?;
            inputs.insert(format!("{kind}:{fid}"), mapped.inputs);
        }
    }
    let none = HashMap::new();
    for (v, row) in &raw.submissions {
        if out.ctx.cancel.is_cancelled() {
            return Ok(false);
        }
        let kind = v["source"].as_str().unwrap_or("gf");
        let (sid, fid) = (id_text(&v["id"]), id_text(&v["form_id"]));
        let c = canon(
            ItemKind::Submission,
            &format!("submission:{kind}:{sid}"),
            None,
        );
        let form = canon(ItemKind::Form, &format!("form:{kind}:{fid}"), None).0;
        let known = inputs.get(&format!("{kind}:{fid}"));
        out.put(
            ItemKind::Submission,
            &c,
            &submission(v, form, known.unwrap_or(&none)),
        )?;
        let st = if known.is_some() {
            ItemStatus::Done
        } else {
            out.issue(
                IssueKind::UnresolvedRelation,
                Severity::Warning,
                c.0,
                format!("entry {sid} belongs to form {kind}:{fid}, which was not found"),
                json!({ "form": format!("{kind}:{fid}") }),
            )?;
            ItemStatus::Partial {
                reason: "form definition missing; answers keyed by source field id".into(),
            }
        };
        out.status(ItemKind::Submission, c.0, st, row)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::tests::{ctx, fixture};

    #[test]
    fn gravity_and_ninja_forms() {
        let f = fixture();
        let mut issues = vec![];
        let gf = json!({
            "id": 3, "title": "Contact", "is_active": "1",
            "fields": [
                {"id": 1, "type": "name", "label": "Name", "isRequired": true,
                 "inputs": [{"id": "1.3", "label": "First"}, {"id": "1.6", "label": "Last"}, {"id": "1.2", "label": "Prefix", "isHidden": true}]},
                {"id": 2, "type": "email", "label": "Email", "conditionalLogic": ""},
                {"id": 4, "type": "select", "label": "Team", "choices": [{"text": "Sales", "value": "sales@example.test"}]},
                {"id": 5, "type": "checkbox", "label": "Topics", "choices": [{"text": "A", "value": "a"}],
                 "inputs": [{"id": "5.1"}, {"id": "5.2"}],
                 "conditionalLogic": {"actionType": "show", "logicType": "all", "rules": [{"fieldId": "2", "operator": "is", "value": "x"}]}},
            ],
            "notifications": {"n1": {"isActive": true, "to": "{Team:4}", "subject": "Hi {Email:2}", "message": "{all_fields}"},
                              "n2": {"isActive": false, "to": "x"}},
            "confirmations": {"c1": {"isDefault": true, "type": "message", "message": "<p>Thanks</p>"}},
        });
        let m = gravity(&gf, &mut ctx(&f, &mut issues));
        let names: Vec<&str> = m.form.fields.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            ["name-first", "name-last", "email", "team", "topics"]
        );
        assert_eq!(m.form.fields[4].conditions["rules"][0]["field"], "email");
        // Gravity's "" for "no logic" is no conditions, not a panic.
        assert_eq!(m.form.fields[2].conditions, Value::Null);
        assert_eq!(m.form.notifications.len(), 1);
        assert_eq!(m.form.notifications[0]["to"], "{{team}}");
        assert_eq!(m.form.notifications[0]["subject"], "Hi {{email}}");
        assert_eq!(
            m.form.notifications[0]["routing"][0]["send_to"],
            "sales@example.test"
        );
        assert_eq!(m.form.confirmation.plain_text(), "Thanks");
        let sub = submission(
            &json!({"date": "2024-01-02 03:04:05", "fields": {"1.3": "Ann", "5.1": "a", "5.2": "b", "9": "?"}}),
            ItemId(uuid::Uuid::nil()),
            &m.inputs,
        );
        assert_eq!(sub.values["name-first"], "Ann");
        assert_eq!(sub.values["topics"], "a, b");
        assert_eq!(sub.values["9"], "?");

        let nf = json!({
            "id": 2, "settings": {"title": "Enquiry", "conditions": [
                {"when": [{"key": "want_1234567890", "comparator": "equal", "value": "1"}], "then": [{"key": "detail", "trigger": "show_field"}]},
            ]},
            "fields": [
                {"id": 11, "key": "detail", "type": "textarea", "label": "Detail", "order": 2},
                {"id": 10, "key": "want_1234567890", "type": "checkbox", "label": "Want?", "order": 1, "required": "1"},
                {"id": 12, "key": "submit", "type": "submit", "label": "Send", "order": 3},
            ],
            "actions": [
                {"id": 1, "type": "email", "active": "1", "to": "{system:admin_email}", "email_subject": "From {field:detail}"},
                {"id": 2, "type": "successmessage", "active": "1", "success_msg": "<p>Done</p>"},
            ],
        });
        let m = ninja(&nf, &mut ctx(&f, &mut issues));
        let names: Vec<&str> = m.form.fields.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["want", "detail"]);
        assert!(m.form.fields[0].required);
        assert_eq!(m.form.fields[1].conditions[0]["when"][0]["field"], "want");
        assert_eq!(m.form.notifications[0]["to"], "{system:admin_email}");
        assert_eq!(m.form.notifications[0]["subject"], "From {{detail}}");
        assert_eq!(m.form.confirmation.plain_text(), "Done");
        assert_eq!(m.inputs["11"], "detail");
    }
}
