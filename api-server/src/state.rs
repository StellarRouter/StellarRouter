use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::{
    auth::AuthConfig,
    rpc::{FeeConfig, SorobanRpcClient},
    types::TransactionStatusEvent,
};

/// Capacity of the broadcast channel used for WebSocket status events.
pub const BROADCAST_CHANNEL_CAPACITY: usize = 1000;

#[derive(Clone)]
pub struct AppState {
    pub rpc: SorobanRpcClient,
    #[allow(dead_code)]
    pub execution_contract_id: String,
    pub router_core_contract_id: String,
    #[allow(dead_code)]
    pub auth_config: AuthConfig,
    pub tx_status_tx: broadcast::Sender<TransactionStatusEvent>,
    pub tx_subscribers: Arc<DashMap<String, usize>>,
}

impl AppState {
    pub fn new(
        rpc_url: String,
        execution_contract_id: String,
        router_core_contract_id: String,
        auth_config: AuthConfig,
        fee_config: FeeConfig,
    ) -> Self {
        let (tx_status_tx, _) = broadcast::channel(BROADCAST_CHANNEL_CAPACITY);
        Self {
            rpc: SorobanRpcClient::new(rpc_url, Some(router_core_contract_id.clone()), fee_config),
            execution_contract_id,
            router_core_contract_id,
            auth_config,
            tx_status_tx,
            tx_subscribers: Arc::new(DashMap::new()),
        }
    }

    #[cfg(test)]
    pub fn broadcast_status(&self, event: TransactionStatusEvent) {
        let _ = self.tx_status_tx.send(event);
    }

    /// Returns the fixed capacity of the broadcast channel.
    pub fn broadcast_channel_capacity(&self) -> usize {
        BROADCAST_CHANNEL_CAPACITY
    }

    /// Returns the total number of active subscriptions across all tx IDs.
    pub fn active_subscriptions(&self) -> usize {
        self.tx_subscribers.iter().map(|e| *e.value()).sum()
    }

    /// Returns the number of distinct tx IDs currently being tracked.
    pub fn unique_tx_ids(&self) -> usize {
        self.tx_subscribers.len()
    }

    pub fn add_subscriber(&self, tx_id: String) {
        self.tx_subscribers
            .entry(tx_id)
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }

    pub fn remove_subscriber(&self, tx_id: &str) {
        if let Some(mut entry) = self.tx_subscribers.get_mut(tx_id) {
            if *entry > 1 {
                *entry -= 1;
            } else {
                drop(entry);
                self.tx_subscribers.remove(tx_id);
            }
        }
    }
}
