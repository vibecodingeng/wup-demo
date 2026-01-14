//! HTTP API handlers for the event service.

use crate::redis_client::RedisClient;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post},
    Router,
};
use external_services::aggregate::AggregateMetadata;
use external_services::polymarket::{EventData, PolymarketClient, TokenMapping};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};

/// Application state.
#[derive(Clone)]
pub struct AppState {
    pub redis: RedisClient,
    pub polymarket: PolymarketClient,
}

/// Create the API router.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        // Event endpoints (now with platform prefix)
        .route("/events", get(list_events_handler))
        .route("/event/{platform}/{slug}", get(get_event_handler))
        .route("/event/{platform}/{slug}/tokens", get(get_tokens_handler))
        .route("/event/{platform}/{slug}/refresh", post(refresh_event_handler))
        // Aggregate endpoints
        .route("/aggregates", get(list_aggregates_handler))
        .route("/aggregate", post(create_aggregate_handler))
        .route("/aggregate/{aggregate_id}", get(get_aggregate_handler))
        .route("/aggregate/{aggregate_id}", delete(delete_aggregate_handler))
        .route("/aggregate/{aggregate_id}/map", post(map_event_handler))
        .route("/aggregate/{aggregate_id}/map/{platform}/{slug}", delete(unmap_event_handler))
        .route("/mapping/{platform}/{slug}", get(get_mapping_handler))
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
// Aggregate Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
struct CreateAggregateRequest {
    aggregate_id: String,
    name: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MapEventRequest {
    platform: String,
    slug: String,
}

#[derive(Serialize)]
struct AggregateListResponse {
    aggregates: Vec<String>,
    count: usize,
}

#[derive(Serialize)]
struct MappingResponse {
    platform: String,
    slug: String,
    aggregate_id: Option<String>,
}

#[derive(Serialize)]
struct MapEventResponse {
    status: String,
    aggregate_id: String,
    platform: String,
    slug: String,
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

// =============================================================================
// Aggregate Handlers
// =============================================================================

/// List all aggregates.
async fn list_aggregates_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AggregateListResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.redis.list_aggregates().await {
        Ok(aggregates) => {
            let count = aggregates.len();
            Ok(Json(AggregateListResponse { aggregates, count }))
        }
        Err(e) => {
            error!("Failed to list aggregates: {:?}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ))
        }
    }
}

/// Create a new aggregate.
async fn create_aggregate_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateAggregateRequest>,
) -> Result<Json<AggregateMetadata>, (StatusCode, Json<ErrorResponse>)> {
    info!("Creating aggregate: {}", req.aggregate_id);

    let metadata = AggregateMetadata {
        aggregate_id: req.aggregate_id.clone(),
        name: req.name,
        description: req.description,
        platforms: vec![],
        created_at: chrono::Utc::now().timestamp_millis(),
    };

    if let Err(e) = state.redis.store_aggregate(&metadata).await {
        error!("Failed to create aggregate {}: {:?}", req.aggregate_id, e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to create aggregate: {}", e),
            }),
        ));
    }

    Ok(Json(metadata))
}

/// Get aggregate by ID.
async fn get_aggregate_handler(
    State(state): State<Arc<AppState>>,
    Path(aggregate_id): Path<String>,
) -> Result<Json<AggregateMetadata>, (StatusCode, Json<ErrorResponse>)> {
    match state.redis.get_aggregate(&aggregate_id).await {
        Ok(Some(aggregate)) => Ok(Json(aggregate)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Aggregate not found: {}", aggregate_id),
            }),
        )),
        Err(e) => {
            error!("Failed to get aggregate {}: {:?}", aggregate_id, e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ))
        }
    }
}

/// Delete an aggregate.
async fn delete_aggregate_handler(
    State(state): State<Arc<AppState>>,
    Path(aggregate_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    info!("Deleting aggregate: {}", aggregate_id);

    if let Err(e) = state.redis.delete_aggregate(&aggregate_id).await {
        error!("Failed to delete aggregate {}: {:?}", aggregate_id, e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to delete aggregate: {}", e),
            }),
        ));
    }

    Ok(Json(serde_json::json!({
        "status": "deleted",
        "aggregate_id": aggregate_id
    })))
}

/// Map a platform event slug to an aggregate.
async fn map_event_handler(
    State(state): State<Arc<AppState>>,
    Path(aggregate_id): Path<String>,
    Json(req): Json<MapEventRequest>,
) -> Result<Json<MapEventResponse>, (StatusCode, Json<ErrorResponse>)> {
    info!(
        "Mapping {}:{} -> aggregate {}",
        req.platform, req.slug, aggregate_id
    );

    // Verify aggregate exists
    match state.redis.get_aggregate(&aggregate_id).await {
        Ok(Some(mut metadata)) => {
            // Add platform to list if not already present
            if !metadata.platforms.contains(&req.platform) {
                metadata.platforms.push(req.platform.clone());
                if let Err(e) = state.redis.store_aggregate(&metadata).await {
                    error!("Failed to update aggregate platforms: {:?}", e);
                }
            }
        }
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("Aggregate not found: {}", aggregate_id),
                }),
            ));
        }
        Err(e) => {
            error!("Failed to get aggregate {}: {:?}", aggregate_id, e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ));
        }
    }

    // Create the mapping
    if let Err(e) = state
        .redis
        .set_event_mapping(&req.platform, &req.slug, &aggregate_id)
        .await
    {
        error!(
            "Failed to create mapping {}:{} -> {}: {:?}",
            req.platform, req.slug, aggregate_id, e
        );
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to create mapping: {}", e),
            }),
        ));
    }

    Ok(Json(MapEventResponse {
        status: "mapped".to_string(),
        aggregate_id,
        platform: req.platform,
        slug: req.slug,
    }))
}

/// Remove a mapping from platform event slug to aggregate.
async fn unmap_event_handler(
    State(state): State<Arc<AppState>>,
    Path((aggregate_id, platform, slug)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    info!(
        "Unmapping {}:{} from aggregate {}",
        platform, slug, aggregate_id
    );

    if let Err(e) = state.redis.delete_event_mapping(&platform, &slug).await {
        error!(
            "Failed to delete mapping {}:{}: {:?}",
            platform, slug, e
        );
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to delete mapping: {}", e),
            }),
        ));
    }

    Ok(Json(serde_json::json!({
        "status": "unmapped",
        "aggregate_id": aggregate_id,
        "platform": platform,
        "slug": slug
    })))
}

/// Get aggregate ID mapping for a platform event slug.
async fn get_mapping_handler(
    State(state): State<Arc<AppState>>,
    Path((platform, slug)): Path<(String, String)>,
) -> Result<Json<MappingResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.redis.get_aggregate_id(&platform, &slug).await {
        Ok(aggregate_id) => Ok(Json(MappingResponse {
            platform,
            slug,
            aggregate_id,
        })),
        Err(e) => {
            error!("Failed to get mapping {}:{}: {:?}", platform, slug, e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ))
        }
    }
}
