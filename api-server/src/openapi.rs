use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::handlers::health,
        crate::handlers::simulate,
        crate::handlers::get_route,
        crate::handlers::list_routes,
    ),
    info(title = "Router API Server", version = "0.1.0",)
)]
pub struct ApiDoc;
