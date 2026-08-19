mod config;
mod link_post;
mod metadata;
mod server;
mod sources;
mod store;
mod url_extractor;
mod writer;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use config::Config;
use link_post::{create_timestamp_id, extract_tags, LinkPost, PubkyTag};
use metadata::{fetch_and_extract, normalize_url, PageMetadata};
use store::UrlStore;

#[derive(Parser)]
#[command(name = "pubky-web-index", about = "Index the web into Pubky")]
struct Cli {
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Crawl seed URLs from config and publish as link posts
    Run,

    /// Query Common Crawl CDX for a domain and index results
    Crawl {
        /// Domain pattern (e.g., "*.wikipedia.org")
        domain: String,
    },

    /// Stream URLs from Nostr relays and index them (local only, no homeserver needed)
    Nostr {
        /// Max URLs to collect from relays
        #[arg(short, long, default_value = "100")]
        max: usize,
    },

    /// Stream URLs from Bluesky Jetstream and index them (local only)
    Bluesky {
        /// Max URLs to collect from firehose
        #[arg(short, long, default_value = "100")]
        max: usize,
    },

    /// Index a single URL (for testing)
    Index {
        /// The URL to index
        url: String,
    },

    /// Start the search API server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "3334")]
        port: u16,
    },

    /// Run as a long-lived daemon: periodic crawl + publish to homeserver + health endpoint
    Daemon,

    /// Generate a new bot identity keypair
    Keygen,

    /// Show statistics about the local index
    Stats,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "pubky_web_index=info".to_string()),
        )
        .init();

    let cli = Cli::parse();

    if matches!(cli.command, Commands::Keygen) {
        run_keygen();
        return Ok(());
    }

    let config = Config::from_env_or_file(&cli.config)?;

    match cli.command {
        Commands::Run => run_seed_crawl(&config).await,
        Commands::Crawl { domain } => run_cdx_crawl(&config, &domain).await,
        Commands::Nostr { max } => run_nostr_local(&config, max).await,
        Commands::Bluesky { max } => run_bluesky_local(&config, max).await,
        Commands::Index { url } => run_single_index(&config, &url).await,
        Commands::Serve { port } => run_server(&config, port).await,
        Commands::Daemon => run_daemon(&config).await,
        Commands::Keygen => unreachable!(),
        Commands::Stats => run_stats(&config),
    }
}

async fn run_seed_crawl(config: &Config) -> anyhow::Result<()> {
    let seed_urls = config
        .crawl
        .seed_urls
        .as_ref()
        .cloned()
        .unwrap_or_default();

    if seed_urls.is_empty() {
        anyhow::bail!("no seed_urls configured in [crawl] section");
    }

    tracing::info!(count = seed_urls.len(), "starting seed URL crawl");

    let store = UrlStore::open(&config.storage.db_path)?;
    let writer = writer::PubkyWriter::connect(&config.homeserver).await?;

    if let Err(e) = writer.publish_profile().await {
        tracing::warn!(error = %e, "failed to publish bot profile");
    }

    let indexed = publish_url_batch(&store, Some(&writer), &seed_urls, "direct", config).await?;
    tracing::info!(total = indexed, "seed crawl complete");
    Ok(())
}

async fn run_cdx_crawl(config: &Config, domain: &str) -> anyhow::Result<()> {
    let crawl_id = config
        .crawl
        .crawl_id
        .as_deref()
        .unwrap_or("CC-MAIN-2026-12");

    let urls = sources::common_crawl::query_cdx(
        crawl_id,
        domain,
        &config.crawl.language_filter,
        config.crawl.max_urls,
    )
    .await?;

    tracing::info!(count = urls.len(), domain = %domain, "CDX returned URLs, beginning indexing");

    let store = UrlStore::open(&config.storage.db_path)?;
    let writer = writer::PubkyWriter::connect(&config.homeserver).await?;

    let indexed = publish_url_batch(&store, Some(&writer), &urls, "common_crawl", config).await?;
    tracing::info!(indexed, "CDX crawl complete for {}", domain);
    Ok(())
}

