.PHONY: test-all lint build-wasm

test-all:
	cargo test

lint:
	cargo clippy --all-targets --all-features -- -D warnings

build-wasm:
	cargo build --target wasm32-unknown-unknown --release
