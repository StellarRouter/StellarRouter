#[cfg(test)]
mod tests {
    use crate::types::{
        FeeEstimate, RouteDetails, SimulateRequest, TransactionStatus, TransactionStatusEvent,
    };

    #[test]
    fn test_simulate_request_serialization() {
        let req = SimulateRequest {
            target: "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4".to_string(),
            function: "transfer".to_string(),
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

    #[test]
    fn test_transaction_status_event_serialization() {
        let event = TransactionStatusEvent {
            tx_id: "tx_12345".to_string(),
            status: TransactionStatus::Pending,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            message: Some("Transaction queued".to_string()),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: TransactionStatusEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.tx_id, event.tx_id);
        assert_eq!(deserialized.status, TransactionStatus::Pending);
    }

    #[test]
    fn test_transaction_status_enum() {
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Pending).unwrap(),
            "\"PENDING\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Submitted).unwrap(),
            "\"SUBMITTED\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Confirmed).unwrap(),
            "\"CONFIRMED\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Failed).unwrap(),
            "\"FAILED\""
        );
    }

    #[test]
    fn test_fee_estimate_calculation() {
        let fee = FeeEstimate {
            base_fee: 100,
            resource_fee: 1000,
            total_fee: 1100,
            surge_multiplier: 100,
            high_load: false,
        };

        assert_eq!(fee.base_fee + fee.resource_fee, 1100);
        assert!(!fee.high_load);
    }

    #[test]
    fn test_fee_estimate_with_surge() {
        let fee = FeeEstimate {
            base_fee: 100,
            resource_fee: 1000,
            total_fee: 2200,
            surge_multiplier: 200,
            high_load: true,
        };

        assert_eq!(fee.total_fee, (fee.base_fee + fee.resource_fee) * 2);
        assert!(fee.high_load);
    }

    /// GET /routes returns 503 when router_core_contract_id is empty.
    #[tokio::test]
    async fn test_list_routes_no_contract_id_returns_503() {
        use axum::{body::Body, http::Request, http::StatusCode, routing::get, Router};
        use tower::ServiceExt;

        use crate::{handlers, state::AppState};

        let state = AppState::new(
            "http://localhost:1".to_string(),
            "".to_string(),
            "".to_string(), // no router_core_contract_id
        );
        let app = Router::new()
            .route("/routes", get(handlers::list_routes))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/routes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
