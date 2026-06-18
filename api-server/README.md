# router-api-server

Off-chain API server for stellar-router providing transaction simulation and real-time status tracking via WebSocket.

## Features

### Transaction Simulation Endpoint (`/simulate`)

Allows developers to preview transaction outcomes before execution.

**Request:**
```json
POST /simulate
{
  "target": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
  "function": "transfer",
  "route_details": {
    "name": "swap_route",
    "version": 1,
    "expected_outputs": ["1000000"]
  }
}
```

**Response:**
```json
{
  "success": true,
  "estimated_fees": {
    "base_fee": 100,
    "resource_fee": 1000,
    "total_fee": 1100,
    "surge_multiplier": 100,
    "high_load": false
  },
  "expected_outputs": ["1000000"],
  "route_breakdown": {
    "route_name": "swap_route",
    "version": 1,
    "target_contract": "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4",
    "function": "transfer"
  },
  "message": "Simulation successful"
}
```

### WebSocket Transaction Status Tracking (`/ws`)

Real-time transaction status updates via WebSocket.

**Subscribe to transaction:**
```json
{
  "action": "subscribe",
  "tx_id": "tx_12345"
}
```

**Status events:**
```json
{
  "msg_type": "status_update",
  "data": {
    "tx_id": "tx_12345",
    "status": "PENDING",
    "timestamp": "2026-04-28T02:38:56Z",
    "message": "Transaction queued"
  }
}
```

**Supported statuses:**
- `PENDING` - Transaction is pending
- `SUBMITTED` - Transaction submitted to network
- `CONFIRMED` - Transaction confirmed on-chain
- `FAILED` - Transaction failed

**Unsubscribe from transaction:**
```json
{
  "action": "unsubscribe",
  "tx_id": "tx_12345"
}
```

## Running

### Prerequisites

- Rust 1.78+
- Soroban RPC endpoint URL
- Router execution contract ID

### Environment Variables

```bash
export LISTEN_ADDR="127.0.0.1:8080"
export SOROBAN_RPC_URL="https://soroban-testnet.stellar.org"
export ROUTER_EXECUTION_CONTRACT_ID="CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4"
export ROUTER_API_MAX_REQUESTS="60"
export ROUTER_API_RATE_WINDOW_SECS="60"
```

`ROUTER_API_MAX_REQUESTS` and `ROUTER_API_RATE_WINDOW_SECS` control the token-bucket limiter for protected API routes. Requests are limited by `X-API-Key` when present, otherwise by remote IP address.

### Start Server

```bash
cargo run --release -p router-api-server
```

### Docker

```bash
docker build -t router-api-server -f Dockerfile.api .
docker run -p 8080:8080 \
  -e SOROBAN_RPC_URL="https://soroban-testnet.stellar.org" \
  -e ROUTER_EXECUTION_CONTRACT_ID="CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABSC4" \
  router-api-server
```

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/simulate` | POST | Simulate transaction |
| `/routes` | GET | List registered route names |
| `/routes/:name` | GET | Fetch route details |
| `/ws` | GET | WebSocket connection for status tracking |

## Reconnection Handling

The WebSocket client should implement automatic reconnection with exponential backoff:

1. Initial connection attempt
2. On disconnect, wait 1 second before retry
3. Double wait time on each subsequent failure (max 30 seconds)
4. Re-subscribe to previous transaction IDs after reconnection

## Error Handling

### Simulation Errors

- `400 Bad Request` - Missing or invalid parameters
- `500 Internal Server Error` - RPC or contract call failure

### WebSocket Errors

- Invalid JSON in message
- Unknown action type
- Connection timeout (server-side: 5 minutes of inactivity)

## Development

Run tests:
```bash
cargo test -p router-api-server
```

Build release:
```bash
cargo build --release -p router-api-server
```
