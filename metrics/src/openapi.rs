use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::server::metrics_handler,
        crate::server::health_handler,
        crate::server::ready_handler,
    ),
    info(title = "Router Metrics Exporter API", version = "0.1.0",)
)]
pub struct ApiDoc;
