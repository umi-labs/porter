//! Port of WordPress's `wpautop()` (wp-includes/formatting.php), with `$br = true`.

use super::texturize::sub;
use regex::Regex;
use std::sync::LazyLock;

const ALL_BLOCKS: &str = "(?:table|thead|tfoot|caption|col|colgroup|tbody|tr|td|th|div|dl|dd|dt|ul|ol|li|pre|form|map|area|blockquote|address|style|p|h[1-6]|hr|fieldset|legend|section|article|aside|hgroup|header|footer|nav|figure|figcaption|details|menu|summary)";

struct Res {
    double_br: Regex,
    before_block: Regex,
    after_block: Regex,
    after_hr: Regex,
    option_before: Regex,
    option_after: Regex,
    object_open: Regex,
    object_close: Regex,
    param: Regex,
    media_open: Regex,
    media_close: Regex,
    source: Regex,
    figcaption_before: Regex,
    figcaption_after: Regex,
    many_nl: Regex,
    para_split: Regex,
    empty_p: Regex,
    unclosed_p: Regex,
    p_around_block: Regex,
    p_li: Regex,
    p_blockquote: Regex,
    p_before_block: Regex,
    p_after_block: Regex,
    preserve: [Regex; 4],
    nl: Regex,
    br_after_block: Regex,
    br_before_block: Regex,
}

static RE: LazyLock<Res> = LazyLock::new(|| {
    // PCRE's `\s` is ASCII-only; Rust's would also eat no-break spaces.
    let re = |s: &str| {
        Regex::new(&s.replace("ALL", ALL_BLOCKS).replace(r"\s", r"(?-u:\s)")).expect("valid regex")
    };
    Res {
        double_br: re(r"<br\s*/?>\s*<br\s*/?>"),
        before_block: re(r"(<ALL[\s/>])"),
        after_block: re(r"(</ALL>)"),
        after_hr: re(r"(<hr\s*?/?>)"),
        option_before: re(r"\s*<option"),
        option_after: re(r"</option>\s*"),
        object_open: re(r"(<object[^>]*>)\s*"),
        object_close: re(r"\s*</object>"),
        param: re(r"\s*(</?(?:param|embed)[^>]*>)\s*"),
        media_open: re(r"([<\[](?:audio|video)[^>\]]*[>\]])\s*"),
        media_close: re(r"\s*([<\[]/(?:audio|video)[>\]])"),
        source: re(r"\s*(<(?:source|track)[^>]*>)\s*"),
        figcaption_before: re(r"\s*(<figcaption[^>]*>)"),
        figcaption_after: re(r"</figcaption>\s*"),
        many_nl: re(r"\n\n+"),
        para_split: re(r"\n\s*\n"),
        empty_p: re(r"<p>\s*</p>"),
        unclosed_p: re(r"<p>([^<]+)</(div|address|form)>"),
        p_around_block: re(r"<p>\s*(</?ALL[^>]*>)\s*</p>"),
        p_li: re(r"<p>(<li.+?)</p>"),
        p_blockquote: re(r"(?i)<p><blockquote([^>]*)>"),
        p_before_block: re(r"<p>\s*(</?ALL[^>]*>)"),
        p_after_block: re(r"(</?ALL[^>]*>)\s*</p>"),
        preserve: [
            re(r"(?s)<script.*?</script>"),
            re(r"(?s)<style.*?</style>"),
            re(r"(?s)<svg.*?</svg>"),
            re(r"(?s)<math.*?</math>"),
        ],
        nl: re(r"\s*\n"),
        br_after_block: re(r"(</?ALL[^>]*>)\s*<br />"),
        br_before_block: re(r"<br />(\s*</?(?:p|li|div|dl|dd|dt|th|pre|td|ul|ol)[^>]*>)"),
    }
});

/// `wp_replace_in_html_tags()` for "\n": newlines inside tags and comments get a placeholder.
fn newlines_in_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let tail = &rest[lt..];
        let end = if tail.starts_with("<!--") {
            tail.find("-->").map(|e| e + 3)
        } else {
            tail[1..]
                .find(['<', '>'])
                .map(|e| e + 1)
                .filter(|&e| tail.as_bytes()[e] == b'>')
                .map(|e| e + 1)
        }
        .unwrap_or(1);
        out.push_str(&tail[..end].replace('\n', " <!-- wpnl --> "));
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

