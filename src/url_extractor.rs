use regex::Regex;
use std::sync::LazyLock;

use crate::metadata::normalize_url;

static URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://[^\s<>\[\](){}|\\^`\x00-\x1f\x7f]+").unwrap()
});

/// Extracts and normalizes HTTP/HTTPS URLs from free text.
pub fn extract_urls(text: &str) -> Vec<String> {
    URL_REGEX
        .find_iter(text)
        .filter_map(|m| {
            let raw = m.as_str().trim_end_matches(|c: char| {
                matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"' | '\'')
            });
            normalize_url(raw)
        })
        .filter(|url| !is_excluded_url(url))
        .collect()
}

fn is_excluded_url(url: &str) -> bool {
    let dominated_by = [
        "localhost",
        "127.0.0.1",
        "0.0.0.0",
        "10.",
        "192.168.",
        "172.16.",
    ];

    for pattern in &dominated_by {
        if url.contains(pattern) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_single_url() {
        let text = "Check out https://example.com/article today!";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/article"]);
    }

    #[test]
    fn test_extract_multiple_urls() {
        let text = "Visit https://a.com and https://b.com for more.";
        let urls = extract_urls(text);
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://a.com".to_string()));
        assert!(urls.contains(&"https://b.com".to_string()));
    }

    #[test]
    fn test_strip_trailing_punctuation() {
        let text = "See https://example.com/page.";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/page"]);
    }

    #[test]
    fn test_no_urls() {
        let text = "This has no links at all";
        let urls = extract_urls(text);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_excludes_localhost() {
        let text = "Running at http://localhost:3000/test";
        let urls = extract_urls(text);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_excludes_private_ips() {
        let text = "Dev: http://192.168.1.1/admin";
        let urls = extract_urls(text);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_url_with_fragment_stripped() {
        let text = "See https://example.com/page#section for details";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://example.com/page"]);
    }
}
