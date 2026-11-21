use indexmap::IndexMap;
use shared::{
    State,
    extensions::{
        Extension, ExtensionPermissionsBuilder, ExtensionRouteBuilder, ExtensionUpdateInfo,
        background_tasks::BackgroundTaskBuilder, settings::ExtensionSettingsDeserializer,
    },
    permissions::PermissionGroup,
};
use std::sync::Arc;

mod access;
mod anvil;
mod cache;
mod chunk;
mod colors;
mod coords;
mod db;
mod entities;
mod files;
mod heatmap;
mod map;
mod nbt;
mod online;
mod players;
mod render;
mod routes;
mod settings;
mod tiles;
mod tunnel;
mod worlds;

pub const PACKAGE: &str = "dev.caloptreyx.worldmap";

const LATEST_RELEASE: &str = "https://api.github.com/repos/Caloptreyx/World-Map/releases/latest";

/// The subset of GitHub's release payload the updates page needs.
#[derive(serde::Serialize, serde::Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: String,
}

fn version_from_tag(tag: &str) -> Result<semver::Version, semver::Error> {
    semver::Version::parse(tag.trim_start_matches('v'))
}

#[derive(Default)]
pub struct ExtensionStruct;

#[async_trait::async_trait]
impl Extension for ExtensionStruct {
    async fn initialize_router(
        &mut self,
        state: State,
        builder: ExtensionRouteBuilder,
    ) -> ExtensionRouteBuilder {
        builder
            .add_client_server_api_router(|router| {
                router.nest("/world-map", routes::server::router(&state))
            })
            .add_admin_api_router(|router| {
                router.nest(
                    &format!("/extensions/{PACKAGE}"),
                    routes::admin::router(&state),
                )
            })
    }

    async fn initialize_permissions(
        &mut self,
        _state: State,
        builder: ExtensionPermissionsBuilder,
    ) -> ExtensionPermissionsBuilder {
        builder.add_server_permission_group(
            "world-map",
            PermissionGroup {
                description: "Permissions for the World Map of Minecraft servers.",
                permissions: IndexMap::from([
                    (
                        "view",
                        "Allows viewing the world map, players, entities and the heatmap. Also needs permission to read file contents.",
                    ),
                    (
                        "delete-chunks",
                        "Allows deleting selected chunks so they generate again, while the server is stopped. Also needs permission to update files.",
                    ),
                    (
                        "teleport",
                        "Allows teleporting players from the map. Also needs permission to send console commands.",
                    ),
                    (
                        "reset-heatmap",
                        "Allows clearing the collected player heatmap of the server.",
                    ),
                ]),
            },
        )
    }

    async fn initialize_background_tasks(
        &mut self,
        _state: State,
        builder: BackgroundTaskBuilder,
    ) -> BackgroundTaskBuilder {
        builder
            .add_task("world-map-heatmap", |state| async move {
                heatmap::run(&state).await
            })
            .await;
        builder
    }

    async fn settings_deserializer(&self, _state: State) -> ExtensionSettingsDeserializer {
        Arc::new(settings::SettingsDataDeserializer)
    }

    async fn check_for_updates(
        &self,
        state: State,
        current_version: &semver::Version,
    ) -> Result<Option<ExtensionUpdateInfo>, anyhow::Error> {
        // GitHub allows 60 unauthenticated requests per hour, and the panel rechecks on every
        // start and every 12 hours.
        let release: Release = state
            .cache
            .cached("dev.caloptreyx.worldmap::latest-release", 60 * 60, || async {
                let response = state.client.get(LATEST_RELEASE).send().await?;
                Ok::<Release, anyhow::Error>(response.error_for_status()?.json::<Release>().await?)
            })
            .await?;

        let latest = version_from_tag(&release.tag_name)?;

        Ok((latest > *current_version).then(|| ExtensionUpdateInfo {
            version: latest,
            changes: release
                .body
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(compact_str::CompactString::from)
                .collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_from_tag_accepts_prefixed_and_bare_tags() {
        assert_eq!(version_from_tag("v0.1.0").unwrap().to_string(), "0.1.0");
        assert_eq!(version_from_tag("1.2.3").unwrap().to_string(), "1.2.3");
        assert!(version_from_tag("release-1").is_err());
    }
}
