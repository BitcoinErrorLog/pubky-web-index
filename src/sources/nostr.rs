use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::url_extractor::extract_urls;

const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.nostr.band",
];

#[derive(Debug, Deserialize)]
struct NostrEvent {
    // id: String,
    // pubkey: String,
    // created_at: u64,
    kind: u64,
    content: String,
}

/// Connects to Nostr relays, subscribes to kind 1 notes, and yields URLs found in them.
/// Runs until `max_urls` unique URLs are collected or the relay closes.
pub async fn stream_urls_from_relays(
    relays: Option<&[String]>,
    max_urls: usize,
) -> Vec<String> {
    let relay_urls: Vec<&str> = match relays {
        Some(r) => r.iter().map(|s| s.as_str()).collect(),
        None => DEFAULT_RELAYS.to_vec(),
    };

    let mut all_urls = Vec::new();

    for relay_url in relay_urls {
        if all_urls.len() >= max_urls {
            break;
        }

        let remaining = max_urls - all_urls.len();
        match connect_to_relay(relay_url, remaining).await {
            Ok(urls) => {
                tracing::info!(relay = %relay_url, found = urls.len(), "collected URLs from relay");
                all_urls.extend(urls);
            }
            Err(e) => {
                tracing::warn!(relay = %relay_url, error = %e, "failed to connect to relay");
            }
        }
    }

    all_urls.dedup();
    all_urls.truncate(max_urls);
    all_urls
}

async fn connect_to_relay(relay_url: &str, max_urls: usize) -> anyhow::Result<Vec<String>> {
    let (mut ws, _) = connect_async(relay_url).await?;

    // NIP-01: Send a REQ with a filter for kind 1 (text notes), limited batch
    let sub_id = "pubky-web-index";
    let filter = serde_json::json!([
        "REQ",
        sub_id,
        {
            "kinds": [1],
            "limit": max_urls.min(500)
        }
    ]);

    ws.send(Message::Text(filter.to_string())).await?;

    let mut urls = Vec::new();
    let timeout = tokio::time::Duration::from_secs(30);
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() || urls.len() >= max_urls {
            break;
        }

        let msg = tokio::time::timeout(remaining, ws.next()).await;

        match msg {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Some(found) = parse_nostr_event_urls(&text) {
                    for url in found {
                        if !urls.contains(&url) {
                            urls.push(url);
                        }
                    }
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Err(e))) => {
                tracing::debug!(error = %e, "websocket error");
                break;
            }
            Err(_) => break, // timeout
            _ => {}
        }
    }

    let _ = ws.close(None).await;
    Ok(urls)
}

fn parse_nostr_event_urls(raw: &str) -> Option<Vec<String>> {
    let arr: serde_json::Value = serde_json::from_str(raw).ok()?;
    let arr = arr.as_array()?;

    // NIP-01 EVENT message: ["EVENT", <sub_id>, <event>]
    if arr.len() < 3 {
        return None;
    }

    let msg_type = arr[0].as_str()?;
    if msg_type != "EVENT" {
        return None;
    }

    let event: NostrEvent = serde_json::from_value(arr[2].clone()).ok()?;
    if event.kind != 1 {
        return None;
    }

    let urls = extract_urls(&event.content);
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
    fn test_parse_nostr_event_with_url() {
        let raw = r#"["EVENT","sub1",{"id":"abc","pubkey":"def","created_at":1234567890,"kind":1,"content":"Check this out https://example.com/article","tags":[],"sig":"ghi"}]"#;
        let urls = parse_nostr_event_urls(raw);
        assert!(urls.is_some());
        let urls = urls.unwrap();
        assert!(urls.contains(&"https://example.com/article".to_string()));
    }

    #[test]
    fn test_parse_nostr_event_no_url() {
        let raw = r#"["EVENT","sub1",{"id":"abc","pubkey":"def","created_at":1234567890,"kind":1,"content":"Just a regular note","tags":[],"sig":"ghi"}]"#;
        let urls = parse_nostr_event_urls(raw);
        assert!(urls.is_none());
    }

    #[test]
    fn test_parse_non_event() {
        let raw = r#"["EOSE","sub1"]"#;
        let urls = parse_nostr_event_urls(raw);
        assert!(urls.is_none());
    }
}
