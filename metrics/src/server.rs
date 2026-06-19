//! HTTP server that exposes the `/metrics`, `/health`, and `/ready` endpoints.
//!
//! Uses `axum` to serve:
//! - `GET /metrics` — Prometheus text format metrics
//! - `GET /health`  — simple liveness probe (returns `200 OK`)
//! - `GET /ready`   — readiness probe (returns `200 OK` if router_up == 1, `503` otherwise)
//! - `GET /swagger-ui/` — Swagger UI for API documentation
//! - `GET /api-docs/openapi.json` — OpenAPI specification
//!
//! Every request is assigned a unique `request_id` that is logged and returned
//! in the `X-Request-Id` response header.

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::{header, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use prometheus::{Encoder, Registry, TextEncoder};
use std::net::SocketAddr;
use tracing::{info, info_span, Instrument};

use crate::auth::AuthConfig;
use crate::logging::new_request_id;
use crate::openapi::ApiDoc;
use crate::rate_limit::{rate_limit_middleware, RateLimiter};
use crate::replay_protection::{NonceCache, ReplayProtectionConfig};

/// Shared server state.
#[derive(Clone)]
struct AppState {
    registry: Registry,
}

/// Start the HTTP server and block until it exits.
pub async fn serve(listen: String, registry: Registry, limiter: RateLimiter) -> Result<()> {
    let addr: SocketAddr = listen
        .parse()
        .with_context(|| format!("invalid listen address: {listen}"))?;

    let state = AppState { registry };
    let auth_config = AuthConfig::from_env();
    let replay_config = ReplayProtectionConfig::from_env();
    let nonce_cache = NonceCache::new(replay_config);

    // Routes that require auth + rate limiting
    let protected = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/api-docs/openapi.json", get(openapi_handler))
        .layer(middleware::from_fn_with_state(
            limiter,
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            nonce_cache,
            crate::replay_protection::replay_protection_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            auth_config,
            crate::auth::auth_middleware,
        ));

    // Health/ready probes must be reachable without credentials
    let public = Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler));

    let app = Router::new()
        .merge(protected)
        .merge(public)
        .layer(middleware::from_fn(request_id_middleware))
        .with_state(state);

    info!(%addr, "HTTP server listening");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    axum::serve(listener, app)
        .await
        .context("HTTP server error")?;

    Ok(())
}

// ── Middleware ────────────────────────────────────────────────────────────────

/// Attach a unique `request_id` to every request span and response header.
async fn request_id_middleware(req: Request<axum::body::Body>, next: Next) -> Response {
    let request_id = new_request_id();
    let method = req.method().clone();
    let uri = req.uri().clone();

    let span = info_span!(
        "http_request",
        request_id = %request_id,
        method = %method,
        uri = %uri,
    );

    async move {
        info!(request_id = %request_id, %method, %uri, "incoming request");
        let mut response = next.run(req).await;
        let status = response.status().as_u16();
        info!(request_id = %request_id, status, "request complete");

        if let Ok(val) = HeaderValue::from_str(&request_id) {
            response.headers_mut().insert("x-request-id", val);
        }
        response
    }
    .instrument(span)
    .await
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /metrics` — render all registered metrics in Prometheus text format.
///
/// Returns Prometheus-formatted metrics for all registered gauges, counters, and histograms.
#[utoipa::path(
    get,
    path = "/metrics",
    tag = "metrics",
    responses(
        (status = 200, description = "Prometheus metrics in text format", content_type = "text/plain; version=0.0.4"),
        (status = 500, description = "Failed to encode metrics"),
    ),
)]
async fn metrics_handler(State(state): State<AppState>) -> Response {
    let encoder = TextEncoder::new();
    let metric_families = state.registry.gather();

    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to encode metrics: {e}"),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, encoder.format_type())],
        buffer,
    )
        .into_response()
}

/// `GET /health` — simple liveness probe.
///
/// Returns 200 OK if the exporter is running.
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Exporter is alive"),
    ),
)]
async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// `GET /ready` — readiness probe.
#[utoipa::path(get, path = "/ready", tag = "health",
    responses((status = 200, description = "Ready"), (status = 503, description = "Not ready")))]
async fn ready_handler(State(state): State<AppState>) -> impl IntoResponse {
    // Check if router_up metric is 1
    let metric_families = state.registry.gather();

    let router_up = metric_families
        .iter()
        .find(|mf| mf.get_name() == "router_up")
        .and_then(|mf| mf.get_metric().first())
        .and_then(|m| {
            if m.has_gauge() {
                Some(m.get_gauge().get_value())
            } else {
                None
            }
        })
        .unwrap_or(0.0);

    if router_up >= 1.0 {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

/// `GET /api-docs/openapi.json` — OpenAPI specification.
async fn openapi_handler() -> impl IntoResponse {
    use utoipa::OpenApi;
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::util::ServiceExt; // for `oneshot`

    fn make_app() -> Router {
        let registry = Registry::new();
        // Register a test gauge so the /metrics output is non-empty
        let gauge = prometheus::Gauge::new("test_gauge", "a test gauge").unwrap();
        registry.register(Box::new(gauge.clone())).unwrap();
        gauge.set(42.0);

        let state = AppState { registry };
        Router::new()
            .route("/metrics", get(metrics_handler))
            .route("/health", get(health_handler))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_health_returns_200() {
        let app = make_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_metrics_returns_200_with_prometheus_content_type() {
        let app = make_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("text/plain"),
            "expected text/plain content type, got: {content_type}"
        );
    }

    #[tokio::test]
    async fn test_metrics_body_contains_gauge() {
        use axum::body::to_bytes;

        let app = make_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(
            body_str.contains("test_gauge"),
            "expected test_gauge in metrics output"
        );
        assert!(
            body_str.contains("42"),
            "expected gauge value 42 in metrics output"
        );
    }

    #[tokio::test]
    async fn test_ready_returns_503_when_router_up_is_zero() {
        let registry = Registry::new();
        // Register router_up gauge and set it to 0
        let gauge = prometheus::Gauge::new("router_up", "exporter health").unwrap();
        registry.register(Box::new(gauge.clone())).unwrap();
        gauge.set(0.0);

        let state = AppState { registry };
        let app = Router::new()
            .route("/ready", get(ready_handler))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_ready_returns_200_when_router_up_is_one() {
        let registry = Registry::new();
        // Register router_up gauge and set it to 1
        let gauge = prometheus::Gauge::new("router_up", "exporter health").unwrap();
        registry.register(Box::new(gauge.clone())).unwrap();
        gauge.set(1.0);

        let state = AppState { registry };
        let app = Router::new()
            .route("/ready", get(ready_handler))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
