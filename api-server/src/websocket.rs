use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::{
    broadcast::{error::RecvError, Receiver},
    mpsc::Sender,
};
use tracing::{error, info, warn};

use crate::{
    state::AppState,
    types::{SubscribeMessage, TransactionStatusEvent},
};

const DEFAULT_WS_FAN_IN_CAPACITY: usize = 1000;
const WS_FAN_IN_CAPACITY_ENV: &str = "WS_FAN_IN_CHANNEL_CAPACITY";

/// WebSocket upgrade handler
pub async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    info!("WebSocket client connected");

    // Fan-in channel: one relay task per subscription forwards events here.
    let fan_in_capacity = websocket_fan_in_capacity();
    let (fan_in_tx, mut fan_in_rx) =
        tokio::sync::mpsc::channel::<TransactionStatusEvent>(fan_in_capacity);
    info!(
        capacity = fan_in_capacity,
        env = WS_FAN_IN_CAPACITY_ENV,
        "WebSocket fan-in channel initialized"
    );

    // Cancellation tokens so we can stop relay tasks on unsubscribe.
    let mut subscriptions: Vec<(String, tokio_util::sync::CancellationToken)> = Vec::new();

    loop {
        tokio::select! {
            biased; // always drain inbound client messages before outbound events

            // ── inbound client messages ──────────────────────────────────────
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<SubscribeMessage>(&text) {
                            Ok(sub_msg) => {
                                if sub_msg.action == "subscribe" {
                                    info!("Client subscribed to tx_id: {}", sub_msg.tx_id);

                                    let already = subscriptions.iter().any(|(id, _)| id == &sub_msg.tx_id);
                                    if !already {
                                        let cancel = tokio_util::sync::CancellationToken::new();
                                        subscriptions.push((sub_msg.tx_id.clone(), cancel.clone()));
                                        state.add_subscriber(sub_msg.tx_id.clone());

                                        let mut rx: Receiver<TransactionStatusEvent> =
                                            state.tx_status_tx.subscribe();
                                        let tx_id_filter = sub_msg.tx_id.clone();
                                        let fan_in = fan_in_tx.clone();
                                        tokio::spawn(async move {
                                            loop {
                                                tokio::select! {
                                                    biased;
                                                    _ = cancel.cancelled() => break,
                                                    result = rx.recv() => match result {
                                                        Ok(event) if event.tx_id == tx_id_filter => {
                                                            warn_if_fan_in_near_capacity(&fan_in, &tx_id_filter);
                                                            if fan_in.send(event).await.is_err() {
                                                                break;
                                                            }
                                                        }
                                                        Ok(_) => {}
                                                        Err(RecvError::Lagged(n)) => {
                                                            warn!(
                                                                "WS relay for {} lagged ({} skipped)",
                                                                tx_id_filter, n
                                                            );
                                                        }
                                                        Err(RecvError::Closed) => break,
                                                    }
                                                }
                                            }
                                        });
                                    } else {
                                        // Duplicate subscribe: bump counter only.
                                        state.add_subscriber(sub_msg.tx_id.clone());
                                    }

                                    let response = json!({
                                        "msg_type": "subscribed",
                                        "data": {
                                            "tx_id": sub_msg.tx_id,
                                            "status": "subscribed",
                                        },
                                    });

                                    if let Err(e) = sender
                                        .send(Message::Text(response.to_string()))
                                        .await
                                    {
                                        error!("Failed to send subscription confirmation: {}", e);
                                        break;
                                    }
                                } else if sub_msg.action == "unsubscribe" {
                                    info!("Client unsubscribed from tx_id: {}", sub_msg.tx_id);

                                    if let Some(pos) = subscriptions.iter().position(|(id, _)| id == &sub_msg.tx_id) {
                                        let (_, cancel) = subscriptions.remove(pos);
                                        cancel.cancel();
                                    }
                                    state.remove_subscriber(&sub_msg.tx_id);

                                    // Send ack so the client can synchronize before
                                    // checking that no further events arrive.
                                    let response = json!({
                                        "msg_type": "unsubscribed",
                                        "data": { "tx_id": sub_msg.tx_id },
                                    });
                                    if let Err(e) = sender
                                        .send(Message::Text(response.to_string()))
                                        .await
                                    {
                                        error!("Failed to send unsubscribe ack: {}", e);
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse WebSocket message: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("WebSocket client disconnected");
                        for (tx_id, cancel) in &subscriptions {
                            cancel.cancel();
                            state.remove_subscriber(tx_id);
                        }
                        break;
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // ── outbound events from relay tasks ─────────────────────────────
            Some(event) = fan_in_rx.recv() => {
                // Guard: only forward if the subscription is still active.
                let still_subscribed = subscriptions.iter().any(|(id, _)| id == &event.tx_id);
                if still_subscribed {
                    let response = json!({
                        "msg_type": "status_update",
                        "data": {
                            "tx_id": event.tx_id,
                            "status": event.status,
                            "timestamp": event.timestamp,
                            "message": event.message,
                        },
                    });

                    if let Err(e) = sender.send(Message::Text(response.to_string())).await {
                        error!("Failed to send status update: {}", e);
                        break;
                    }
                }
            }
        }
    }

    info!("WebSocket handler exiting");
}

fn websocket_fan_in_capacity() -> usize {
    parse_websocket_fan_in_capacity(std::env::var(WS_FAN_IN_CAPACITY_ENV).ok().as_deref())
}

fn parse_websocket_fan_in_capacity(value: Option<&str>) -> usize {
    value
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|capacity| *capacity > 0)
        .unwrap_or(DEFAULT_WS_FAN_IN_CAPACITY)
}

fn fan_in_warning_threshold(max_capacity: usize) -> usize {
    (max_capacity / 10).max(1)
}

fn warn_if_fan_in_near_capacity(fan_in: &Sender<TransactionStatusEvent>, tx_id: &str) {
    let remaining_capacity = fan_in.capacity();
    let max_capacity = fan_in.max_capacity();

    if remaining_capacity <= fan_in_warning_threshold(max_capacity) {
        warn!(
            tx_id = %tx_id,
            remaining_capacity,
            max_capacity,
            "WebSocket fan-in channel is near capacity"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        fan_in_warning_threshold, parse_websocket_fan_in_capacity, DEFAULT_WS_FAN_IN_CAPACITY,
    };

    #[test]
    fn parses_configured_websocket_fan_in_capacity() {
        assert_eq!(parse_websocket_fan_in_capacity(Some("2048")), 2048);
        assert_eq!(parse_websocket_fan_in_capacity(Some(" 64 ")), 64);
    }

    #[test]
    fn falls_back_to_default_for_missing_or_invalid_capacity() {
        assert_eq!(
            parse_websocket_fan_in_capacity(None),
            DEFAULT_WS_FAN_IN_CAPACITY
        );
        assert_eq!(
            parse_websocket_fan_in_capacity(Some("0")),
            DEFAULT_WS_FAN_IN_CAPACITY
        );
        assert_eq!(
            parse_websocket_fan_in_capacity(Some("not-a-number")),
            DEFAULT_WS_FAN_IN_CAPACITY
        );
    }

    #[test]
    fn warning_threshold_is_ten_percent_with_minimum_one_slot() {
        assert_eq!(fan_in_warning_threshold(1000), 100);
        assert_eq!(fan_in_warning_threshold(9), 1);
    }
}
