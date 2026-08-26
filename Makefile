.PHONY: test-all lint build-wasm coverage
.PHONY: test-all lint audit build-wasm

test-all:
	cargo test

lint:
	cargo clippy --all-targets --all-features -- -D warnings

# Checks for vulnerabilities in the dependency tree (matches the CI security audit).
# Prerequisite: cargo install cargo-audit
#
# Usage:
#   make audit
#
# For more details on configuration, see .github/workflows/security-audit.yml
audit:
	cargo audit

build-wasm:
	cargo build --target wasm32-unknown-unknown --release

# Code coverage for the off-chain crates (same crates as the CI coverage job).
# Generates an HTML report (tarpaulin-report.html) and a Cobertura XML report
# (cobertura.xml) under ./coverage.
# Prerequisite: cargo install cargo-tarpaulin
coverage:
	cargo tarpaulin -p router-api-server -p router-metrics-exporter -p router-off-chain-common --timeout 300 --out html --out xml --output-dir ./coverage
