use serde::Deserialize;

use crate::metadata::normalize_url;

const CDX_API_BASE: &str = "https://index.commoncrawl.org";

#[derive(Debug, Deserialize)]
struct CdxRecord {
    url: String,
    status: Option<String>,
    mime: Option<String>,
    #[serde(rename = "mime-detected")]
    mime_detected: Option<String>,
    languages: Option<String>,
}

/// Queries the Common Crawl CDX API for URLs matching a domain pattern.
/// Returns normalized URLs that passed basic quality filters.
pub async fn query_cdx(
    crawl_id: &str,
    domain: &str,
    language_filter: &[String],
    max_results: usize,
) -> anyhow::Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .user_agent("PubkyWebIndex/0.1 (+https://pubky.app)")
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let url = format!(
        "{}/{}-index?url={}&output=json&limit={}&filter=status:200&filter=mime:text/html",
        CDX_API_BASE, crawl_id, domain, max_results
    );

    tracing::info!(cdx_url = %url, "querying Common Crawl CDX API");

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("CDX API returned HTTP {}", resp.status());
    }

    let text = resp.text().await?;
    let mut urls = Vec::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let record: CdxRecord = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(line = %line, error = %e, "skipping malformed CDX record");
                continue;
            }
        };

        if let Some(status) = &record.status {
            if status != "200" {
                continue;
            }
        }

        if let Some(mime) = record.mime_detected.as_ref().or(record.mime.as_ref()) {
            if !mime.contains("text/html") {
                continue;
            }
        }

        if !language_filter.is_empty() {
            if let Some(lang) = &record.languages {
                let matches = language_filter
                    .iter()
                    .any(|f| lang.to_lowercase().contains(&f.to_lowercase()));
                if !matches {
                    continue;
                }
            }
        }

        if let Some(normalized) = normalize_url(&record.url) {
            urls.push(normalized);
        }
    }

    urls.dedup();
    tracing::info!(count = urls.len(), domain = %domain, "CDX query returned URLs");
    Ok(urls)
}
