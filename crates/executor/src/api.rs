//! HTTP API for the executor service.
//!
//! Endpoints:
//! - `POST /order` - Smart routing order (finds best platform) - supports both limit and market orders
//! - `POST /order/:platform` - Direct platform order - supports both limit and market orders
//! - `DELETE /order/:platform/:order_id` - Cancel order
//! - `GET /orders/:platform` - List open orders
//! - `GET /health` - Health check
//!
//! # Order Types
//!
//! Use the unified `/order` endpoint for both limit and market orders:
//!
//! - **Market orders**: Omit `order_type` (defaults to `Market`) and provide `amount`.
//!   - BUY: amount is in USDC
//!   - SELL: amount is in shares
//!
//! - **Limit orders**: Set `order_type` to `GTC`, `GTD`, or `FOK` and provide `price` and `size`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::error;

use crate::types::OrderRequest;
use crate::{router::SmartRouter, types::OrderSide};

/// Application state shared across handlers.
pub struct AppState {
    pub router: SmartRouter,
}

/// Create the HTTP router.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        // Unified order endpoint - handles both limit and market orders
        .route("/order", post(smart_order_handler))
        .route("/order/{platform}", post(direct_order_handler))
        // Order management
        .route("/order/{platform}/{order_id}", delete(cancel_order_handler))
        .route("/orders/{platform}", get(list_orders_handler))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

/// Health check response.
#[derive(Serialize)]
struct HealthResponse {
    status: String,
    platforms: std::collections::HashMap<String, bool>,
    cached_markets: usize,
}

/// Health check handler.
async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let platforms = state.router.health_check().await;
    let response = HealthResponse {
        status: "ok".to_string(),
        platforms,
        cached_markets: state.router.bbo_cache().market_count(),
    };
    Json(response)
}

/// API error response.
#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    code: String,
}

impl ErrorResponse {
    fn new(error: impl ToString, code: impl ToString) -> Self {
        Self {
            error: error.to_string(),
            code: code.to_string(),
        }
    }
}

/// Smart order routing - finds best platform automatically.
/// POST /order
async fn smart_order_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<OrderRequest>,
) -> impl IntoResponse {
    if req.side == OrderSide::Buy && req.amount.unwrap_or_default() > Decimal::from(5) {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::to_value(ErrorResponse::new(
                    "Amount must be less than 5",
                    "INVALID_AMOUNT",
                ))
                .unwrap(),
            ),
        );
    }
    match state.router.submit_order(req).await {
        Ok(response) => (
            StatusCode::OK,
            Json(serde_json::to_value(response).unwrap()),
        ),
        Err(e) => {
            error!("Smart order failed: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::to_value(ErrorResponse::new(e.to_string(), "ORDER_FAILED"))
                        .unwrap(),
                ),
            )
        }
    }
}

/// Direct platform order - bypasses smart routing.
/// POST /order/:platform
async fn direct_order_handler(
    State(state): State<Arc<AppState>>,
    Path(platform): Path<String>,
    Json(mut req): Json<OrderRequest>,
) -> impl IntoResponse {
    // Set platform explicitly to bypass smart routing
    req.platform = Some(platform);

    match state.router.submit_order(req).await {
        Ok(response) => (
            StatusCode::OK,
            Json(serde_json::to_value(response).unwrap()),
        ),
        Err(e) => {
            error!("Direct order failed: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::to_value(ErrorResponse::new(e.to_string(), "ORDER_FAILED"))
                        .unwrap(),
                ),
            )
        }
    }
}

/// Cancel order path parameters.
#[derive(Deserialize)]
struct CancelOrderPath {
    platform: String,
    order_id: String,
}

/// Cancel order handler.
/// DELETE /order/:platform/:order_id
async fn cancel_order_handler(
    State(state): State<Arc<AppState>>,
    Path(CancelOrderPath { platform, order_id }): Path<CancelOrderPath>,
) -> impl IntoResponse {
    match state.router.cancel_order(&platform, &order_id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "cancelled", "order_id": order_id})),
        ),
        Err(e) => {
            error!("Cancel order failed: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::to_value(ErrorResponse::new(e.to_string(), "CANCEL_FAILED"))
                        .unwrap(),
                ),
            )
        }
    }
}

/// List open orders handler.
/// GET /orders/:platform
async fn list_orders_handler(
    State(state): State<Arc<AppState>>,
    Path(platform): Path<String>,
) -> impl IntoResponse {
    match state.router.get_open_orders(&platform).await {
        Ok(orders) => (StatusCode::OK, Json(serde_json::to_value(orders).unwrap())),
        Err(e) => {
            error!("List orders failed: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::to_value(ErrorResponse::new(e.to_string(), "LIST_FAILED")).unwrap(),
                ),
            )
        }
    }
}
