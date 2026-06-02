# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog and this project adheres to Semantic Versioning.

## [Unreleased]

### Added
- `router-access`: blacklist entries can now include an optional `reason` and an `expires_at` timestamp. Expired blacklist entries are treated as not blacklisted.
- `router-registry`: `ContractEntry` includes an optional `deprecation_reason` and the `deprecate()` API accepts an optional reason which is emitted in the `contract_deprecated` event.
- `metrics/alerts.yml`: example Prometheus alerting rules for circuit breaker opens, high failure/error rates, and high request volume.

### Changed
- Documentation: added a top-level `CHANGELOG.md` following Keep a Changelog format.

## [0.3.0] - 2024-11-15

### Added
- `router-execution`: execution pipeline with simulation, retries, and fee estimation
- `router-quote`: read-only quote preview contract for expected output, fees, and route details
- `router-middleware`: circuit breaker functionality with auto-recovery
- `router-middleware`: call logging with configurable retention
- `router-core`: route metadata support (description, tags, owner)
- `router-core`: route aliasing system
- `router-core`: route scoring and best route selection
- `metrics`: Prometheus/OpenTelemetry metrics exporter

### Changed
- `router-core`: admin() now panics on uninitialized contract instead of returning Result
- `router-middleware`: admin() now panics on uninitialized contract instead of returning Result

### Fixed
- `router-middleware`: rate limit state no longer written when route is disabled before commit
- `router-middleware`: call log retention now correctly enforces maximum entries

## [0.2.0] - 2024-09-20

### Added
- `router-middleware`: rate limiting per route with configurable windows
- `router-middleware`: global and per-route enable/disable controls
- `router-middleware`: pre_call and post_call hooks
- `router-timelock`: delayed execution queue for sensitive operations
- `router-multicall`: batch multiple cross-contract calls in one transaction
- `router-core`: pause/unpause controls at global and per-route level
- `router-core`: total_routed counter
- Integration tests for cross-contract interactions

### Changed
- `router-core`: route removal now cleans up dangling aliases
- Event naming convention: all events now use past tense verbs in snake_case

## [0.1.0] - 2024-07-10

### Added
- `router-core`: central dispatcher with route registration and resolution
- `router-registry`: versioned contract address registry with deprecation support
- `router-access`: role-based access control with blacklisting
- Basic event emission for all route operations
- Docker Compose setup for local development
- Comprehensive unit test suite for all contracts
- README with architecture diagrams and usage examples

