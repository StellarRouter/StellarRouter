.PHONY: test-all lint deny build-wasm

test-all:
	cargo test

lint:
	cargo clippy --all-targets --all-features -- -D warnings

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
