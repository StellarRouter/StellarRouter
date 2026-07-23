use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use std::net::{Ipv4Addr, SocketAddr};
use tower::ServiceExt;

use crate::{
    auth,
    auth::AuthConfig,
    handlers,
    poller::TxStatusPoller,
    rate_limit::{rate_limit_middleware, RateLimitConfig, RateLimiter},
    rpc::FeeConfig,
    state::AppState,
    types::{
        RouteDetails, SimulateRequest, SimulateResponse, StatsResponse, TransactionStatus,
        TransactionStatusEvent,
    },
};

/// Valid 56-char Stellar contract ID for use in tests.
const VALID_CONTRACT_ID: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4";

fn test_app() -> Router {
    let auth = AuthConfig {
        enabled: false,
        api_key: None,
    };

    let state = AppState::new(
        // Use a localhost port that immediately refuses connections so RPC
        // calls fail fast and the heuristic fallback is exercised.
        "http://127.0.0.1:19999".to_string(),
        "".to_string(),
        "".to_string(),
        auth,
        FeeConfig::default(),
    );

    Router::new()
        .route("/health", get(handlers::health))
        .route("/stats", get(handlers::stats))
        .route("/simulate", post(handlers::simulate))
        .route("/routes/:name", get(handlers::get_route))
        .with_state(state)
}

fn rate_limited_health_app(max_requests: u32) -> Router {
    let limiter = RateLimiter::new(RateLimitConfig {
        max_requests,
        window: std::time::Duration::from_secs(60),
    });

    Router::new()
        .route("/health", get(handlers::health))
        .route_layer(from_fn_with_state(limiter, rate_limit_middleware))
}

fn request_with_addr(path: &str, addr: SocketAddr) -> Request<Body> {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));
    request
}

fn request_with_addr_and_api_key(path: &str, addr: SocketAddr, api_key: &str) -> Request<Body> {
    let mut request = Request::builder()
        .uri(path)
        .header("x-api-key", api_key)
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));
    request
}

async fn spawn_ws_server() -> (std::net::SocketAddr, AppState) {
    use axum::routing::get;
    use tokio::net::TcpListener;

    let auth = AuthConfig {
        enabled: false,
        api_key: None,
    };

    let state = AppState::new(
        "http://localhost:1".to_string(),
        "".to_string(),
        "".to_string(),
        auth.clone(),
        FeeConfig::default(),
    );

    let app = Router::new()
        .route("/ws", get(crate::websocket::ws_handler))
        .with_state(state.clone());

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = axum::serve(listener, app);
    tokio::spawn(async move {
        let _ = server.await;
    });

    (addr, state)
}

#[tokio::test]
async fn test_ws_subscribe_broadcast_unsubscribe_and_cleanup() {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use std::time::Duration;
    use tokio::time::timeout;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as TungMessage;

    let (addr, state) = spawn_ws_server().await;
    let url = format!("ws://{}/ws", addr);

    let (ws_stream, _resp) = connect_async(&url).await.expect("connect");
    let (mut write, mut read) = ws_stream.split();

    // Subscribe to tx_id "tx123"
    let subscribe = json!({ "action": "subscribe", "tx_id": "tx123" }).to_string();
    write
        .send(TungMessage::Text(subscribe.into()))
        .await
        .unwrap();

    // Expect subscribed confirmation
    let msg = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if let TungMessage::Text(txt) = msg {
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["msg_type"], "subscribed");
    } else {
        panic!("expected text message");
    }

    // Ensure subscriber count incremented
    {
        let entry = state.tx_subscribers.get("tx123").unwrap();
        assert_eq!(*entry, 1usize);
    }

    // Broadcast an event and expect status_update
    let event = TransactionStatusEvent {
        tx_id: "tx123".to_string(),
        status: TransactionStatus::Pending,
        timestamp: "2026-06-17T00:00:00Z".to_string(),
        message: Some("ok".to_string()),
    };

    state.broadcast_status(event.clone());

    let msg = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if let TungMessage::Text(txt) = msg {
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["msg_type"], "status_update");
        assert_eq!(v["data"]["tx_id"], "tx123");
    } else {
        panic!("expected text message");
    }

    // Unsubscribe
    let unsubscribe = json!({ "action": "unsubscribe", "tx_id": "tx123" }).to_string();
    write
        .send(TungMessage::Text(unsubscribe.into()))
        .await
        .unwrap();

    // Wait for unsubscribe acknowledgment before broadcasting to avoid a race
    // between the server processing the unsubscribe and the broadcast arriving.
    let ack = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if let TungMessage::Text(txt) = ack {
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["msg_type"], "unsubscribed");
    }

    // After confirmed unsubscribe, broadcast another event and expect no message
    state.broadcast_status(event);
    let res = timeout(Duration::from_millis(200), read.next()).await;
    assert!(res.is_err(), "did not expect a message after unsubscribe");

    // Disconnect: drop write/read by closing the sink
    let _ = write.send(TungMessage::Close(None)).await;
    // Give the server a moment to process disconnect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Subscriber cleanup should have removed the entry
    assert!(state.tx_subscribers.get("tx123").is_none());
}

