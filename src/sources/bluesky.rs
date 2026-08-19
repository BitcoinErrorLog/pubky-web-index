use futures_util::StreamExt;
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::url_extractor::extract_urls;

const JETSTREAM_URL: &str =
    "wss://jetstream2.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post";

#[derive(Debug, Deserialize)]
struct JetstreamMessage {
    kind: Option<String>,
    commit: Option<JetstreamCommit>,
}

#[derive(Debug, Deserialize)]
struct JetstreamCommit {
    operation: Option<String>,
    record: Option<serde_json::Value>,
}

/// Connects to the Bluesky Jetstream firehose, collects URLs from posts.
/// Runs until `max_urls` unique URLs are collected or timeout.
pub async fn stream_urls_from_jetstream(max_urls: usize) -> anyhow::Result<Vec<String>> {
    let (mut ws, _) = connect_async(JETSTREAM_URL).await?;

    tracing::info!("connected to Bluesky Jetstream");

    let mut urls = Vec::new();
    let timeout = tokio::time::Duration::from_secs(60);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() || urls.len() >= max_urls {
            break;
        }

        let msg = tokio::time::timeout(remaining, ws.next()).await;

        match msg {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Some(found) = parse_jetstream_urls(&text) {
                    for url in found {
                        if !urls.contains(&url) {
                            urls.push(url);
                        }
                    }
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Err(e))) => {
                tracing::debug!(error = %e, "jetstream websocket error");
                break;
            }
            Err(_) => break,
            _ => {}
        }
    }

    let _ = ws.close(None).await;

    urls.truncate(max_urls);
    tracing::info!(count = urls.len(), "collected URLs from Bluesky Jetstream");
    Ok(urls)
}

fn parse_jetstream_urls(raw: &str) -> Option<Vec<String>> {
    let msg: JetstreamMessage = serde_json::from_str(raw).ok()?;

    if msg.kind.as_deref() != Some("commit") {
        return None;
    }

    let commit = msg.commit?;
    if commit.operation.as_deref() != Some("create") {
        return None;
    }

    let record = commit.record?;

    // app.bsky.feed.post records have a "text" field
    let text = record.get("text")?.as_str()?;

    let urls = extract_urls(text);
    if urls.is_empty() {
        None
    } else {
        Some(urls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jetstream_post_with_url() {
        let raw = r#"{"did":"did:plc:abc","time_us":1234567890,"kind":"commit","commit":{"rev":"abc","operation":"create","collection":"app.bsky.feed.post","rkey":"xyz","record":{"$type":"app.bsky.feed.post","text":"Check this out https://example.com/cool-article","createdAt":"2026-03-28T12:00:00Z"}}}"#;
        let urls = parse_jetstream_urls(raw);
        assert!(urls.is_some());
        assert!(urls
            .unwrap()
            .contains(&"https://example.com/cool-article".to_string()));
    }

    #[test]
    fn test_parse_jetstream_post_no_url() {
        let raw = r#"{"did":"did:plc:abc","time_us":1234567890,"kind":"commit","commit":{"rev":"abc","operation":"create","collection":"app.bsky.feed.post","rkey":"xyz","record":{"$type":"app.bsky.feed.post","text":"Just a regular post","createdAt":"2026-03-28T12:00:00Z"}}}"#;
        let urls = parse_jetstream_urls(raw);
        assert!(urls.is_none());
    }

    #[test]
    fn test_parse_jetstream_non_commit() {
        let raw = r#"{"kind":"identity","did":"did:plc:abc"}"#;
        let urls = parse_jetstream_urls(raw);
        assert!(urls.is_none());
    }
}
