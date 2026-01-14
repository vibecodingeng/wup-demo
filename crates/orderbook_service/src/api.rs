//! HTTP API handlers and routes using axum.
//!
//! Provides REST API for orderbook queries with aggregate_id support.
//!
//! Routes:
//! - GET /health - Health check
//! - GET /stats - Store statistics
//! - GET /orderbooks - List all orderbooks
//! - GET /orderbook/{aggregate_id} - Get all orderbooks for an aggregate
//! - GET /orderbook/{aggregate_id}/{hashed_market_id} - Get orderbooks for market within aggregate
//! - GET /orderbook/{aggregate_id}/{hashed_market_id}/{clob_token_id} - Get specific orderbook
//! - GET /bbo/{aggregate_id}/{hashed_market_id} - Get BBOs for market
//! - GET /bbo/{aggregate_id}/{hashed_market_id}/{clob_token_id} - Get specific BBO

use crate::store::OrderbookStore;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

/// Application state shared across handlers.
#[derive(Clone)]
pub struct AppState {
    pub store: OrderbookStore,
}

/// Query parameters for orderbook endpoints.
#[derive(Debug, Deserialize)]
pub struct OrderbookQuery {
    /// Number of price levels to return (default: 10).
    pub depth: Option<usize>,
}

/// Create the API router.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Health and stats
        .route("/health", get(health_handler))
        .route("/stats", get(stats_handler))
        .route("/orderbooks", get(list_all_handler))
        // Aggregate-based routes
        .route("/orderbook/{aggregate_id}", get(get_aggregate_handler))
        .route("/orderbook/{aggregate_id}/{hashed_market_id}", get(get_market_handler))
        .route("/orderbook/{aggregate_id}/{hashed_market_id}/{clob_token_id}", get(get_orderbook_handler))
        .route("/bbo/{aggregate_id}/{hashed_market_id}", get(get_market_bbos_handler))
        .route("/bbo/{aggregate_id}/{hashed_market_id}/{clob_token_id}", get(get_bbo_handler))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(Arc::new(state))
}

// ============================================================================
// Handlers
// ============================================================================

/// Health check endpoint.
/// GET /health
async fn health_handler() -> impl IntoResponse {
    Json(HealthResponse { status: "ok" })
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// Get store statistics.
/// GET /stats
async fn stats_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let stats = state.store.stats();
    Json(stats)
}

/// List all aggregates, markets, and tokens (summary).
/// GET /orderbooks
async fn list_all_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let response = state.store.list_all();
    Json(response)
}

/// Get all orderbooks for an aggregate.
/// GET /orderbook/{aggregate_id}
async fn get_aggregate_handler(
    State(state): State<Arc<AppState>>,
    Path(aggregate_id): Path<String>,
    Query(query): Query<OrderbookQuery>,
) -> Result<impl IntoResponse, ApiError> {
    match state.store.get_aggregate(&aggregate_id, query.depth) {
        Some(response) => Ok(Json(response)),
        None => Err(ApiError::NotFound(format!("Aggregate '{}' not found", aggregate_id))),
    }
}

/// Get all orderbooks for a market within an aggregate.
/// GET /orderbook/{aggregate_id}/{hashed_market_id}
async fn get_market_handler(
    State(state): State<Arc<AppState>>,
    Path((aggregate_id, hashed_market_id)): Path<(String, String)>,
    Query(query): Query<OrderbookQuery>,
) -> Result<impl IntoResponse, ApiError> {
    match state.store.get_market(&aggregate_id, &hashed_market_id, query.depth) {
        Some(response) => Ok(Json(response)),
        None => Err(ApiError::NotFound(format!(
            "Market '{}' in aggregate '{}' not found",
            hashed_market_id, aggregate_id
        ))),
    }
}

/// Get a specific orderbook.
/// GET /orderbook/{aggregate_id}/{hashed_market_id}/{clob_token_id}
async fn get_orderbook_handler(
    State(state): State<Arc<AppState>>,
    Path((aggregate_id, hashed_market_id, clob_token_id)): Path<(String, String, String)>,
    Query(query): Query<OrderbookQuery>,
) -> Result<impl IntoResponse, ApiError> {
    match state.store.get(&aggregate_id, &hashed_market_id, &clob_token_id, query.depth) {
        Some(response) => Ok(Json(response)),
        None => Err(ApiError::NotFound(format!(
            "Orderbook for aggregate '{}', market '{}', token '{}' not found",
            aggregate_id, hashed_market_id, clob_token_id
        ))),
    }
}

/// Get all BBOs for a market within an aggregate.
/// GET /bbo/{aggregate_id}/{hashed_market_id}
async fn get_market_bbos_handler(
    State(state): State<Arc<AppState>>,
    Path((aggregate_id, hashed_market_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    match state.store.get_market_bbos(&aggregate_id, &hashed_market_id) {
        Some(response) => Ok(Json(response)),
        None => Err(ApiError::NotFound(format!(
            "Market '{}' in aggregate '{}' not found",
            hashed_market_id, aggregate_id
        ))),
    }
}

/// Get BBO for a specific token.
/// GET /bbo/{aggregate_id}/{hashed_market_id}/{clob_token_id}
async fn get_bbo_handler(
    State(state): State<Arc<AppState>>,
    Path((aggregate_id, hashed_market_id, clob_token_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    match state.store.get_bbo(&aggregate_id, &hashed_market_id, &clob_token_id) {
        Some(response) => Ok(Json(response)),
        None => Err(ApiError::NotFound(format!(
            "BBO for aggregate '{}', market '{}', token '{}' not found",
            aggregate_id, hashed_market_id, clob_token_id
        ))),
    }
}

// ============================================================================
// Error Handling
// ============================================================================

/// API error types.
#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    #[allow(dead_code)]
    InternalError(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::InternalError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(ErrorResponse { error: message });

        (status, body).into_response()
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}
