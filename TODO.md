# TODO

- [x] Implement API key length limiting in `router-off-chain-common/src/rate_limit.rs` (truncate to 256 bytes where possible, otherwise reject to IP fallback)

- [ ] Add unit tests for: normal API key, empty header, oversized API key, non-UTF8 header

- [ ] Run Rust tests (`cargo test`) to ensure no regressions

