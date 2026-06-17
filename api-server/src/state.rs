use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::{auth::AuthConfig, rpc::SorobanRpcClient, types::TransactionStatusEvent};

#[derive(Clone)]
pub struct AppState {
    pub rpc: SorobanRpcClient,
    pub execution_contract_id: String,
    pub router_core_contract_id: String,
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
    ) -> Self {
        let (tx_status_tx, _) = broadcast::channel(1000);
        Self {
            rpc: SorobanRpcClient::new(rpc_url, Some(router_core_contract_id.clone())),
            execution_contract_id,
            router_core_contract_id,
            auth_config,
            tx_status_tx,
            tx_subscribers: Arc::new(DashMap::new()),
        }
    }

    pub fn broadcast_status(&self, event: TransactionStatusEvent) {
        let _ = self.tx_status_tx.send(event);
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
