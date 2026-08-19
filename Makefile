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
