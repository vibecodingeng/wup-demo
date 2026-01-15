//! HTTP API handlers for the event service.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use external_services::polymarket::{EventData, PolymarketClient, TokenMapping};
use external_services::SharedRedisClient;
use serde::Serialize;
use std::sync::Arc;
use tracing::{error, info};

/// Application state.
#[derive(Clone)]
pub struct AppState {
    pub redis: SharedRedisClient,
    pub polymarket: PolymarketClient,
}

/// Create the API router.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        // Event endpoints (with platform prefix)
        .route("/events", get(list_events_handler))
        .route("/event/{platform}/{slug}", get(get_event_handler))
        .route("/event/{platform}/{slug}/tokens", get(get_tokens_handler))
        .route("/event/{platform}/{slug}/refresh", post(refresh_event_handler))
        .with_state(Arc::new(state))
}

// =============================================================================
// Response Types
// =============================================================================

#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

#[derive(Serialize)]
struct EventListResponse {
    /// List of platform:slug pairs
    events: Vec<String>,
    count: usize,
}

#[derive(Serialize)]
struct TokensResponse {
    platform: String,
    event_slug: String,
    tokens: Vec<TokenMapping>,
    count: usize,
}

#[derive(Serialize)]
struct RefreshResponse {
    status: String,
    platform: String,
    event_slug: String,
    token_count: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// Health check endpoint.
async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
    })
}

/// List all stored events.
async fn list_events_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<EventListResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.redis.list_events().await {
        Ok(events) => {
            let count = events.len();
            Ok(Json(EventListResponse { events, count }))
        }
        Err(e) => {
            error!("Failed to list events: {:?}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ))
        }
    }
}

/// Get event data by platform and slug.
async fn get_event_handler(
    State(state): State<Arc<AppState>>,
    Path((platform, slug)): Path<(String, String)>,
) -> Result<Json<EventData>, (StatusCode, Json<ErrorResponse>)> {
    match state.redis.get_event(&platform, &slug).await {
        Ok(Some(event)) => Ok(Json(event)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Event not found: {}:{}", platform, slug),
            }),
        )),
        Err(e) => {
            error!("Failed to get event {}:{}: {:?}", platform, slug, e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ))
        }
    }
}

/// Get token mappings for an event.
async fn get_tokens_handler(
    State(state): State<Arc<AppState>>,
    Path((platform, slug)): Path<(String, String)>,
) -> Result<Json<TokensResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.redis.get_token_mappings(&platform, &slug).await {
        Ok(tokens) => {
            if tokens.is_empty() {
                // Check if event exists
                match state.redis.event_exists(&platform, &slug).await {
                    Ok(true) => Ok(Json(TokensResponse {
                        platform,
                        event_slug: slug,
                        tokens: vec![],
                        count: 0,
                    })),
                    Ok(false) => Err((
                        StatusCode::NOT_FOUND,
                        Json(ErrorResponse {
                            error: format!("Event not found: {}:{}", platform, slug),
                        }),
                    )),
                    Err(e) => Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: e.to_string(),
                        }),
                    )),
                }
            } else {
                let count = tokens.len();
                Ok(Json(TokensResponse {
                    platform,
                    event_slug: slug,
                    tokens,
                    count,
                }))
            }
        }
        Err(e) => {
            error!("Failed to get tokens for {}:{}: {:?}", platform, slug, e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ))
        }
    }
}

/// Refresh event data from platform API.
/// Currently only supports polymarket.
async fn refresh_event_handler(
    State(state): State<Arc<AppState>>,
    Path((platform, slug)): Path<(String, String)>,
) -> Result<Json<RefreshResponse>, (StatusCode, Json<ErrorResponse>)> {
    info!("Refreshing event: {}:{}", platform, slug);

    // Currently only polymarket is supported
    if platform != "polymarket" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Unsupported platform: {}. Only 'polymarket' is supported.", platform),
            }),
        ));
    }

    // Fetch from Polymarket API
    let event_data = match state.polymarket.fetch_event_data(&slug).await {
        Ok(data) => data,
        Err(e) => {
            error!("Failed to fetch event {} from Polymarket: {:?}", slug, e);
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to fetch from Polymarket: {}", e),
                }),
            ));
        }
    };

    let token_count = event_data.tokens.len();

    // Store in Redis with platform prefix
    if let Err(e) = state.redis.store_event(&platform, &slug, &event_data).await {
        error!("Failed to store event {}:{} in Redis: {:?}", platform, slug, e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to store in Redis: {}", e),
            }),
        ));
    }

    Ok(Json(RefreshResponse {
        status: "refreshed".to_string(),
        platform,
        event_slug: slug,
        token_count,
    }))
}
