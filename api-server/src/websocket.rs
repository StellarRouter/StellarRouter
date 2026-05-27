use axum::{
    extract::{
        ws::{WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tracing::{error, info, warn};

use crate::{
    state::AppState,
    types::{SubscribeMessage, TransactionStatus, TransactionStatusEvent},
};

/// WebSocket upgrade handler
pub async fn ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// Handle WebSocket connection
async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    info!("WebSocket client connected");

    let mut subscriptions: Vec<String> = Vec::new();
    let mut rx_handles: Vec<(String, tokio::sync::broadcast::Receiver<TransactionStatusEvent>)> =
        Vec::new();

    loop {
        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(axum::extract::ws::Message::Text(text))) => {
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
                                            "status": "subscribed"
                                        }
                                    });

                                    if let Err(e) = sender.send(axum::extract::ws::Message::Text(
                                        response.to_string().into(),
                                    )).await {
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
                    Some(Ok(axum::extract::ws::Message::Close(_))) | None => {
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

            result = async {
                for (tx_id, rx) in &mut rx_handles {
                    if let Ok(event) = rx.try_recv() {
                        return Some((tx_id.clone(), event));
                    }
                }
                if let Some((tx_id, rx)) = rx_handles.first_mut() {
                    match rx.recv().await {
                        Ok(event) => Some((tx_id.clone(), event)),
                        Err(_) => None,
                    }
                } else {
                    std::future::pending().await
                }
            } => {
                if let Some((_tx_id, event)) = result {
                    let response = json!({
                        "msg_type": "status_update",
                        "data": {
                            "tx_id": event.tx_id,
                            "status": event.status,
                            "timestamp": event.timestamp,
                            "message": event.message
                        }
                    });

                    if let Err(e) = sender.send(axum::extract::ws::Message::Text(
                        response.to_string().into(),
                    )).await {
                        error!("Failed to send status update: {}", e);
                        break;
                    }
                }
            }
        }
    }

    info!("WebSocket handler exiting");
}
