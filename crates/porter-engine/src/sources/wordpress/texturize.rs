//! WordPress's `wptexturize()` (wp-includes/formatting.php) for the default English settings:
//! smart quotes, apostrophes, dashes, ellipses, times and primes.

use regex::{Captures, Regex};
use std::sync::LazyLock;

const OPEN_Q: &str = "“";
const CLOSE_Q: &str = "”";
const OPEN_SQ: &str = "‘";
const CLOSE_SQ: &str = "’";
const APOS: &str = "’";
const PRIME: &str = "′";
const DOUBLE_PRIME: &str = "″";
const EN: &str = "–";
const EM: &str = "—";
// Placeholders (WordPress uses <!--apos--> etc.) so later rules don't see quotes already decided.
const APOS_FLAG: &str = "\u{1}";
const OPEN_SQ_FLAG: &str = "\u{2}";
const OPEN_Q_FLAG: &str = "\u{3}";
const FLAG: &str = "\u{4}";

/// Text inside these elements is left as written, as WordPress does.
const NO_TEXTURIZE: [&str; 6] = ["pre", "code", "kbd", "style", "script", "tt"];

fn re(s: &str) -> Regex {
    Regex::new(s).expect("valid regex")
}
static SQ_NUM: LazyLock<Regex> = LazyLock::new(|| re(r"'([0-9][0-9])'"));
static SQ_NUM_DQ: LazyLock<Regex> = LazyLock::new(|| re(r#"'([0-9][0-9])""#));
static SQ_QUOTED_NUM: LazyLock<Regex> = LazyLock::new(|| re(r"'([0-9][.,0-9]*)'"));
static DQ_QUOTED_NUM: LazyLock<Regex> = LazyLock::new(|| re(r#""([0-9][.,0-9]*)""#));
static DOUBLE_DASH: LazyLock<Regex> = LazyLock::new(|| re("--"));
static TIMES: LazyLock<Regex> = LazyLock::new(|| re(r"\b([0-9][0-9.,]*)x([0-9][0-9.,]*)\b"));
static TAG_NAME: LazyLock<Regex> = LazyLock::new(|| re(r"^</?([a-zA-Z][a-zA-Z0-9]*)"));

fn is_space(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\t' | ' ' | '\u{a0}')
}
pub(super) fn prev(s: &str, i: usize) -> Option<char> {
    s[..i].chars().next_back()
}
pub(super) fn next(s: &str, i: usize) -> Option<char> {
    s[i..].chars().next()
}
fn space_or_edge(c: Option<char>) -> bool {
    c.is_none_or(is_space)
}
/// WordPress's `(?=$|[.,:;!?)}\-\]]|>|spaces)` after a closing quote.
fn after_close(c: Option<char>) -> bool {
    c.is_none_or(|c| ".,:;!?)}-]>".contains(c) || is_space(c))
}

/// Replaces each match of `re` for which `ok(s, start, end)` holds. A rejected match is
/// retried one character later, as a regex lookaround would be.
pub(super) fn sub(
    s: &str,
    re: &Regex,
    ok: impl Fn(&str, usize, usize) -> bool,
    rep: impl Fn(&Captures) -> String,
) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut last, mut pos) = (0, 0);
    while pos <= s.len() {
        let Some(c) = re.captures_at(s, pos) else {
            break;
        };
        let Some(m) = c.get(0) else { break };
        if ok(s, m.start(), m.end()) {
            out.push_str(&s[last..m.start()]);
            out.push_str(&rep(&c));
            last = m.end();
            pos = m.end().max(m.start() + 1);
        } else {
            pos = m.start() + next(s, m.start()).map_or(1, char::len_utf8);
        }
    }
    out.push_str(&s[last..]);
    out
}

/// Replaces each `ch` for which `ok(s, index)` holds.
fn sub_char(s: &str, ch: char, ok: impl Fn(&str, usize) -> bool, rep: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        if c == ch && ok(s, i) {
            out.push_str(rep);
        } else {
            out.push(c);
        }
    }
    out
}

/// `wptexturize_primes()`: after a number a quote is a prime (6' 2"), unless it closes a quotation.
fn primes(text: &str, needle: char, prime: &str, open: &str, close: &str) -> String {
    let n_len = needle.len_utf8();
    let after_digit = |s: &str, i: usize| prev(s, i).is_some_and(|c| c.is_ascii_digit());
    let quote_ok = |s: &str, i: usize| after_close(next(s, i + n_len));
    let sentences: Vec<String> = text
        .split(open)
        .enumerate()
        .map(|(key, s)| {
            if !s.contains(needle) {
                return s.to_string();
            }
            let mut s = s.to_string();
            if key != 0 && !s.contains(close) {
                let n = s
                    .char_indices()
                    .filter(|&(i, c)| c == needle && quote_ok(&s, i))
                    .count();
                s = sub_char(&s, needle, quote_ok, FLAG);
                if n > 1 {
                    let before = s.clone();
                    s = sub_char(&s, '\u{4}', |s, i| !after_digit(s, i), close);
                    if s == before {
                        let pos = s
                            .rfind(&format!("{FLAG}."))
                            .or_else(|| s.rfind(FLAG))
                            .unwrap_or(0);
                        s = format!("{}{close}{}", &s[..pos], &s[pos + FLAG.len()..]);
                    }
                    s = sub_char(&s, needle, after_digit, prime);
                    s = sub_char(&s, '\u{4}', after_digit, prime).replace(FLAG, close);
                } else if n == 1 {
                    s = sub_char(&s.replace(FLAG, close), needle, after_digit, prime);
                } else {
                    s = sub_char(&s, needle, after_digit, prime);
                }
            } else {
                s = sub_char(&s, needle, after_digit, prime);
                s = sub_char(&s, needle, quote_ok, close);
            }
            if needle == '"' {
                s = s.replace('"', close);
            }
            s
        })
        .collect();
    sentences.join(open)
}

/// The text rules, for one run of text with no tags in it.
fn texturize_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut t = text.to_string();
    for c in [
        "'tain't", "'twere", "'twas", "'tis", "'til", "'bout", "'nuff", "'round", "'cause", "'em",
    ] {
        t = t.replace(c, &c.replace('\'', APOS));
    }
    for (from, to) in [
        ("...", "…"),
        ("``", OPEN_Q),
        ("''", CLOSE_Q),
        (" (tm)", " ™"),
    ] {
        t = t.replace(from, to);
    }

    if t.contains('\'') {
        let num_ok = |s: &str, _: usize, e: usize| after_close(next(s, e));
        t = sub(&t, &SQ_NUM, num_ok, |c| {
            format!("{APOS_FLAG}{}{CLOSE_SQ}", &c[1])
        });
        t = sub(&t, &SQ_NUM_DQ, num_ok, |c| {
            format!("{APOS_FLAG}{}{CLOSE_Q}", &c[1])
        });
        // '99 '99s '99's, but never '9 or '99% or '999 or '99.0
        t = sub_char(
            &t,
            '\'',
            |s, i| {
                let mut cs = s[i + 1..].chars();
                let (a, b, c, d) = (cs.next(), cs.next(), cs.next(), cs.next());
                a.is_some_and(|a| a.is_ascii_digit())
                    && b.is_some_and(|b| b.is_ascii_digit())
                    && !c.is_some_and(|c| {
                        c == '%'
                            || c.is_ascii_digit()
                            || ((c == '.' || c == ',') && d.is_some_and(|d| d.is_ascii_digit()))
                    })
            },
            APOS_FLAG,
        );
        // quoted numbers like '0.42'
        t = sub(
            &t,
            &SQ_QUOTED_NUM,
            |s, st, _| space_or_edge(prev(s, st)),
            |c| format!("{OPEN_SQ_FLAG}{}{CLOSE_SQ}", &c[1]),
        );
        // at the start, or after ( { < [ " - or a space
        t = sub_char(
            &t,
            '\'',
            |s, i| prev(s, i).is_none_or(|c| "([{\"-<".contains(c) || is_space(c)),
            OPEN_SQ_FLAG,
        );
        // apostrophe in a word
        t = sub_char(
            &t,
            '\'',
            |s, i| {
                prev(s, i).is_none_or(|c| !is_space(c))
                    && next(s, i + 1)
                        .is_some_and(|c| !".,:;!?\"'(){}[]-<>".contains(c) && !is_space(c))
            },
            APOS_FLAG,
        );
        t = primes(&t, '\'', PRIME, OPEN_SQ_FLAG, CLOSE_SQ)
            .replace(APOS_FLAG, APOS)
            .replace(OPEN_SQ_FLAG, OPEN_SQ);
    }

    if t.contains('"') {
        t = sub(
            &t,
            &DQ_QUOTED_NUM,
            |s, st, _| space_or_edge(prev(s, st)),
            |c| format!("{OPEN_Q_FLAG}{}{CLOSE_Q}", &c[1]),
        );
        t = sub_char(
            &t,
            '"',
            |s, i| {
                prev(s, i).is_none_or(|c| "([{-<".contains(c) || is_space(c))
                    && !next(s, i + 1).is_some_and(is_space)
            },
            OPEN_Q_FLAG,
        );
        t = primes(&t, '"', DOUBLE_PRIME, OPEN_Q_FLAG, CLOSE_Q).replace(OPEN_Q_FLAG, OPEN_Q);
    }

    if t.contains('-') {
        t = t.replace("---", EM);
        t = sub(
            &t,
            &DOUBLE_DASH,
            |s, st, e| space_or_edge(prev(s, st)) && space_or_edge(next(s, e)),
            |_| EM.to_string(),
        );
        t = sub(
            &t,
            &DOUBLE_DASH,
            |s, st, _| !s[..st].ends_with("xn"),
            |_| EN.to_string(),
        );
        t = sub_char(
            &t,
            '-',
            |s, i| space_or_edge(prev(s, i)) && space_or_edge(next(s, i + 1)),
            EN,
        );
    }

    // 9x9 -> 9×9, but never 0x9999
    if t.contains('x') {
        t = sub(
            &t,
            &TIMES,
            |s, st, _| !s[st..].starts_with("0x"),
            |c| format!("{}×{}", &c[1], &c[2]),
        );
    }
    t
}