/// Nostr crawl in local-only mode: fetches URLs, extracts metadata, stores in SQLite.
/// No homeserver connection needed.
async fn run_nostr_local(config: &Config, max_urls: usize) -> anyhow::Result<()> {
    tracing::info!(max = max_urls, "streaming URLs from Nostr relays (local-only mode)");

    let nostr_relays = config.crawl.nostr_relays.as_deref();
    let urls = sources::nostr::stream_urls_from_relays(nostr_relays, max_urls).await;

    if urls.is_empty() {
        tracing::warn!("no URLs found from Nostr relays");
        return Ok(());
    }

    tracing::info!(count = urls.len(), "discovered URLs from Nostr, fetching metadata");

    let store = UrlStore::open(&config.storage.db_path)?;
    let indexed = index_urls_locally(&store, &urls, "nostr", config).await?;
    tracing::info!(indexed, "Nostr local indexing complete");
    Ok(())
}

/// Bluesky crawl in local-only mode.
async fn run_bluesky_local(config: &Config, max_urls: usize) -> anyhow::Result<()> {
    tracing::info!(max = max_urls, "streaming URLs from Bluesky Jetstream (local-only mode)");

    let urls = sources::bluesky::stream_urls_from_jetstream(max_urls).await?;

    if urls.is_empty() {
        tracing::warn!("no URLs found from Bluesky firehose");
        return Ok(());
    }

    tracing::info!(count = urls.len(), "discovered URLs from Bluesky, fetching metadata");

    let store = UrlStore::open(&config.storage.db_path)?;
    let indexed = index_urls_locally(&store, &urls, "bluesky", config).await?;
    tracing::info!(indexed, "Bluesky local indexing complete");
    Ok(())
}

async fn run_single_index(config: &Config, raw_url: &str) -> anyhow::Result<()> {
    let url =
        normalize_url(raw_url).ok_or_else(|| anyhow::anyhow!("invalid URL: {}", raw_url))?;

    tracing::info!(url = %url, "indexing single URL");

    let meta = fetch_and_extract(&url).await?;

    if !meta.is_usable() {
        anyhow::bail!("page has no usable title: {}", url);
    }

    let effective = meta.effective_url().to_string();
    let post_id = create_timestamp_id();
    let post = LinkPost::from_metadata(&meta);

    tracing::info!(
        post_id = %post_id,
        title = ?meta.title,
        description = ?meta.description,
        image = ?meta.image_url,
        "extracted metadata"
    );

    let json = post.to_json()?;
    tracing::info!(json = %json, "post JSON");

    let store = UrlStore::open(&config.storage.db_path)?;
    store_metadata(&store, &effective, &post_id, &meta, "direct")?;

    tracing::info!(url = %effective, post_id = %post_id, "indexed locally");
    Ok(())
}

async fn run_server(config: &Config, port: u16) -> anyhow::Result<()> {
    let store = UrlStore::open(&config.storage.db_path)?;
    let count = store.count()?;
    tracing::info!(indexed_urls = count, "loaded index");
    server::run_server(store, port).await
}

fn run_stats(config: &Config) -> anyhow::Result<()> {
    let store = UrlStore::open(&config.storage.db_path)?;

    let total = store.count()?;
    let direct = store.count_by_source("direct")?;
    let cc = store.count_by_source("common_crawl")?;
    let nostr = store.count_by_source("nostr")?;
    let bluesky = store.count_by_source("bluesky")?;

    println!("pubky-web-index statistics");
    println!("  Database: {}", config.storage.db_path.display());
    println!("  Total indexed URLs: {}", total);
    println!("  Direct crawl:  {}", direct);
    println!("  Common Crawl:  {}", cc);
    println!("  Nostr:         {}", nostr);
    println!("  Bluesky:       {}", bluesky);

    Ok(())
}

