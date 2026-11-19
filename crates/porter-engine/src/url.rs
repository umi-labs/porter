use ::url::{Url, form_urlencoded};
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};

/// Canonical form of a URL used to match routes across connectors:
/// `host[:port]/path/[?sorted&query]`, no scheme, no `www.`, no fragment.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UrlKey(pub String);

/// Characters re-encoded in a path segment after decoding.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

const DROPPED_PARAMS: [&str; 3] = ["fbclid", "gclid", "_ga"];

/// Returns `None` for unparseable, relative-without-base, or non-http(s) URLs.
pub fn normalise_url(raw: &str, base: Option<&Url>) -> Option<UrlKey> {
    let url = match base {
        Some(b) => b.join(raw.trim()).ok()?,
        None => Url::parse(raw.trim()).ok()?,
    };
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    let mut key = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if let Some(port) = url.port() {
        key.push_str(&format!(":{port}"));
    }

    let decoded: Vec<String> = url
        .path()
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned())
        .collect();
    let has_ext = decoded
        .last()
        .and_then(|s| s.rsplit_once('.'))
        .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty());
    // File paths (last segment has an extension) are case-sensitive; page paths are not.
    let segments: Vec<String> = decoded
        .iter()
        .map(|s| {
            let s = if has_ext { s.clone() } else { s.to_lowercase() };
            utf8_percent_encode(&s, SEGMENT).to_string()
        })
        .collect();
    key.push('/');
    key.push_str(&segments.join("/"));
    if !segments.is_empty() && !has_ext {
        key.push('/');
    }

    let mut params: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            let k = k.to_lowercase();
            !k.starts_with("utm_") && !DROPPED_PARAMS.contains(&k.as_str())
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if !params.is_empty() {
        params.sort();
        key.push('?');
        key.push_str(
            &form_urlencoded::Serializer::new(String::new())
                .extend_pairs(params)
                .finish(),
        );
    }
    Some(UrlKey(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> UrlKey {
        normalise_url(s, None).unwrap()
    }

    #[test]
    fn file_paths_keep_case() {
        assert_ne!(
            key("https://x.com/Photo.jpg"),
            key("https://x.com/photo.jpg")
        );
        assert_eq!(key("https://x.com/About/"), key("https://x.com/about"));
    }

    #[test]
    fn url_equal_keys() {
        let want = key("https://example.com/about/");
        for s in [
            "http://www.Example.com/About",
            "https://example.com/about?utm_source=x",
            "https://example.com/%61bout/",
            "https://example.com/about#top",
        ] {
            assert_eq!(key(s), want, "{s}");
        }
        assert_eq!(want.0, "example.com/about/");
    }

    #[test]
    fn url_distinct_keys() {
        let a = key("https://example.com/?p=12");
        let b = key("https://example.com/?p=13");
        assert_ne!(a, b);
        assert_eq!(a.0, "example.com/?p=12");
        assert_eq!(
            key("https://example.com/file.pdf").0,
            "example.com/file.pdf"
        );
    }

    #[test]
    fn url_query_sorted_and_tracking_dropped() {
        assert_eq!(
            key("https://example.com/x?b=2&fbclid=z&a=1&gclid=q&_ga=1&UTM_medium=m").0,
            "example.com/x/?a=1&b=2"
        );
    }

    #[test]
    fn url_relative_and_rejects() {
        let base = ::url::Url::parse("https://www.example.com/blog/").unwrap();
        assert_eq!(
            normalise_url("post-1", Some(&base)).unwrap().0,
            "example.com/blog/post-1/"
        );
        assert_eq!(normalise_url("/a", None), None);
        assert_eq!(normalise_url("mailto:x@example.com", None), None);
        assert_eq!(key("https://example.com:8080/a").0, "example.com:8080/a/");
    }
}