#[tokio::test]
async fn test_ws_multiple_subscriptions_and_duplicate_subscribe_counting() {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use std::time::Duration;
    use tokio::time::timeout;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as TungMessage;

    let (addr, state) = spawn_ws_server().await;
    let url = format!("ws://{}/ws", addr);

    let (ws_stream, _resp) = connect_async(&url).await.expect("connect");
    let (mut write, mut read) = ws_stream.split();

    // Subscribe to txA and txB
    let sub_a = json!({ "action": "subscribe", "tx_id": "txA" }).to_string();
    let sub_b = json!({ "action": "subscribe", "tx_id": "txB" }).to_string();
    write.send(TungMessage::Text(sub_a.into())).await.unwrap();
    let _ = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    write.send(TungMessage::Text(sub_b.into())).await.unwrap();
    let _ = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Broadcast events for each and ensure delivery
    let event_a = TransactionStatusEvent {
        tx_id: "txA".to_string(),
        status: TransactionStatus::Submitted,
        timestamp: "2026-06-17T00:00:01Z".to_string(),
        message: None,
    };
    let event_b = TransactionStatusEvent {
        tx_id: "txB".to_string(),
        status: TransactionStatus::Confirmed,
        timestamp: "2026-06-17T00:00:02Z".to_string(),
        message: Some("done".to_string()),
    };

    state.broadcast_status(event_a.clone());
    let msg = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if let TungMessage::Text(txt) = msg {
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["msg_type"], "status_update");
        assert_eq!(v["data"]["tx_id"], "txA");
    }

    state.broadcast_status(event_b.clone());
    let msg = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if let TungMessage::Text(txt) = msg {
        let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
        assert_eq!(v["msg_type"], "status_update");
        assert_eq!(v["data"]["tx_id"], "txB");
    }

    // Subscribe to same tx twice
    let sub_dup = json!({ "action": "subscribe", "tx_id": "dup" }).to_string();
    write
        .send(TungMessage::Text(sub_dup.clone().into()))
        .await
        .unwrap();
    let _ = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    write.send(TungMessage::Text(sub_dup.into())).await.unwrap();
    let _ = timeout(Duration::from_secs(1), read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Count should be 2
    {
        let entry = state.tx_subscribers.get("dup").unwrap();
        assert_eq!(*entry, 2usize);
    }

    // Cleanup: close connection
    let _ = write.send(TungMessage::Close(None)).await;
}

#[tokio::test]
async fn test_health_returns_200() {
    let app = test_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_health_returns_ok_body() {
    let app = test_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_rate_limiter_rejects_requests_over_limit_for_same_ip() {
    let app = rate_limited_health_app(2);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 5000));

    let first = app
        .clone()
        .oneshot(request_with_addr("/health", addr))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .clone()
        .oneshot(request_with_addr("/health", addr))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);

    let third = app
        .oneshot(request_with_addr("/health", addr))
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(third.headers().contains_key("retry-after"));

    let body = axum::body::to_bytes(third.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "rate_limit_exceeded");
}

