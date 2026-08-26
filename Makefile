.PHONY: test-all lint deny build-wasm
.PHONY: test-all lint fmt-check audit deny build-wasm coverage
.PHONY: test-all lint build-wasm coverage
.PHONY: test-all lint audit build-wasm

test-all:
	cargo test

lint:
	cargo clippy --all-targets --all-features -- -D warnings

# Checks formatting matches CI (cargo fmt --all --check in .github/workflows/ci.yml).
# Exits non-zero if any file would be reformatted.
fmt-check:
	cargo fmt --all --check

# Checks for vulnerabilities in the dependency tree (matches the CI security audit).
# Prerequisite: cargo install cargo-audit
#
# Usage:
#   make audit
#
# For more details on configuration, see .github/workflows/security-audit.yml
audit:
	cargo audit

# Checks licenses, bans, advisories, and sources in the dependency tree
# (matches the CI cargo deny step in .github/workflows/security-audit.yml).
# Prerequisite: cargo install cargo-deny
#
# Usage:
#   make deny
deny:
	cargo deny check

build-wasm:
	cargo build --target wasm32-unknown-unknown --release

# Code coverage for the off-chain crates (same crates as the CI coverage job).
# Generates an HTML report (tarpaulin-report.html) and a Cobertura XML report
# (cobertura.xml) under ./coverage.
# Prerequisite: cargo install cargo-tarpaulin
coverage:
	cargo tarpaulin -p router-api-server -p router-metrics-exporter -p router-off-chain-common --timeout 300 --out html --out xml --output-dir ./coverage
