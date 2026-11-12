//! WordPress shortcode string -> tree. Text between shortcodes is a node with tag `""`.

use indexmap::IndexMap;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScNode {
    pub tag: String,
    pub attrs: IndexMap<String, String>,
    pub children: Vec<ScNode>,
    pub text: String,
}

// Attrs run to the first "]" like WordPress's own regex (Divi escapes "]" in values as %93).
static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(/)?([a-zA-Z0-9_-]+)([^\]]*)\]").expect("valid regex"));
// Double-, single- or un-quoted, as WordPress accepts.
static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([\w-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s'"]+))"#).expect("valid regex")
});

/// Shortcodes most WordPress sites register. Like WordPress, only known tags parse;
/// anything else in brackets (e.g. "[key]" in prose) stays text.
pub fn default_known() -> Regex {
    Regex::new(
        r"^(et_pb_\w+|gravityforms?|ninja_form|caption|gallery|embed|audio|video|playlist|smartslider3)$",
    )
    .expect("valid regex")
}

fn node(tag: &str) -> ScNode {
    ScNode {
        tag: tag.to_string(),
        ..ScNode::default()
    }
}

fn push_text(parent: &mut ScNode, text: &str) {
    if text.is_empty() {
        return;
    }
    // Merge with the previous text node (an unknown "[tag]" split it).
    match parent.children.last_mut() {
        Some(prev) if prev.tag.is_empty() => prev.text.push_str(text),
        _ => parent.children.push(ScNode {
            text: text.to_string(),
            ..ScNode::default()
        }),
    }
}

/// Pops the top of the stack into its parent's children.
fn close_top(stack: &mut Vec<ScNode>) {
    if stack.len() > 1
        && let Some(n) = stack.pop()
        && let Some(parent) = stack.last_mut()
    {
        parent.children.push(n);
    }
}

pub fn parse_shortcodes(src: &str, known: &Regex) -> Vec<ScNode> {
    // Start of the last `[/tag]` per tag, so "a closer follows" is O(1).
    let mut last_close: HashMap<&str, usize> = HashMap::new();
    for c in TAG.captures_iter(src) {
        if c.get(1).is_some() && c[3].is_empty() {
            last_close.insert(
                c.get(2).map_or("", |m| m.as_str()),
                c.get(0).map_or(0, |m| m.start()),
            );
        }
    }

    let mut stack = vec![node("#root")];
    let mut last = 0;
    for c in TAG.captures_iter(src) {
        let (Some(whole), Some(tag)) = (c.get(0), c.get(2)) else {
            continue;
        };
        let tag = tag.as_str();
        if !known.is_match(tag) {
            continue;
        }
        if let Some(top) = stack.last_mut() {
            push_text(top, &src[last..whole.start()]);
        }
        last = whole.end();
        let raw_attrs = &c[3];

        if c.get(1).is_some() {
            // Pop back to the matching opener; unmatched closers are ignored.
            if let Some(at) = stack
                .iter()
                .rposition(|n| n.tag == tag)
                .filter(|&at| at > 0)
            {
                while stack.len() > at {
                    close_top(&mut stack);
                }
            }
            continue;
        }
        let mut n = node(tag);
        for a in ATTR.captures_iter(raw_attrs) {
            let v = a
                .get(2)
                .or(a.get(3))
                .or(a.get(4))
                .map_or("", |m| m.as_str());
            n.attrs.insert(a[1].to_string(), v.to_string());
        }
        // Enclosing only if not slash-closed and a closer follows (WordPress's rule).
        let enclosing = !raw_attrs.trim_end().ends_with('/')
            && last_close.get(tag).is_some_and(|&at| at >= last);
        if enclosing {
            stack.push(n);
        } else if let Some(top) = stack.last_mut() {
            top.children.push(n);
        }
    }
    if let Some(top) = stack.last_mut() {
        push_text(top, &src[last..]);
    }
    while stack.len() > 1 {
        close_top(&mut stack);
    }
    stack.pop().map(|r| r.children).unwrap_or_default()
}

/// Shortcode source for a node, for keeping what a converter can't map.
pub fn to_source(n: &ScNode) -> String {
    if n.tag.is_empty() {
        return n.text.clone();
    }
    let mut s = format!("[{}", n.tag);
    for (k, v) in &n.attrs {
        if v.contains('"') {
            s.push_str(&format!(" {k}='{v}'"));
        } else {
            s.push_str(&format!(" {k}=\"{v}\""));
        }
    }
    if n.children.is_empty() {
        s.push_str(" /]");
    } else {
        s.push(']');
        for c in &n.children {
            s.push_str(&to_source(c));
        }
        s.push_str(&format!("[/{}]", n.tag));
    }
    s
}

/// `[caption]<img ...> Caption text[/caption]` as the `<figure>` WordPress renders.
pub fn caption_html(n: &ScNode) -> String {
    let html: String = n.children.iter().map(to_source).collect();
    let align = n.attrs.get("align").map(String::as_str).unwrap_or("");
    let (media, text) = match html.rfind('>') {
        Some(i) => html.split_at(i + 1),
        None => ("", html.as_str()),
    };
    format!(
        "<figure class=\"wp-caption {align}\">{media}<figcaption>{}</figcaption></figure>",
        text.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<ScNode> {
        parse_shortcodes(s, &default_known())
    }

    #[test]
    fn shortcodes_nested_and_unknown_text() {
        let nodes = parse("[et_pb_section][et_pb_text]Hi [key][/et_pb_text][/et_pb_section]");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].tag, "et_pb_section");
        let text = &nodes[0].children[0];
        assert_eq!(text.tag, "et_pb_text");
        assert_eq!(text.children.len(), 1);
        assert_eq!(text.children[0].tag, "");
        assert_eq!(text.children[0].text, "Hi [key]");
    }

    #[test]
    fn shortcode_attr_quoting() {
        let n = &parse(r#"[gravityform id="3" title='Contact us' ajax=true]"#)[0];
        assert_eq!(n.attrs["id"], "3");
        assert_eq!(n.attrs["title"], "Contact us");
        assert_eq!(n.attrs["ajax"], "true");
    }

    #[test]
    fn shortcode_self_closing_and_leaf() {
        let nodes = parse(
            r#"[et_pb_divider height="4px" /][et_pb_image src="a.jpg"][/et_pb_image][gallery ids="1,2"]after"#,
        );
        let tags: Vec<_> = nodes.iter().map(|n| n.tag.as_str()).collect();
        assert_eq!(tags, ["et_pb_divider", "et_pb_image", "gallery", ""]);
        assert_eq!(nodes[0].attrs["height"], "4px");
        assert_eq!(nodes[3].text, "after");
    }

    #[test]
    fn shortcode_unmatched_closer_ignored_and_unknown_kept() {
        let nodes = parse("cookie [key] and [/et_pb_text][link]");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].text, "cookie [key] and [link]");
    }

    #[test]
    fn shortcode_known_is_a_parameter() {
        let known = Regex::new("^site_cta$").unwrap();
        let nodes = parse_shortcodes(r#"a [site_cta link="/x"] b [et_pb_text]"#, &known);
        assert_eq!(nodes[1].tag, "site_cta");
        assert_eq!(nodes[2].text, " b [et_pb_text]");
    }
}
