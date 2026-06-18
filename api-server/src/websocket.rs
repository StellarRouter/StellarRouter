use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use futures_util::stream::FuturesUnordered;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tracing::{error, info, warn};

use crate::{
    state::AppState,
    types::{SubscribeMessage, TransactionStatusEvent},
};

/// WebSocket upgrade handler
pub async fn ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    info!("WebSocket client connected");

    let mut subscriptions: Vec<String> = Vec::new();
    let mut rx_handles: Vec<(String, tokio::sync::broadcast::Receiver<TransactionStatusEvent>)> = Vec::new();

    loop {
        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<SubscribeMessage>(&text) {
                            Ok(sub_msg) => {
                                if sub_msg.action == "subscribe" {
                                    info!("Client subscribed to tx_id: {}", sub_msg.tx_id);
                                    subscriptions.push(sub_msg.tx_id.clone());
                                    state.add_subscriber(sub_msg.tx_id.clone());
                                    let rx = state.tx_status_tx.subscribe();
                                    rx_handles.push((sub_msg.tx_id.clone(), rx));

                                    let response = json!({
                                        "msg_type": "subscribed",
                                        "data": {
                                            "tx_id": sub_msg.tx_id,
                                            "status": "subscribed",
                                        },
                                    });

                                    if let Err(e) = sender.send(Message::Text(response.to_string())).await {
                                        error!("Failed to send subscription confirmation: {}", e);
                                        break;
                                    }
                                } else if sub_msg.action == "unsubscribe" {
                                    info!("Client unsubscribed from tx_id: {}", sub_msg.tx_id);
                                    subscriptions.retain(|id| id != &sub_msg.tx_id);
                                    state.remove_subscriber(&sub_msg.tx_id);
                                    rx_handles.retain(|(id, _)| id != &sub_msg.tx_id);
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse WebSocket message: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("WebSocket client disconnected");
                        for tx_id in &subscriptions {
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
            received = async {
                // Race every subscription receiver concurrently. Using
                // FuturesUnordered ensures an update on *any* subscribed tx_id is
                // delivered as soon as it arrives, rather than only when the first
                // receiver in the list happens to receive one.
                let mut receivers: FuturesUnordered<_> = rx_handles
                    .iter_mut()
                    .map(|(tx_id, rx)| {
                        let tx_id = tx_id.clone();
                        async move { (tx_id, rx.recv().await) }
                    })
                    .collect();

                match receivers.next().await {
                    Some(result) => result,
                    // No active subscriptions: park this branch so the loop is
                    // driven solely by the inbound-message branch.
                    None => std::future::pending().await,
                }
            } => {
                let (sub_tx_id, recv_result) = received;
                match recv_result {
                    Ok(event) => {
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
                    Err(RecvError::Lagged(skipped)) => {
                        // The receiver fell behind the broadcast buffer; it
                        // resumes from the oldest retained event on the next poll.
                        warn!(
                            "WebSocket subscriber for tx_id {} lagged; {} event(s) dropped",
                            sub_tx_id, skipped
                        );
                    }
                    Err(RecvError::Closed) => {
                        info!("Broadcast channel closed; closing WebSocket handler");
                        break;
                    }
                }
            }
        }
    }

    info!("WebSocket handler exiting");
}