pub fn autop(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }
    let r = &*RE;
    // Just to make things a little easier, pad the end.
    let mut text = format!("{html}\n");

    // Pre tags shouldn't be touched by autop: placeholders now, contents back at the end.
    let mut pre_tags: Vec<(String, String)> = vec![];
    if text.contains("<pre") {
        let mut parts: Vec<&str> = text.split("</pre>").collect();
        let last_part = parts.pop().unwrap_or_default();
        let mut rebuilt = String::new();
        for part in parts {
            let Some(start) = part.find("<pre") else {
                rebuilt.push_str(part);
                continue;
            };
            let name = format!("<pre wp-pre-tag-{}></pre>", pre_tags.len());
            pre_tags.push((name.clone(), format!("{}</pre>", &part[start..])));
            rebuilt.push_str(&part[..start]);
            rebuilt.push_str(&name);
        }
        rebuilt.push_str(last_part);
        text = rebuilt;
    }

    // Change multiple <br>s into two line breaks, which will turn into paragraphs.
    text = r.double_br.replace_all(&text, "\n\n").into_owned();
    // Double line breaks around block-level tags.
    text = r.before_block.replace_all(&text, "\n\n$1").into_owned();
    text = r.after_block.replace_all(&text, "$1\n\n").into_owned();
    text = r.after_hr.replace_all(&text, "$1\n\n").into_owned();
    text = text.replace("\r\n", "\n").replace('\r', "\n");
    text = newlines_in_tags(&text);

    if text.contains("<option") {
        text = r.option_before.replace_all(&text, "<option").into_owned();
        text = r.option_after.replace_all(&text, "</option>").into_owned();
    }
    if text.contains("</object>") {
        text = r.object_open.replace_all(&text, "$1").into_owned();
        text = r.object_close.replace_all(&text, "</object>").into_owned();
        text = r.param.replace_all(&text, "$1").into_owned();
    }
    if text.contains("<source") || text.contains("<track") {
        text = r.media_open.replace_all(&text, "$1").into_owned();
        text = r.media_close.replace_all(&text, "$1").into_owned();
        text = r.source.replace_all(&text, "$1").into_owned();
    }
    if text.contains("<figcaption") {
        text = r.figcaption_before.replace_all(&text, "$1").into_owned();
        text = r
            .figcaption_after
            .replace_all(&text, "</figcaption>")
            .into_owned();
    }

    text = r.many_nl.replace_all(&text, "\n\n").into_owned();
    // Every chunk between blank lines becomes a paragraph.
    let mut wrapped = String::with_capacity(text.len() + 64);
    for p in r.para_split.split(&text).filter(|p| !p.is_empty()) {
        wrapped.push_str("<p>");
        wrapped.push_str(p.trim_matches('\n'));
        wrapped.push_str("</p>\n");
    }
    text = wrapped;

    text = r.empty_p.replace_all(&text, "").into_owned();
    text = r
        .unclosed_p
        .replace_all(&text, "<p>$1</p></$2>")
        .into_owned();
    text = r.p_around_block.replace_all(&text, "$1").into_owned();
    text = r.p_li.replace_all(&text, "$1").into_owned();
    text = r
        .p_blockquote
        .replace_all(&text, "<blockquote$1><p>")
        .into_owned();
    text = text.replace("</blockquote></p>", "</p></blockquote>");
    text = r.p_before_block.replace_all(&text, "$1").into_owned();
    text = r.p_after_block.replace_all(&text, "$1").into_owned();

    // Line breaks become <br />, except inside script/style/svg/math.
    for re in &r.preserve {
        text = re
            .replace_all(&text, |c: &regex::Captures| {
                c[0].replace('\n', "<WPPreserveNewline />")
            })
            .into_owned();
    }
    text = text.replace("<br>", "<br />").replace("<br/>", "<br />");
    text = sub(
        &text,
        &r.nl,
        |s, st, _| !s[..st].ends_with("<br />"),
        |_| "<br />\n".into(),
    );
    text = text.replace("<WPPreserveNewline />", "\n");

    text = r.br_after_block.replace_all(&text, "$1").into_owned();
    text = r.br_before_block.replace_all(&text, "$1").into_owned();
    // PHP's `\n</p>$`: `$` also matches before a final newline.
    for (from, to) in [("\n</p>\n", "</p>\n"), ("\n</p>", "</p>")] {
        if text.ends_with(from) {
            text.truncate(text.len() - from.len());
            text.push_str(to);
            break;
        }
    }

    for (name, pre) in &pre_tags {
        text = text.replace(name, pre);
    }
    if text.contains("<!-- wpnl -->") {
        text = text
            .replace(" <!-- wpnl --> ", "\n")
            .replace("<!-- wpnl -->", "\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autop_paragraphs_and_br() {
        assert_eq!(autop("a\n\nb\nc"), "<p>a</p>\n<p>b<br />\nc</p>\n");
    }

    #[test]
    fn autop_blocks_pre_and_empty() {
        assert_eq!(autop("  \n"), "");
        assert_eq!(
            autop("<h2>Title</h2>\nText here\n<ul>\n<li>x</li>\n</ul>"),
            "<h2>Title</h2>\n<p>Text here</p>\n<ul>\n<li>x</li>\n</ul>\n"
        );
        assert_eq!(
            autop("<pre>a\n\nb</pre>\n\nafter"),
            "<pre>a\n\nb</pre>\n<p>after</p>\n"
        );
        assert_eq!(autop("a\u{a0}\nb"), "<p>a\u{a0}<br />\nb</p>\n");
        assert_eq!(
            autop("<a\nhref=\"x\">y</a>"),
            "<p><a\nhref=\"x\">y</a></p>\n"
        );
    }
}
