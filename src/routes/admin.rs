//! The extension settings, mounted at `/api/admin/extensions/dev.caloptreyx.worldmap`. Reading
//! needs `extensions.read`, changing `extensions.manage`.
use crate::settings::{self, Settings};
use axum::{http::StatusCode, routing::get};
use serde_json::json;
use shared::{
    ApiError, GetState, Payload, State,
    models::{admin_activity::GetAdminActivityLogger, user::GetPermissionManager},
    response::{ApiResponse, ApiResponseResult},
};
use utoipa_axum::router::OpenApiRouter;

async fn get_settings(state: GetState, permissions: GetPermissionManager) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.read")?;
    ApiResponse::new_serialized(json!({ "settings": settings::load(&state).await? })).ok()
}

async fn put_settings(
    state: GetState,
    permissions: GetPermissionManager,
    activity_logger: GetAdminActivityLogger,
    Payload(data): Payload<Settings>,
) -> ApiResponseResult {
    permissions.has_admin_permission("extensions.manage")?;
    if let Err(errors) = data.validate() {
        return ApiResponse::new_serialized(ApiError::new_strings_value(errors))
            .with_status(StatusCode::BAD_REQUEST)
            .ok();
    }
    {
        let mut panel_settings = state.settings.get_mut().await?;
        let stored: &mut Settings = panel_settings.get_mut_extension_settings(crate::PACKAGE)?;
        *stored = data.clone();
        panel_settings.save().await?;
    }
    activity_logger
        .log(
            "settings:extensions:update",
            json!({ "extension": crate::PACKAGE, "settings": data }),
        )
        .await;
    ApiResponse::new_serialized(json!({ "settings": data })).ok()
}

pub fn router(state: &State) -> OpenApiRouter<State> {
    OpenApiRouter::new()
        .route("/settings", get(get_settings).put(put_settings))
        .with_state(state.clone())
}
