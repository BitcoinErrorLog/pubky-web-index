use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub homeserver: HomeserverConfig,
    pub crawl: CrawlConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HomeserverConfig {
    pub pubkey: String,
    pub secret_key: String,
    #[serde(default)]
    pub testnet: bool,
    pub signup_code: Option<String>,
    pub direct_url: Option<String>,
    pub testnet_host: Option<String>,
    pub pkarr_relay_url: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CrawlConfig {
    #[serde(default = "default_source")]
    pub source: String,
    pub crawl_id: Option<String>,
    #[serde(default = "default_language_filter")]
    pub language_filter: Vec<String>,
    #[serde(default = "default_max_urls")]
    pub max_urls: usize,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_write_delay_ms")]
    pub write_delay_ms: u64,
    pub seed_urls: Option<Vec<String>>,
    pub nostr_relays: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DaemonConfig {
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "default_health_port")]
    pub health_port: u16,
    #[serde(default = "default_nostr_max")]
    pub nostr_max: usize,
    #[serde(default = "default_bluesky_max")]
    pub bluesky_max: usize,
    #[serde(default = "default_true")]
    pub enable_nostr: bool,
    #[serde(default)]
    pub enable_bluesky: bool,
    #[serde(default = "default_true")]
    pub enable_seeds: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_interval_secs(),
            health_port: default_health_port(),
            nostr_max: default_nostr_max(),
            bluesky_max: default_bluesky_max(),
            enable_nostr: true,
            enable_bluesky: false,
            enable_seeds: true,
        }
    }
}

fn default_source() -> String {
    "direct".to_string()
}
fn default_language_filter() -> Vec<String> {
    vec!["en".to_string()]
}
fn default_max_urls() -> usize {
    10_000
}
fn default_batch_size() -> usize {
    50
}
fn default_write_delay_ms() -> u64 {
    200
}
fn default_db_path() -> PathBuf {
    PathBuf::from("./data/webindex.db")
}
fn default_interval_secs() -> u64 {
    300
}
fn default_health_port() -> u16 {
    8080
}
fn default_nostr_max() -> usize {
    200
}
fn default_bluesky_max() -> usize {
    200
}
fn default_true() -> bool {
    true
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Load config from environment variables (for Railway/Docker deployment).
    /// Falls back to file-based config if env vars aren't set.
    pub fn from_env_or_file(path: &Path) -> anyhow::Result<Self> {
        if let Ok(secret_key) = std::env::var("BOT_SECRET_KEY") {
            let config = Config {
                homeserver: HomeserverConfig {
                    pubkey: std::env::var("HOMESERVER_PUBKEY")
                        .unwrap_or_default(),
                    secret_key,
                    testnet: std::env::var("TESTNET")
                        .map(|v| v == "true" || v == "1")
                        .unwrap_or(false),
                    signup_code: std::env::var("SIGNUP_CODE").ok(),
                    direct_url: std::env::var("HOMESERVER_DIRECT_URL").ok(),
                    testnet_host: std::env::var("TESTNET_HOST").ok(),
                    pkarr_relay_url: std::env::var("PKARR_RELAY_URL").ok(),
                },
                crawl: CrawlConfig {
                    source: std::env::var("CRAWL_SOURCE").unwrap_or_else(|_| "daemon".to_string()),
                    crawl_id: std::env::var("CRAWL_ID").ok(),
                    language_filter: std::env::var("LANGUAGE_FILTER")
                        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
                        .unwrap_or_else(|_| vec!["en".to_string()]),
                    max_urls: std::env::var("MAX_URLS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(10_000),
                    batch_size: default_batch_size(),
                    write_delay_ms: std::env::var("WRITE_DELAY_MS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(200),
                    seed_urls: std::env::var("SEED_URLS")
                        .ok()
                        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect()),
                    nostr_relays: std::env::var("NOSTR_RELAYS")
                        .ok()
                        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect()),
                },
                storage: StorageConfig {
                    db_path: std::env::var("DB_PATH")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("./data/webindex.db")),
                },
                daemon: DaemonConfig {
                    interval_secs: std::env::var("DAEMON_INTERVAL_SECS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(300),
                    health_port: std::env::var("PORT")
                        .or_else(|_| std::env::var("HEALTH_PORT"))
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(8080),
                    nostr_max: std::env::var("NOSTR_MAX")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(200),
                    bluesky_max: std::env::var("BLUESKY_MAX")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(200),
                    enable_nostr: std::env::var("ENABLE_NOSTR")
                        .map(|v| v != "false" && v != "0")
                        .unwrap_or(true),
                    enable_bluesky: std::env::var("ENABLE_BLUESKY")
                        .map(|v| v == "true" || v == "1")
                        .unwrap_or(false),
                    enable_seeds: std::env::var("ENABLE_SEEDS")
                        .map(|v| v != "false" && v != "0")
                        .unwrap_or(true),
                },
            };
            Ok(config)
        } else if path.exists() {
            Self::load(path)
        } else {
            anyhow::bail!(
                "No config found. Set BOT_SECRET_KEY env var or provide config file at {}",
                path.display()
            )
        }
    }
}
