use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

use crate::store::{IndexedUrl, UrlStore};

#[derive(Clone)]
struct AppState {
    store: UrlStore,
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Serialize)]
struct SearchResponse {
    results: Vec<IndexedUrl>,
    total: i64,
    query: String,
}

#[derive(Serialize)]
struct StatsResponse {
    total: i64,
    direct: i64,
    common_crawl: i64,
    nostr: i64,
    bluesky: i64,
}

pub async fn run_server(store: UrlStore, port: u16) -> anyhow::Result<()> {
    let state = AppState { store };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/api/search", get(search_handler))
        .route("/api/recent", get(recent_handler))
        .route("/api/stats", get(stats_handler))
        .layer(cors)
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    tracing::info!(addr = %addr, "starting search API server");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn search_handler(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, StatusCode> {
    let query = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(20).min(100);
    let offset = params.offset.unwrap_or(0);

    if query.is_empty() {
        let results = state
            .store
            .recent(limit)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let total = state
            .store
            .count()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(Json(SearchResponse {
            results,
            total,
            query,
        }));
    }

    let results = state
        .store
        .search(&query, limit, offset)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = results.len() as i64;

    Ok(Json(SearchResponse {
        results,
        total,
        query,
    }))
}

async fn recent_handler(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<IndexedUrl>>, StatusCode> {
    let limit = params.limit.unwrap_or(20).min(100);
    state
        .store
        .recent(limit)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn stats_handler(
    State(state): State<AppState>,
) -> Result<Json<StatsResponse>, StatusCode> {
    let store = &state.store;
    Ok(Json(StatsResponse {
        total: store.count().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        direct: store.count_by_source("direct").map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        common_crawl: store.count_by_source("common_crawl").map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        nostr: store.count_by_source("nostr").map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        bluesky: store.count_by_source("bluesky").map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    }))
}