/// Texturizes the text of an HTML string; tags, comments and the text inside
/// pre/code/kbd/style/script/tt stay as they are.
pub fn texturize(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut skip: Vec<String> = vec![];
    // Start of the pending text run; a '<' that doesn't open a tag stays in it.
    let (mut text_at, mut i) = (0, 0);
    while let Some(lt) = text[i..].find('<').map(|lt| lt + i) {
        let tail = &text[lt..];
        let opens =
            next(text, lt + 1).is_some_and(|c| c.is_ascii_alphabetic() || c == '/' || c == '!');
        let end = if !opens {
            None
        } else if tail.starts_with("<!--") {
            tail.find("-->").map(|e| e + 3)
        } else {
            tail.find('>').map(|e| e + 1)
        };
        let Some(end) = end else {
            i = lt + 1;
            continue;
        };
        push_text(&mut out, &text[text_at..lt], &skip);
        let tag = &tail[..end];
        if let Some(c) = TAG_NAME.captures(tag) {
            let name = c[1].to_ascii_lowercase();
            if NO_TEXTURIZE.contains(&name.as_str()) {
                if tag.starts_with("</") {
                    if let Some(at) = skip.iter().rposition(|n| *n == name) {
                        skip.truncate(at);
                    }
                } else if !tag.ends_with("/>") {
                    skip.push(name);
                }
            }
        }
        out.push_str(tag);
        i = lt + end;
        text_at = i;
    }
    push_text(&mut out, &text[text_at..], &skip);
    out
}

