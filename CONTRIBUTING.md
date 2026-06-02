# Contributing to stellar-router

Thank you for your interest in contributing! This document covers everything you need to get started.

## Dev Environment Setup

### Prerequisites

| Tool | Install |
|---|---|
| Rust (stable, 1.75+) | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| wasm32 target | `rustup target add wasm32-unknown-unknown` |
| Stellar CLI | `cargo install --locked stellar-cli` |
| Docker (optional) | [docs.docker.com](https://docs.docker.com/get-docker/) |

### Clone and build

```bash
git clone https://github.com/Maki-Zeninn/stellar-router.git
cd stellar-router
cargo build
```

### Docker (no local Rust required)

```bash
docker compose run tests   # run all unit tests
docker compose run wasm    # build WASM artifacts
```

## Branch Naming

```
<type>/<short-description>
```

| Type | When to use |
|---|---|
| `feat/` | New feature |
| `fix/` | Bug fix |
| `docs/` | Documentation only |
| `refactor/` | Code change with no behaviour change |
| `test/` | Adding or fixing tests |
| `chore/` | Tooling, CI, dependency updates |

Examples: `feat/compare-quotes`, `fix/execution-collector-rpc`, `docs/contributing`

## Commit Message Format

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short summary>

[optional body]

[optional footer: Closes #<issue>]
```

- **type**: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`
- **scope**: the affected component, e.g. `router-quote`, `metrics/collector`, `router-core`
- **summary**: imperative, lowercase, no trailing period, ≤ 72 chars

Examples:
```
feat(router-quote): add compare_quotes with price impact threshold
fix(metrics/collector): implement getEvents RPC in QuoteCollector
docs: add CONTRIBUTING.md
```

## Pull Request Process

1. **Branch** off `main` using the naming convention above.
2. **Implement** your change with tests.
3. **Run tests** locally (see below) — CI must pass.
4. **Open a PR** against `main` with a title following the commit format.
5. **Fill in the PR description** with: what changed, how it was tested, and any follow-up work.
6. **Request a review** — at least one approval is required before merging.
7. **Squash-merge** is preferred to keep the history clean.

Keep PRs focused. One logical change per PR makes review faster and reverts easier.

## Running Tests

### Unit tests (all contracts + off-chain crates)

```bash
cargo test
```

### Single crate

```bash
cargo test -p router-quote
cargo test -p router-metrics-exporter
```

### Integration tests (requires Stellar testnet access)

```bash
# Quick start (handles funding and deployment automatically)
./scripts/run-integration-tests.sh

# Or manually
cargo test --test integration_tests -- --ignored --test-threads=1 --nocapture
```

See [INTEGRATION_TESTS.md](INTEGRATION_TESTS.md) for full details.

### Build WASM artifacts

```bash
cargo build --target wasm32-unknown-unknown --release
```

## Code Style

- Run `cargo fmt` before committing.
- Run `cargo clippy -- -D warnings` and fix any warnings.
- Match the style and conventions of the surrounding code.
- Add doc comments (`///`) to all public items.
- Follow the event naming convention in [`contracts/router-common/EVENT_NAMING_CONVENTION.md`](contracts/router-common/EVENT_NAMING_CONVENTION.md).

## Reporting Issues

Open a GitHub issue with:
- A clear title following the commit format (e.g. `bug(router-quote): …`)
- Steps to reproduce
- Expected vs actual behaviour
- Rust / Stellar CLI version (`rustc --version`, `stellar --version`)