#[tokio::test]
async fn test_rate_limiter_uses_api_key_before_remote_ip() {
    let app = rate_limited_health_app(1);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 5001));

    let api_key_a = app
        .clone()
        .oneshot(request_with_addr_and_api_key("/health", addr, "key-a"))
        .await
        .unwrap();
    assert_eq!(api_key_a.status(), StatusCode::OK);

    let api_key_b = app
        .clone()
        .oneshot(request_with_addr_and_api_key("/health", addr, "key-b"))
        .await
        .unwrap();
    assert_eq!(api_key_b.status(), StatusCode::OK);

    let repeated_api_key_a = app
        .oneshot(request_with_addr_and_api_key("/health", addr, "key-a"))
        .await
        .unwrap();
    assert_eq!(repeated_api_key_a.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_simulate_returns_200_with_valid_request() {
    let app = test_app();
    let body = json!({
        "target": VALID_CONTRACT_ID,
        "function": "transfer",
        "amount": 1_000_000,
        "fee_bps": 30,
        "network_load_bps": 5000,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_simulate_response_has_fee_fields() {
    let app = test_app();
    let body = json!({ "target": VALID_CONTRACT_ID, "function": "transfer" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: SimulateResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(parsed.estimated_fees.base_fee > 0);
    assert!(parsed.estimated_fees.total_fee >= parsed.estimated_fees.base_fee);
    assert_eq!(parsed.simulation.target, VALID_CONTRACT_ID);
    assert_eq!(parsed.simulation.function, "transfer");
}

#[tokio::test]
async fn test_simulate_surge_pricing_at_high_load() {
    let app = test_app();
    let body = json!({
        "target": VALID_CONTRACT_ID,
        "function": "transfer",
        "amount": 1_000_000,
        "network_load_bps": 9000,
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: SimulateResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(parsed.estimated_fees.high_load);
    assert_eq!(parsed.estimated_fees.surge_multiplier, 200);
}

#[tokio::test]
async fn test_simulate_missing_target_returns_400() {
    let app = test_app();
    let body = json!({ "function": "transfer" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    // Missing required field → axum returns 422 Unprocessable Entity
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn test_simulate_missing_function_returns_400() {
    let app = test_app();
    let body = json!({ "target": VALID_CONTRACT_ID });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    // Missing required field → axum returns 422 Unprocessable Entity
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn test_simulate_invalid_contract_id_returns_400() {
    let app = test_app();
    let body = json!({ "target": "not-a-valid-contract-id", "function": "transfer" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["error"].as_str().unwrap().contains("56-character"));
}

#[tokio::test]
async fn test_simulate_contract_id_not_starting_with_c_returns_400() {
    let app = test_app();
    let body = json!({
        "target": "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
        "function": "transfer",
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_simulate_invalid_function_name_returns_400() {
    let app = test_app();
    let body = json!({
        "target": VALID_CONTRACT_ID,
        "function": "bad function!",
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["error"].as_str().unwrap().contains("alphanumeric"));
}

#[tokio::test]
async fn test_simulate_long_function_name_returns_400() {
    let app = test_app();
    let body = json!({
        "target": VALID_CONTRACT_ID,
        "function": "a".repeat(65),
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_simulate_empty_body_returns_400_or_422() {
    let app = test_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn test_get_route_returns_500_when_core_not_configured() {
    let app = test_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/routes/oracle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["error"].is_string());
}

#[tokio::test]
async fn test_get_route_error_response_is_json() {
    let app = test_app();
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/routes/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.get("error").is_some());
}

#[test]
fn test_simulate_request_serialization() {
    let req = SimulateRequest {
        target: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4".to_string(),
        function: "transfer".to_string(),
        amount: 1_000_000,
        fee_bps: 30,
        network_load_bps: 0,
        route_details: Some(RouteDetails {
            name: "swap".to_string(),
            version: Some(1),
            expected_outputs: Some(vec!["1000000".to_string()]),
        }),
    };

    let json = serde_json::to_string(&req).unwrap();
    let deserialized: SimulateRequest = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.target, req.target);
    assert_eq!(deserialized.function, req.function);
}

#[tokio::test]
async fn test_ws_oversized_frame_closes_connection() {
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio::time::timeout;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as TungMessage;

    let (addr, _state) = spawn_ws_server().await;
    let url = format!("ws://{}/ws", addr);

    let (ws_stream, _resp) = connect_async(&url).await.expect("connect");
    let (mut write, mut read) = ws_stream.split();

    // Send a message well over the 4 KB limit (8 KB of padding).
    let oversized = "x".repeat(8 * 1024);
    let send_result = write.send(TungMessage::Text(oversized.into())).await;

    // The send itself may succeed (buffered), but the server should close the
    // connection shortly after receiving the oversized frame.
    if send_result.is_err() {
        // Connection already closed — acceptable.
        return;
    }

    // Read from the stream: the server should close (None) or return an error.
    let next = timeout(Duration::from_secs(3), read.next()).await;
    match next {
        Ok(Some(Err(_))) | Ok(None) => {
            // Connection was closed/reset by server — expected behavior.
        }
        Ok(Some(Ok(TungMessage::Close(_)))) => {
            // Clean close frame — also acceptable.
        }
        _ => {
            panic!("expected connection to be closed after oversized frame");
        }
    }
}

#[test]
fn test_transaction_status_event_serialization() {
    let event = TransactionStatusEvent {
        tx_id: "tx_12345".to_string(),
        status: TransactionStatus::Pending,
        timestamp: "2026-05-28T00:00:00Z".to_string(),
        message: Some("waiting".to_string()),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: TransactionStatusEvent = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.tx_id, event.tx_id);
    assert_eq!(deserialized.status, event.status);
    assert_eq!(deserialized.timestamp, event.timestamp);
    assert_eq!(deserialized.message, event.message);
}

// Keep all the tests below

#[tokio::test]
async fn test_stats_returns_200() {
    // ...
}

#[tokio::test]
async fn test_stats_response_has_expected_fields() {
    // ...
}

// ─────────────────────────────────────────────────────────────────────────────
// Poller integration test helpers
//
// Spins up a minimal fake Soroban RPC server (an axum Router) that responds to
// `getTransaction` with a SUCCESS result, then verifies that a subscribed
// WebSocket client receives a real `status_update` event sourced from the
// poller — not from the test helper `broadcast_status()`.
// ─────────────────────────────────────────────────────────────────────────────

/// Spawn a minimal JSON-RPC stub that answers `getTransaction` for any hash
/// with a configurable status string.  All other methods receive a JSON-RPC
/// error so that tests that accidentally call them will fail loudly.
#[tokio::test]
async fn test_stats_reflects_active_subscriptions() {
    // ...
}

async fn spawn_fake_rpc_server(tx_status: &'static str) -> String {
    // ...
}

async fn spawn_ws_server_with_rpc(rpc_url: String) -> (SocketAddr, AppState) {
    use axum::routing::get;
    use tokio::net::TcpListener;

    let auth = AuthConfig {
        enabled: false,
        api_key: None,
    };

    let state = AppState::new(
        rpc_url,
        "".to_string(),
        "".to_string(),
        auth,
        FeeConfig::default(),
    );

    let app = Router::new()
        .route("/ws", get(crate::websocket::ws_handler))
        .with_state(state.clone());

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (addr, state)
}

#[tokio::test]
async fn test_stats_reflects_active_subscriptions() {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use std::time::Duration;
    use tokio::time::timeout;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as TungMessage;
    use axum::routing::get;
    use tokio::net::TcpListener;

    // Build a server that exposes both /ws and /stats.
    let state = AppState::new(
        "http://localhost:1".to_string(),
        "".to_string(),
        "".to_string(),
        AuthConfig { enabled: false, api_key: None },
        FeeConfig::default(),
    );

    let app = Router::new()
        .route("/ws", get(crate::websocket::ws_handler))
        .route("/stats", get(handlers::stats))
        .with_state(state);

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Connect via WebSocket and subscribe to two distinct tx IDs.
    let (ws_stream, _) = connect_async(format!("ws://{}/ws", addr))
        .await
        .expect("connect");
    let (mut write, mut read) = ws_stream.split();

    for tx_id in &["txAlpha", "txBeta"] {
        let sub = json!({ "action": "subscribe", "tx_id": tx_id }).to_string();
        write.send(TungMessage::Text(sub.into())).await.unwrap();
        // Drain the confirmation message.
        let _ = timeout(Duration::from_secs(1), read.next()).await;
    }

    // Poll /stats via a plain HTTP request.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/stats", addr))
        .send()
        .await
        .expect("stats request failed");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["active_subscriptions"], 2);
    assert_eq!(body["unique_tx_ids"], 2);
    assert_eq!(body["broadcast_channel_capacity"], crate::state::BROADCAST_CHANNEL_CAPACITY);
    // ...
}

#[tokio::test]
async fn test_poller_delivers_status_update_to_ws_client() {
    // ...
}

#[tokio::test]
async fn test_poller_keeps_polling_non_terminal_transactions() {
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };
    use std::time::Duration;

    // Fake RPC that returns PENDING (non-terminal) and counts calls.
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    use axum::{routing::post, Json as AxumJson, Router};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let rpc_addr = listener.local_addr().unwrap();

    let app = Router::new().route(
        "/",
        post(move |AxumJson(body): AxumJson<Value>| {
            let counter = call_count_clone.clone();
            async move {
                let id = body.get("id").cloned().unwrap_or(json!(1));
                counter.fetch_add(1, Ordering::SeqCst);
                AxumJson(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "status": "NOT_FOUND" }
                }))
            }
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let rpc_url = format!("http://{}", rpc_addr);
    let auth = AuthConfig {
        enabled: false,
        api_key: None,
    };
    let state = AppState::new(rpc_url, "".into(), "".into(), auth, FeeConfig::default());

    // Register one subscription manually (simulates a WS client subscribing).
    state.add_subscriber("pending_tx".to_string());

    // Spawn poller with a 50 ms interval.
    let poller = TxStatusPoller::with_interval_ms(state.clone(), 50);
    tokio::spawn(async move { poller.run().await });

    // Wait ~300 ms — should see at least 3 poll rounds.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let count = call_count.load(Ordering::SeqCst);
    assert!(
        count >= 3,
        "expected at least 3 poll calls for a non-terminal tx, got {}",
        count
    );

    // tx should still be in the subscribers map (not removed for non-terminal).
    assert!(
        state.tx_subscribers.get("pending_tx").is_some(),
        "non-terminal tx should still be in tx_subscribers"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Auth + rate-limit ordering tests
//
// Verify that the rate limiter is the outermost middleware layer, so requests
// with an invalid (or missing) API key are counted against the rate limit
// before auth rejection.  Without the correct ordering an attacker could
// brute-force ROUTER_API_KEY with unlimited attempts per second.
// ─────────────────────────────────────────────────────────────────────────────

/// Build a minimal router that has both rate limiting (outermost) and API-key
/// auth (innermost), mirroring the production layer order in main.rs.
fn auth_and_rate_limited_app(max_requests: u32, valid_key: &'static str) -> Router {
    use crate::auth::AuthConfig;
    use axum::middleware::from_fn_with_state;

    let limiter = RateLimiter::new(RateLimitConfig {
        max_requests,
        window: std::time::Duration::from_secs(60),
    });
    let auth = AuthConfig {
        enabled: true,
        api_key: Some(valid_key.to_string()),
    };

    Router::new()
        .route("/health", get(handlers::health))
        // auth is innermost — added first
        .route_layer(from_fn_with_state(auth, auth::auth_middleware))
        // rate limiter is outermost — added last, so it runs first
        .route_layer(from_fn_with_state(limiter, rate_limit_middleware))
}

#[tokio::test]
async fn test_rate_limit_applies_to_invalid_api_key_requests() {
    // Allow only 2 requests before throttling kicks in.
    let app = auth_and_rate_limited_app(2, "correct-key");
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 6000));

    // First two requests with a wrong key: rate limiter passes them through,
    // auth rejects them with 401/403 — but they still consume quota.
    let r1 = app
        .clone()
        .oneshot(request_with_addr_and_api_key("/health", addr, "wrong-key"))
        .await
        .unwrap();
    assert!(
        r1.status() == StatusCode::UNAUTHORIZED || r1.status() == StatusCode::FORBIDDEN,
        "expected auth rejection on attempt 1, got {}",
        r1.status()
    );

    let r2 = app
        .clone()
        .oneshot(request_with_addr_and_api_key("/health", addr, "wrong-key"))
        .await
        .unwrap();
    assert!(
        r2.status() == StatusCode::UNAUTHORIZED || r2.status() == StatusCode::FORBIDDEN,
        "expected auth rejection on attempt 2, got {}",
        r2.status()
    );

    // Third request must be throttled by the rate limiter before auth runs.
    let r3 = app
        .oneshot(request_with_addr_and_api_key("/health", addr, "wrong-key"))
        .await
        .unwrap();
    assert_eq!(
        r3.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "expected 429 after exceeding rate limit with invalid key, got {}",
        r3.status()
    );
    assert!(
        r3.headers().contains_key("retry-after"),
        "429 response must include a Retry-After header"
    );

    let body = axum::body::to_bytes(r3.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["error"], "rate_limit_exceeded",
        "429 body must carry error=rate_limit_exceeded"
    );
}

#[tokio::test]
async fn test_valid_key_still_works_after_invalid_key_is_rate_limited() {
    // The rate limiter keys by client identifier (IP or x-api-key header value).
    // A separate valid key from a different "client" must not be affected by
    // the wrong-key client exhausting its quota.
    let app = auth_and_rate_limited_app(1, "correct-key");
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 6001));

    // Exhaust the wrong-key bucket.
    let _ = app
        .clone()
        .oneshot(request_with_addr_and_api_key("/health", addr, "wrong-key"))
        .await
        .unwrap();
    let throttled = app
        .clone()
        .oneshot(request_with_addr_and_api_key("/health", addr, "wrong-key"))
        .await
        .unwrap();
    assert_eq!(throttled.status(), StatusCode::TOO_MANY_REQUESTS);

    // A request with the correct key uses a distinct rate-limit bucket and
    // should succeed.
    let ok = app
        .oneshot(request_with_addr_and_api_key("/health", addr, "correct-key"))
        .await
        .unwrap();
    assert_eq!(
        ok.status(),
        StatusCode::OK,
        "valid key should succeed even after wrong-key bucket is exhausted"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Log injection / input sanitization tests
//
// These tests confirm that attacker-controlled fields containing newlines,
// carriage returns, or other ASCII control characters do not reach the log
// subscriber unescaped.  The handler-level checks are:
//
//   • GET /routes/:name — validate_route_name rejects the request (422) before
//     the route name is ever passed to `info!()`.
//   • POST /simulate — `function` is sanitized via `sanitize_for_log` before
//     logging; the request still proceeds normally (function is not shape-
//     validated beyond non-empty).
//
// The unit-level guarantee (that sanitize_for_log itself strips control chars)
// lives in router-off-chain-common/src/logging.rs.
// ─────────────────────────────────────────────────────────────────────────────

/// A route name containing a newline would let an attacker forge an additional
/// log line.  The handler must reject it with 422 before reaching `info!()`.
#[tokio::test]
async fn test_get_route_with_newline_in_name_is_rejected() {
    let app = test_app();

    // URL-encode the newline so it arrives as a literal '\n' in the Path extractor.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/routes/oracle%0aINFO%20fake_log_entry")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "route name with embedded newline must be rejected before logging"
    );

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("invalid route name"),
        "error body should describe the validation failure; got: {}",
        json["error"]
    );
}

/// A route name containing a carriage return must also be rejected.
#[tokio::test]
async fn test_get_route_with_carriage_return_in_name_is_rejected() {
    let app = test_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/routes/oracle%0dINFO%20fake")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// A route name containing an ASCII ESC (0x1B) — used in ANSI terminal escape
/// sequences — must be rejected before it reaches the log.
#[tokio::test]
async fn test_get_route_with_escape_sequence_in_name_is_rejected() {
    let app = test_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/routes/oracle%1b%5b31mred%1b%5b0m") // ESC[31m … ESC[0m
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// Injecting a newline into the `function` field of a simulate request must
/// not produce a forged log line.  The handler sanitizes the value before
/// logging and still returns a valid response (function is not shape-validated).
#[tokio::test]
async fn test_simulate_with_newline_in_function_does_not_forge_log() {
    use router_off_chain_common::logging::sanitize_for_log;

    // Confirm at the unit level that the sanitizer strips the newline.
    let malicious = "transfer\nINFO  forged_log_entry level=ERROR";
    let sanitized = sanitize_for_log(malicious);
    assert!(
        !sanitized.contains('\n'),
        "sanitize_for_log must remove newlines; got: {:?}",
        sanitized
    );
    // The forged portion is still present as visible text but cannot split into
    // a new log line.
    assert!(sanitized.contains('\u{240A}')); // ␊ — LINE FEED symbol

    // Also confirm the HTTP handler accepts (not rejects) the request —
    // sanitization is applied at the logging site, not as an input gate.
    let app = test_app();
    let body = json!({
        "target": VALID_CONTRACT_ID,
        "function": malicious,
        "amount": 0,
        "fee_bps": 0,
        "network_load_bps": 0,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/simulate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // The simulate handler doesn't gate on function content — it should still
    // return 200 (heuristic path) or 500 (RPC unreachable in test).  Either
    // way it must NOT return 400/422 for this input.
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
        "simulate should not reject a request purely because `function` contains control chars; \
         got {}",
        resp.status()
    );
}

/// A `function` field containing only control characters (e.g. a bare newline)
/// is still a non-empty string, so it passes the non-empty check.  Verify
/// sanitize_for_log handles it without panicking.
#[tokio::test]
async fn test_simulate_function_all_control_chars_sanitized() {
    use router_off_chain_common::logging::sanitize_for_log;

    let all_controls: String = (0u8..=0x1Fu8).map(char::from).collect();
    let sanitized = sanitize_for_log(&all_controls);

    // No original control characters must remain.
    assert!(
        !sanitized.chars().any(|c| c.is_ascii_control()),
        "sanitized output must contain no ASCII control characters; got: {:?}",
        sanitized
    );
    // Every input character should have been replaced by a control-picture glyph.
    assert_eq!(sanitized.chars().count(), all_controls.chars().count());
}
    // ...
}
