use scraper::{Html, Selector};
use url::Url;

#[derive(Debug, Clone)]
pub struct PageMetadata {
    pub url: String,
    pub canonical_url: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
    pub language: Option<String>,
}

impl PageMetadata {
    pub fn effective_url(&self) -> &str {
        self.canonical_url.as_deref().unwrap_or(&self.url)
    }

    pub fn to_post_content(&self) -> String {
        let title = self.title.as_deref().unwrap_or_default();
        let desc = self.description.as_deref().unwrap_or_default();

        if desc.is_empty() {
            title.to_string()
        } else {
            format!("{}\n\n{}", title, desc)
        }
    }

    pub fn is_usable(&self) -> bool {
        self.title.as_ref().is_some_and(|t| !t.is_empty())
    }
}

pub fn extract_metadata(html: &str, source_url: &str) -> PageMetadata {
    let document = Html::parse_document(html);

    let og_title = select_meta_property(&document, "og:title");
    let og_description = select_meta_property(&document, "og:description");
    let og_image = select_meta_property(&document, "og:image");
    let og_site_name = select_meta_property(&document, "og:site_name");
    let og_url = select_meta_property(&document, "og:url");
    let og_locale = select_meta_property(&document, "og:locale");

    let html_title = select_title(&document);
    let meta_description = select_meta_name(&document, "description");
    let html_lang = select_html_lang(&document);
    let canonical = select_canonical(&document);

    let title = og_title.or(html_title);
    let description = og_description.or(meta_description);
    let image_url = og_image.and_then(|img| resolve_url(source_url, &img));
    let canonical_url = canonical.or(og_url);
    let language = og_locale.or(html_lang);

    PageMetadata {
        url: source_url.to_string(),
        canonical_url,
        title,
        description,
        image_url,
        site_name: og_site_name,
        language,
    }
}

pub async fn fetch_and_extract(url: &str) -> anyhow::Result<PageMetadata> {
    let client = reqwest::Client::builder()
        .user_agent("PubkyWebIndex/0.1 (+https://pubky.app)")
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;

    let resp = client.get(url).send().await?;

    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} for {}", resp.status(), url);
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !content_type.contains("text/html") {
        anyhow::bail!("Not HTML: {} for {}", content_type, url);
    }

    let html = resp.text().await?;
    Ok(extract_metadata(&html, url))
}

fn select_meta_property(doc: &Html, property: &str) -> Option<String> {
    let selector =
        Selector::parse(&format!("meta[property=\"{}\"]", property)).ok()?;
    doc.select(&selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn select_meta_name(doc: &Html, name: &str) -> Option<String> {
    let selector =
        Selector::parse(&format!("meta[name=\"{}\"]", name)).ok()?;
    doc.select(&selector)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn select_title(doc: &Html) -> Option<String> {
    let selector = Selector::parse("title").ok()?;
    doc.select(&selector)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

fn select_html_lang(doc: &Html) -> Option<String> {
    let selector = Selector::parse("html").ok()?;
    doc.select(&selector)
        .next()
        .and_then(|el| el.value().attr("lang"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn select_canonical(doc: &Html) -> Option<String> {
    let selector = Selector::parse("link[rel=\"canonical\"]").ok()?;
    doc.select(&selector)
        .next()
        .and_then(|el| el.value().attr("href"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn resolve_url(base: &str, relative: &str) -> Option<String> {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        Some(relative.to_string())
    } else {
        Url::parse(base)
            .ok()
            .and_then(|base_url| base_url.join(relative).ok())
            .map(|u| u.to_string())
    }
}

pub fn normalize_url(raw: &str) -> Option<String> {
    let mut parsed = Url::parse(raw).ok()?;

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }

    parsed.set_fragment(None);

    let cleaned: String = parsed.to_string();
    let cleaned = cleaned.trim_end_matches('/');

    Some(cleaned.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_og_metadata() {
        let html = r#"
        <html lang="en">
        <head>
            <title>Fallback Title</title>
            <meta property="og:title" content="OG Title" />
            <meta property="og:description" content="OG Description" />
            <meta property="og:image" content="https://example.com/image.jpg" />
            <meta property="og:site_name" content="Example" />
        </head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html, "https://example.com/page");
        assert_eq!(meta.title.as_deref(), Some("OG Title"));
        assert_eq!(meta.description.as_deref(), Some("OG Description"));
        assert_eq!(
            meta.image_url.as_deref(),
            Some("https://example.com/image.jpg")
        );
        assert_eq!(meta.site_name.as_deref(), Some("Example"));
        assert!(meta.is_usable());
    }

    #[test]
    fn test_fallback_to_html_title() {
        let html = r#"
        <html>
        <head><title>HTML Title</title></head>
        <body></body>
        </html>
        "#;

        let meta = extract_metadata(html, "https://example.com");
        assert_eq!(meta.title.as_deref(), Some("HTML Title"));
        assert!(meta.is_usable());
    }

    #[test]
    fn test_normalize_url() {
        assert_eq!(
            normalize_url("https://example.com/page#section"),
            Some("https://example.com/page".to_string())
        );
        assert_eq!(
            normalize_url("https://example.com/page/"),
            Some("https://example.com/page".to_string())
        );
        assert_eq!(normalize_url("ftp://example.com"), None);
    }

    #[test]
    fn test_post_content_format() {
        let meta = PageMetadata {
            url: "https://example.com".to_string(),
            canonical_url: None,
            title: Some("Title".to_string()),
            description: Some("Description".to_string()),
            image_url: None,
            site_name: None,
            language: None,
        };

        assert_eq!(meta.to_post_content(), "Title\n\nDescription");
    }
}