/// Index URLs locally: fetch metadata, store in SQLite. No homeserver involved.
async fn index_urls_locally(
    store: &UrlStore,
    urls: &[String],
    source: &str,
    config: &Config,
) -> anyhow::Result<u64> {
    let mut indexed = 0u64;
    let mut skipped = 0u64;
    let mut failed = 0u64;

    for raw_url in urls {
        if indexed >= config.crawl.max_urls as u64 {
            tracing::info!("reached max_urls limit");
            break;
        }

        let Some(url) = normalize_url(raw_url) else {
            continue;
        };

        if store.has_url(&url)? {
            skipped += 1;
            continue;
        }

        match fetch_and_extract(&url).await {
            Ok(meta) if meta.is_usable() => {
                let effective = meta.effective_url().to_string();
                let post_id = create_timestamp_id();
                store_metadata(store, &effective, &post_id, &meta, source)?;
                indexed += 1;

                if indexed % 10 == 0 || indexed <= 3 {
                    tracing::info!(
                        indexed,
                        skipped,
                        failed,
                        url = %effective,
                        title = ?meta.title,
                        "progress"
                    );
                }
            }
            Ok(_) => {
                failed += 1;
            }
            Err(e) => {
                tracing::debug!(url = %url, error = %e, "fetch failed");
                failed += 1;
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    tracing::info!(indexed, skipped, failed, source, "local indexing batch complete");
    Ok(indexed)
}

/// Shared logic for publishing to a homeserver (used by run, crawl commands).
async fn publish_url_batch(
    store: &UrlStore,
    writer: Option<&writer::PubkyWriter>,
    urls: &[String],
    source: &str,
    config: &Config,
) -> anyhow::Result<u64> {
    if let Some(w) = writer {
        tracing::info!(pubkey = %w.pubkey(), source, total_urls = urls.len(), "starting batch publish");
    }
    let mut indexed = 0u64;
    let mut skipped = 0u64;

    for raw_url in urls {
        if indexed >= config.crawl.max_urls as u64 {
            tracing::info!("reached max_urls limit");
            break;
        }

        let Some(url) = normalize_url(raw_url) else {
            continue;
        };

        if store.has_url(&url)? {
            skipped += 1;
            continue;
        }

        match fetch_and_extract(&url).await {
            Ok(meta) if meta.is_usable() => {
                let effective = meta.effective_url().to_string();
                let post_id = create_timestamp_id();
                let post = LinkPost::from_metadata(&meta);

                if let Some(w) = writer {
                    if let Err(e) = w.publish_link_post(&post_id, &post).await {
                        tracing::error!(url = %url, error = %e, "failed to publish");
                        continue;
                    }

                    let tag_labels = extract_tags(&meta);
                    let post_uri = format!("pubky://{}/pub/pubky.app/posts/{}", w.pubkey(), post_id);
                    let now_micros = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_micros() as i64;

                    for label in &tag_labels {
                        let tag = PubkyTag {
                            uri: post_uri.clone(),
                            label: label.clone(),
                            created_at: now_micros,
                        };
                        if let Err(e) = w.publish_tag(&tag).await {
                            tracing::warn!(label = %label, error = %e, "failed to publish tag");
                        }
                    }

                    if !tag_labels.is_empty() {
                        tracing::debug!(tags = ?tag_labels, "published {} tags", tag_labels.len());
                    }
                }

                store_metadata(store, &effective, &post_id, &meta, source)?;
                indexed += 1;

                if indexed % 50 == 0 || indexed <= 5 {
                    tracing::info!(
                        indexed,
                        skipped,
                        url = %effective,
                        title = ?meta.title,
                        "progress"
                    );
                }

                tokio::time::sleep(std::time::Duration::from_millis(
                    config.crawl.write_delay_ms,
                ))
                .await;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(url = %url, error = %e, "fetch failed");
            }
        }
    }

    tracing::info!(indexed, skipped, source, "batch complete");
    Ok(indexed)
}

async fn run_daemon(config: &Config) -> anyhow::Result<()> {
    let dc = &config.daemon;
    tracing::info!(
        interval_secs = dc.interval_secs,
        nostr = dc.enable_nostr,
        bluesky = dc.enable_bluesky,
        seeds = dc.enable_seeds,
        "starting daemon mode"
    );

    let store = UrlStore::open(&config.storage.db_path)?;

    let writer = {
        let max_retries = 10u32;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match writer::PubkyWriter::connect(&config.homeserver).await {
                Ok(w) => break w,
                Err(e) => {
                    if attempt >= max_retries {
                        return Err(e.context("failed to connect to homeserver after retries"));
                    }
                    let delay = std::cmp::min(2u64.pow(attempt), 30);
                    tracing::warn!(attempt, max_retries, delay_secs = delay, error = %e, "homeserver connect failed, retrying");
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                }
            }
        }
    };
    tracing::info!(pubkey = %writer.pubkey(), "bot connected to homeserver");

    if let Err(e) = writer.publish_profile().await {
        tracing::warn!(error = %e, "failed to publish bot profile (posts may not be indexable)");
    }

    let health_store = store.clone();
    let health_port = dc.health_port;
    tokio::spawn(async move {
        if let Err(e) = server::run_server(health_store, health_port).await {
            tracing::error!(error = %e, "health/search server failed");
        }
    });

    loop {
        tracing::info!("starting crawl cycle");

        if dc.enable_seeds {
            if let Some(seed_urls) = config.crawl.seed_urls.as_ref() {
                if !seed_urls.is_empty() {
                    tracing::info!(count = seed_urls.len(), "crawling seed URLs");
                    let urls = seed_urls.clone();
                    match publish_url_batch(&store, Some(&writer), &urls, "direct", config).await {
                        Ok(n) => tracing::info!(indexed = n, "seed crawl done"),
                        Err(e) => tracing::error!(error = %e, "seed crawl failed"),
                    }
                }
            }
        }

        if dc.enable_nostr {
            tracing::info!(max = dc.nostr_max, "crawling Nostr relays");
            let nostr_relays = config.crawl.nostr_relays.as_deref();
            let urls = sources::nostr::stream_urls_from_relays(nostr_relays, dc.nostr_max).await;
            if !urls.is_empty() {
                tracing::info!(found = urls.len(), "Nostr URLs discovered");
                match publish_url_batch(&store, Some(&writer), &urls, "nostr", config).await {
                    Ok(n) => tracing::info!(indexed = n, "Nostr crawl done"),
                    Err(e) => tracing::error!(error = %e, "Nostr crawl failed"),
                }
            }
        }

        if dc.enable_bluesky {
            tracing::info!(max = dc.bluesky_max, "crawling Bluesky Jetstream");
            match sources::bluesky::stream_urls_from_jetstream(dc.bluesky_max).await {
                Ok(urls) if !urls.is_empty() => {
                    tracing::info!(found = urls.len(), "Bluesky URLs discovered");
                    match publish_url_batch(&store, Some(&writer), &urls, "bluesky", config).await {
                        Ok(n) => tracing::info!(indexed = n, "Bluesky crawl done"),
                        Err(e) => tracing::error!(error = %e, "Bluesky crawl failed"),
                    }
                }
                Ok(_) => tracing::debug!("no Bluesky URLs found"),
                Err(e) => tracing::warn!(error = %e, "Bluesky crawl failed"),
            }
        }

        let total = store.count().unwrap_or(0);
        tracing::info!(total_indexed = total, sleep_secs = dc.interval_secs, "crawl cycle complete, sleeping");
        tokio::time::sleep(std::time::Duration::from_secs(dc.interval_secs)).await;
    }
}

fn run_keygen() {
    use pubky::Keypair;
    let keypair = Keypair::random();
    let pubkey = keypair.public_key().to_string();
    let secret = hex::encode(keypair.secret_key());
    println!("Bot Identity Keypair Generated");
    println!("  Public Key: {}", pubkey);
    println!("  Secret Key: {}", secret);
    println!();
    println!("Set these as environment variables:");
    println!("  BOT_SECRET_KEY={}", secret);
    println!("  HOMESERVER_PUBKEY=<your-homeserver-pubkey>");
}

fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
}

fn store_metadata(
    store: &UrlStore,
    url: &str,
    post_id: &str,
    meta: &PageMetadata,
    source: &str,
) -> anyhow::Result<()> {
    store.insert_url(
        url,
        post_id,
        meta.title.as_deref(),
        meta.description.as_deref(),
        meta.image_url.as_deref(),
        meta.site_name.as_deref(),
        extract_domain(url).as_deref(),
        meta.language.as_deref(),
        source,
    )
}
