use indexmap::IndexMap;
use porter_model::FieldValue;
use serde::{Deserialize, Serialize};

/// One step of a field transform; TOML inline table `{ op = "regex", pattern = "..", replace = ".." }`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Transform {
    Trim,
    Lower,
    Upper,
    Slugify,
    DateParse { format: String },
    DateFormat { format: String },
    Split { sep: String },
    Join { sep: String },
    Number,
    UrlRewrite { from: String, to: String },
    Regex { pattern: String, replace: String },
    Default { value: String },
    Lookup { table: IndexMap<String, String> },
}

impl Transform {
    fn name(&self) -> &'static str {
        match self {
            Transform::Trim => "trim",
            Transform::Lower => "lower",
            Transform::Upper => "upper",
            Transform::Slugify => "slugify",
            Transform::DateParse { .. } => "date-parse",
            Transform::DateFormat { .. } => "date-format",
            Transform::Split { .. } => "split",
            Transform::Join { .. } => "join",
            Transform::Number => "number",
            Transform::UrlRewrite { .. } => "url-rewrite",
            Transform::Regex { .. } => "regex",
            Transform::Default { .. } => "default",
            Transform::Lookup { .. } => "lookup",
        }
    }
}

pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn text_of(v: &FieldValue) -> Option<String> {
    match v {
        FieldValue::Text(s) => Some(s.clone()),
        FieldValue::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Applies the transforms in order. Errors name the transform and the offending value.
pub fn apply(value: &FieldValue, transforms: &[Transform]) -> Result<FieldValue, String> {
    let mut v = value.clone();
    for t in transforms {
        v = step(&v, t)?;
    }
    Ok(v)
}

fn describe(v: &FieldValue) -> String {
    match v {
        FieldValue::Text(s) => format!("\"{s}\""),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn step(v: &FieldValue, t: &Transform) -> Result<FieldValue, String> {
    let fail = |why: &str| format!("{}: {why} (value {})", t.name(), describe(v));
    match (t, v) {
        (Transform::Default { value }, FieldValue::Null) => {
            return Ok(FieldValue::Text(value.clone()));
        }
        (Transform::Default { value }, FieldValue::Text(s)) if s.trim().is_empty() => {
            return Ok(FieldValue::Text(value.clone()));
        }
        (Transform::Default { .. }, _) => return Ok(v.clone()),
        (_, FieldValue::Null) => return Ok(FieldValue::Null),
        (Transform::Join { sep }, FieldValue::List(items)) => {
            let parts: Option<Vec<String>> = items.iter().map(text_of).collect();
            return parts
                .map(|p| FieldValue::Text(p.join(sep)))
                .ok_or_else(|| fail("list holds non-text items"));
        }
        (Transform::Join { .. }, _) => return Err(fail("expected a list")),
        (Transform::DateFormat { format }, FieldValue::Date(d)) => {
            return Ok(FieldValue::Text(d.format(format).to_string()));
        }
        (Transform::DateFormat { .. }, _) => return Err(fail("expected a date")),
        (Transform::Number, FieldValue::Number(_)) => return Ok(v.clone()),
        // Everything else works on text, element-wise over lists.
        (_, FieldValue::List(items)) if !matches!(t, Transform::Split { .. }) => {
            return items
                .iter()
                .map(|i| step(i, t))
                .collect::<Result<_, _>>()
                .map(FieldValue::List);
        }
        _ => {}
    }
    let s = text_of(v).ok_or_else(|| fail("expected text"))?;
    Ok(match t {
        Transform::Trim => FieldValue::Text(s.trim().to_string()),
        Transform::Lower => FieldValue::Text(s.to_lowercase()),
        Transform::Upper => FieldValue::Text(s.to_uppercase()),
        Transform::Slugify => FieldValue::Text(slugify(&s)),
        Transform::DateParse { format } => FieldValue::Date(
            parse_date(&s, format)
                .ok_or_else(|| fail(&format!("does not match format \"{format}\"")))?,
        ),
        Transform::Split { sep } => FieldValue::List(
            s.split(sep.as_str())
                .map(|p| FieldValue::Text(p.trim().to_string()))
                .filter(|p| *p != FieldValue::Text(String::new()))
                .collect(),
        ),
        Transform::Number => {
            FieldValue::Number(s.trim().parse().map_err(|_| fail("not a number"))?)
        }
        Transform::UrlRewrite { from, to } => {
            FieldValue::Text(match s.strip_prefix(from.as_str()) {
                Some(rest) => format!("{to}{rest}"),
                None => s,
            })
        }
        Transform::Regex { pattern, replace } => {
            let re = regex::Regex::new(pattern)
                .map_err(|e| format!("regex: bad pattern \"{pattern}\": {e}"))?;
            FieldValue::Text(re.replace_all(&s, replace.as_str()).into_owned())
        }
        Transform::Lookup { table } => FieldValue::Text(
            table
                .get(&s)
                .cloned()
                .ok_or_else(|| fail("no entry in lookup table"))?,
        ),
        Transform::Default { .. } | Transform::Join { .. } | Transform::DateFormat { .. } => {
            unreachable!("handled above")
        }
    })
}

fn parse_date(s: &str, format: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
    let s = s.trim();
    if let Ok(d) = DateTime::parse_from_str(s, format) {
        return Some(d.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDateTime::parse_from_str(s, format) {
        return Some(d.and_utc());
    }
    NaiveDate::parse_from_str(s, format)
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> FieldValue {
        FieldValue::Text(s.into())
    }

    #[test]
    fn transforms_chain() {
        let out = apply(
            &t("  Hello World "),
            &[Transform::Trim, Transform::Lower, Transform::Slugify],
        )
        .unwrap();
        assert_eq!(out, t("hello-world"));
    }

    #[test]
    fn lookup_missing_errors() {
        let mut table = IndexMap::new();
        table.insert("a".to_string(), "b".to_string());
        let e = apply(&t("zzz"), &[Transform::Lookup { table }]).unwrap_err();
        assert!(e.contains("zzz") && e.contains("lookup"), "{e}");
    }

    #[test]
    fn misc_transforms() {
        assert_eq!(
            apply(&t("a, b,,c"), &[Transform::Split { sep: ",".into() }]).unwrap(),
            FieldValue::List(vec![t("a"), t("b"), t("c")])
        );
        assert_eq!(
            apply(
                &apply(&t("a|b"), &[Transform::Split { sep: "|".into() }]).unwrap(),
                &[Transform::Join { sep: "-".into() }]
            )
            .unwrap(),
            t("a-b")
        );
        assert_eq!(
            apply(&t(" 3.5 "), &[Transform::Number]).unwrap(),
            FieldValue::Number(3.5)
        );
        assert!(
            apply(&t("x"), &[Transform::Number])
                .unwrap_err()
                .contains("\"x\"")
        );
        assert_eq!(
            apply(
                &t("https://old.test/a/b"),
                &[Transform::UrlRewrite {
                    from: "https://old.test".into(),
                    to: "/new".into()
                }]
            )
            .unwrap(),
            t("/new/a/b")
        );
        assert_eq!(
            apply(
                &t("a1b22"),
                &[Transform::Regex {
                    pattern: r"\d+".into(),
                    replace: "#".into()
                }]
            )
            .unwrap(),
            t("a#b#")
        );
        assert_eq!(
            apply(
                &FieldValue::Null,
                &[Transform::Default { value: "x".into() }]
            )
            .unwrap(),
            t("x")
        );
        let d = apply(
            &t("05/03/2024"),
            &[Transform::DateParse {
                format: "%d/%m/%Y".into(),
            }],
        )
        .unwrap();
        assert_eq!(
            apply(
                &d,
                &[Transform::DateFormat {
                    format: "%Y-%m-%d".into()
                }]
            )
            .unwrap(),
            t("2024-03-05")
        );
        assert!(
            apply(
                &t("nope"),
                &[Transform::DateParse {
                    format: "%Y".into()
                }]
            )
            .is_err()
        );
    }
}
