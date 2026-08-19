# pubky-web-index

Crawls URLs and publishes them as Pubky link posts. Also keeps a local SQLite index with Open Graph metadata and serves a small search API.

Sources:

- Seed URLs from config
- URLs posted on Nostr relays
- URLs posted on Bluesky (Jetstream)
- Common Crawl CDX (optional)

## Build

```sh
cargo build --release
```

## Config

Copy the example and fill in a bot secret and homeserver:

```sh
cp config.example.toml config.toml
cargo run --release -- keygen
```

`config.toml` is gitignored. Do not commit secrets.

Alternatively, configure with environment variables (`BOT_SECRET_KEY`, `HOMESERVER_PUBKEY`, `HOMESERVER_DIRECT_URL`, `PKARR_RELAY_URL`, `ENABLE_NOSTR`, `ENABLE_BLUESKY`, `DB_PATH`, `PORT`).

## Commands

```sh
# Generate a bot keypair
cargo run --release -- keygen

# Crawl seed URLs and publish to the homeserver
cargo run --release -- run

# Index URLs from Nostr (local SQLite only)
cargo run --release -- nostr --max 200

# Index URLs from Bluesky (local SQLite only)
cargo run --release -- bluesky --max 200

# Serve the search API (default port 3334)
cargo run --release -- serve --port 8082

# Periodic crawl + search API
cargo run --release -- daemon

# Local index stats
cargo run --release -- stats
```

## Search API

- `GET /api/search?q=bitcoin&limit=30`
- `GET /api/recent?limit=30`
- `GET /api/stats`

Each record stores URL, title, description, image, site name, domain, language, source (`direct` / `nostr` / `bluesky`), and the Pubky post id.

## Docker

The image runs `daemon` and reads config from environment variables. Mount `/data` if you want the SQLite index to persist:

```sh
docker build -t pubky-web-index .
docker run --rm -p 8082:8080 \
  -e BOT_SECRET_KEY= \
  -e HOMESERVER_PUBKEY= \
  -e HOMESERVER_DIRECT_URL=http://host.docker.internal:6286 \
  -e PORT=8080 \
  -e ENABLE_NOSTR=true \
  -e ENABLE_BLUESKY=true \
  -v "$(pwd)/data:/data" \
  pubky-web-index
```
