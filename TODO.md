# TODO

- [ ] Confirm current behavior for GET /routes and Soroban RPC error handling in api-server/src/rpc.rs.
- [ ] Implement change so get_all_routes propagates RPC/backend errors (no silent Ok(vec![])).
- [ ] Add/update response shape or error propagation so callers can distinguish empty registry vs backend failure.
- [ ] Add tests (unit/integration) covering RPC failure -> non-empty error response for GET /routes.
- [ ] Run cargo test for api-server (and any affected workspace crates).

