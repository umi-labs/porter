use porter_model::Document;

/// Renders a URL path pattern. Placeholders: `{slug}`, `{id}`, `{type}`, `{yyyy}`, `{mm}`,
/// `{parent.path}`. Date placeholders use `created`, then `updated`.
pub fn render(pattern: &str, doc: &Document, parent_path: Option<&str>) -> Result<String, String> {
    let date = doc.created.or(doc.updated);
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let Some(j) = tail.find('}') else {
            return Err(format!("path pattern \"{pattern}\" has an unclosed '{{'"));
        };
        match &tail[..j] {
            "slug" => out.push_str(&doc.slug),
            "id" => out.push_str(&doc.source.source_id),
            "type" => out.push_str(&doc.doc_type),
            k @ ("yyyy" | "mm") => {
                let d =
                    date.ok_or_else(|| format!("{{{k}}} needs a date but the item has none"))?;
                out.push_str(&d.format(if k == "yyyy" { "%Y" } else { "%m" }).to_string());
            }
            "parent.path" => out.push_str(
                parent_path
                    .ok_or_else(|| "{parent.path} used but the item has no parent".to_string())?,
            ),
            other => return Err(format!("unknown placeholder {{{other}}} in \"{pattern}\"")),
        }
        rest = &tail[j + 1..];
    }
    out.push_str(rest);
    // Collapse runs of slashes; keep a leading slash.
    let mut norm = String::with_capacity(out.len());
    for c in out.chars() {
        if !(c == '/' && norm.ends_with('/')) {
            norm.push(c);
        }
    }
    Ok(norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use porter_model::{DocStatus, ItemId, ItemKind, Seo, SourceRef};

    fn doc(slug: &str) -> Document {
        let src = SourceRef {
            connector: "c".into(),
            source_id: "42".into(),
            url: None,
        };
        Document {
            id: ItemId::new(ItemKind::Document, &src),
            doc_type: "room".into(),
            status: DocStatus::Published,
            locale: None,
            translations: vec![],
            slug: slug.into(),
            path: String::new(),
            parent: None,
            title: String::new(),
            fields: Default::default(),
            seo: Seo::default(),
            created: Some(Utc.with_ymd_and_hms(2024, 3, 5, 0, 0, 0).unwrap()),
            updated: None,
            author: None,
            original: serde_json::Value::Null,
            source: src,
        }
    }

    #[test]
    fn nested_path_pattern() {
        let d = doc("suite");
        assert_eq!(
            render("{parent.path}/{slug}/", &d, Some("/rooms/")).unwrap(),
            "/rooms/suite/"
        );
    }

    #[test]
    fn all_placeholders() {
        let d = doc("a");
        assert_eq!(
            render("/{type}/{yyyy}/{mm}/{id}-{slug}/", &d, None).unwrap(),
            "/room/2024/03/42-a/"
        );
    }

    #[test]
    fn errors() {
        let d = doc("a");
        assert!(render("/{nope}/", &d, None).unwrap_err().contains("nope"));
        assert!(render("{parent.path}/x", &d, None).is_err());
        let mut u = doc("a");
        u.created = None;
        assert!(render("/{yyyy}/", &u, None).is_err());
    }
}
