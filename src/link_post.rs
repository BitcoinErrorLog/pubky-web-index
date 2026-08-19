use base32::{encode, Alphabet};
use blake3::Hasher;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::metadata::PageMetadata;

const MAX_CONTENT_LENGTH: usize = 2000;
const MAX_TAG_LABEL_LENGTH: usize = 20;
const MAX_TAGS_PER_POST: usize = 5;

static LAST_TIMESTAMP: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Debug)]
pub struct LinkPost {
    pub content: String,
    pub kind: String,
    pub parent: Option<String>,
    pub embed: Option<LinkPostEmbed>,
    pub attachments: Option<Vec<String>>,
}

#[derive(Serialize, Debug)]
pub struct LinkPostEmbed {
    pub kind: String,
    pub uri: String,
}

impl LinkPost {
    pub fn from_metadata(meta: &PageMetadata) -> Self {
        let mut content = meta.to_post_content();

        if let Some(site) = &meta.site_name {
            if !content.contains(site) {
                content = format!("{}\n\n— {}", content, site);
            }
        }

        let content = truncate_content(&content, MAX_CONTENT_LENGTH);
        let url = meta.effective_url().to_string();

        let attachments = meta.image_url.as_ref().map(|img| vec![img.clone()]);

        LinkPost {
            content,
            kind: "link".to_string(),
            parent: None,
            embed: Some(LinkPostEmbed {
                kind: "link".to_string(),
                uri: url,
            }),
            attachments,
        }
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Generate a timestamp-based post ID compatible with PubkyAppPost's TimestampId.
/// Uses current time in microseconds, encoded as 8-byte big-endian Crockford Base32 (13 chars).
/// Guarantees monotonically increasing IDs even when called rapidly.
pub fn create_timestamp_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_micros() as u64;

    let prev = LAST_TIMESTAMP.fetch_max(now, Ordering::SeqCst);
    let ts = if now <= prev { prev + 1 } else { now };
    LAST_TIMESTAMP.store(ts, Ordering::SeqCst);

    let bytes = (ts as i64).to_be_bytes();
    encode(Alphabet::Crockford, &bytes)
}

/// Homeserver path for a post.
pub fn post_path(post_id: &str) -> String {
    format!("/pub/pubky.app/posts/{}", post_id)
}

fn truncate_content(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars - 3).collect();
        format!("{}...", truncated)
    }
}

#[derive(Serialize, Debug)]
pub struct PubkyTag {
    pub uri: String,
    pub label: String,
    pub created_at: i64,
}

impl PubkyTag {
    pub fn create_id(&self) -> String {
        let data = format!("{}:{}", self.uri, self.label);
        let mut hasher = Hasher::new();
        hasher.update(data.as_bytes());
        let hash = hasher.finalize();
        let half = &hash.as_bytes()[..hash.as_bytes().len() / 2];
        encode(Alphabet::Crockford, half)
    }

    pub fn path(&self) -> String {
        format!("/pub/pubky.app/tags/{}", self.create_id())
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Extract tag labels from page metadata. Returns lowercase single-word tags,
/// max 20 chars each, no whitespace or special chars.
pub fn extract_tags(meta: &PageMetadata) -> Vec<String> {
    let stop_words: std::collections::HashSet<&str> = [
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for",
        "of", "with", "by", "from", "is", "it", "as", "be", "was", "are",
        "this", "that", "how", "what", "why", "when", "where", "who", "which",
        "not", "no", "if", "do", "does", "did", "has", "have", "had", "will",
        "can", "may", "its", "all", "your", "you", "we", "our", "their", "my",
        "more", "about", "just", "new", "also", "than", "other", "into", "some",
        "up", "out", "so", "one", "two", "get", "like", "make", "us", "been",
    ]
    .iter()
    .copied()
    .collect();

    let invalid_chars: &[char] = &[',', ':', '.', '!', '?', '"', '\'', '(', ')', '[', ']', '{', '}', '/', '\\', '|', '<', '>', '@', '#', '$', '%', '^', '&', '*', '+', '=', '~', '`', ';'];

    let mut seen = std::collections::HashSet::new();
    let mut tags = Vec::new();

    let text = format!(
        "{} {}",
        meta.title.as_deref().unwrap_or(""),
        meta.site_name.as_deref().unwrap_or("")
    );

    for word in text.split_whitespace() {
        let clean: String = word
            .to_lowercase()
            .chars()
            .filter(|c| !invalid_chars.contains(c))
            .collect();

        if clean.is_empty()
            || clean.len() < 3
            || clean.len() > MAX_TAG_LABEL_LENGTH
            || clean.chars().any(|c| c.is_whitespace())
            || stop_words.contains(clean.as_str())
            || clean.chars().all(|c| c.is_numeric())
        {
            continue;
        }

        if seen.insert(clean.clone()) {
            tags.push(clean);
        }
        if tags.len() >= MAX_TAGS_PER_POST {
            break;
        }
    }

    if let Some(domain) = meta.site_name.as_deref() {
        let domain_tag: String = domain
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        if domain_tag.len() >= 3
            && domain_tag.len() <= MAX_TAG_LABEL_LENGTH
            && seen.insert(domain_tag.clone())
            && tags.len() < MAX_TAGS_PER_POST
        {
            tags.push(domain_tag);
        }
    }

    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timestamp_id_length() {
        let id = create_timestamp_id();
        assert_eq!(id.len(), 13, "post ID must be exactly 13 characters");
    }

    #[test]
    fn test_timestamp_id_monotonic() {
        let id1 = create_timestamp_id();
        let id2 = create_timestamp_id();
        assert_ne!(id1, id2, "sequential IDs must be unique");
        assert!(id2 > id1, "IDs must be monotonically increasing");
    }

    #[test]
    fn test_post_path() {
        let id = create_timestamp_id();
        assert_eq!(id.len(), 13, "post ID must be exactly 13 characters");
        let path = post_path(&id);
        assert!(path.starts_with("/pub/pubky.app/posts/"));
    }

    #[test]
    fn test_from_metadata() {
        let meta = PageMetadata {
            url: "https://example.com/article".to_string(),
            canonical_url: None,
            title: Some("Great Article".to_string()),
            description: Some("A description".to_string()),
            image_url: Some("https://example.com/img.jpg".to_string()),
            site_name: Some("Example".to_string()),
            language: Some("en".to_string()),
        };

        let post = LinkPost::from_metadata(&meta);
        assert_eq!(post.kind, "link");
        assert_eq!(post.content, "Great Article\n\nA description\n\n\u{2014} Example");
        assert!(post.embed.is_some());
        assert_eq!(
            post.embed.as_ref().unwrap().uri,
            "https://example.com/article"
        );
        assert!(post.attachments.is_some());
    }

    #[test]
    fn test_truncate() {
        let long = "a".repeat(3000);
        let post = LinkPost {
            content: truncate_content(&long, MAX_CONTENT_LENGTH),
            kind: "link".to_string(),
            parent: None,
            embed: None,
            attachments: None,
        };
        assert!(post.content.chars().count() <= MAX_CONTENT_LENGTH);
        assert!(post.content.ends_with("..."));
    }
}
