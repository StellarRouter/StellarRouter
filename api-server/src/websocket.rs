use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::broadcast::{error::RecvError, Receiver};
use tracing::{error, info, warn};

use crate::{
    state::AppState,
    types::{SubscribeMessage, TransactionStatusEvent},
};

/// WebSocket upgrade handler
pub async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    info!("WebSocket client connected");

    // Fan-in channel: one relay task per subscription forwards events here.
    let (fan_in_tx, mut fan_in_rx) = tokio::sync::mpsc::channel::<TransactionStatusEvent>(1000);

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