fn push_text(out: &mut String, text: &str, skip: &[String]) {
    if skip.is_empty() {
        out.push_str(&texturize_text(text));
    } else {
        out.push_str(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texturize_cases() {
        for (from, to) in [
            ("1902 - The Mill", "1902 – The Mill"),
            ("-", "–"),
            (
                "known as the \"Last Train of the Valley\".",
                "known as the “Last Train of the Valley”.",
            ),
            ("women's rights", "women’s rights"),
            (
                "nicknamed the 'The Fortress' by",
                "nicknamed the ‘The Fortress’ by",
            ),
            ("and more...", "and more…"),
            ("a 6' 2\" man", "a 6′ 2″ man"),
            ("1990s and the '90s", "1990s and the ’90s"),
            ("3x4 room", "3×4 room"),
            ("0x1F", "0x1F"),
            ("mon--fri", "mon–fri"),
            ("wait -- what", "wait — what"),
            ("plain text", "plain text"),
        ] {
            assert_eq!(texturize(from), to, "{from}");
        }
    }

    #[test]
    fn texturize_leaves_tags_and_code_alone() {
        assert_eq!(
            texturize(r#"<a href="x" title='y'>it's</a> <code>"a"--b</code> "c""#),
            r#"<a href="x" title='y'>it’s</a> <code>"a"--b</code> “c”"#
        );
    }
}
