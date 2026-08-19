# api-server utoipa OpenAPI integration - TODO

- [x] Update `api-server/Cargo.toml` to add `utoipa` and `utoipa-swagger-ui` dependencies.

- [ ] Annotate handlers in `api-server/src/handlers.rs` with `#[utoipa::path(...)]` for:
  - [ ] GET /health
  - [ ] POST /simulate
  - [ ] GET /routes
  - [ ] GET /routes/{name}
- [ ] Create new `api-server/src/openapi.rs` with `ApiDoc` deriving `OpenApi` and wiring the annotated handlers.
- [ ] Update `api-server/src/main.rs` to serve:
  - [ ] `GET /api-docs/openapi.json`
  - [ ] Swagger UI at `/swagger-ui/`
- [ ] Run `cargo test` to confirm compilation.
- [ ] Smoke test running server and opening the two doc routes.

