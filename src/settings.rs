//! Extension settings, edited on the extension's card in the admin area.
use serde::{Deserialize, Serialize};
use shared::{
    State,
    extensions::settings::{
        ExtensionSettings, SettingsDeserializeExt, SettingsDeserializer, SettingsSerializeExt,
        SettingsSerializer,
    },
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Eggs whose servers get the map; empty means every server that looks like Minecraft Java
    /// (a `server.properties` and no Bedrock files).
    pub eggs: Vec<uuid::Uuid>,
    /// Sample player positions for the heatmap.
    pub heatmap_enabled: bool,
    /// Minutes between samples (the game saves player files every 5 minutes by default).
    pub heatmap_interval_minutes: u32,
    /// Days of heatmap samples to keep.
    pub heatmap_retention_days: u32,
    /// Largest region file read, in MiB.
    pub max_region_mb: u32,
    /// Region files rendered at the same time, panel-wide.
    pub render_concurrency: u32,
    /// The Nether is drawn from this height down, below its roof.
    pub nether_ceiling: i32,
    /// Most entities sent for one view.
    pub entity_limit: u32,
    /// Most chunks one deletion may select.
    pub max_delete_chunks: u32,
    /// Rendered tiles and 3D surfaces kept in the database together, in MiB; the least recently
    /// viewed go first.
    pub tile_cache_mb: u32,
    /// Days chunk tool backups (`.worldmap-backups/`) are kept before they are deleted.
    pub backup_retention_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            eggs: Vec::new(),
            heatmap_enabled: true,
            heatmap_interval_minutes: 5,
            heatmap_retention_days: 30,
            max_region_mb: 64,
            render_concurrency: 2,
            nether_ceiling: 100,
            entity_limit: 5000,
            max_delete_chunks: 65536,
            tile_cache_mb: 2048,
            backup_retention_days: 7,
        }
    }
}

fn check<T: PartialOrd + std::fmt::Display>(
    errors: &mut Vec<String>,
    name: &str,
    value: T,
    min: T,
    max: T,
) {
    if value < min || value > max {
        errors.push(format!("{name} must be between {min} and {max}"));
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        check(&mut errors, "heatmap_interval_minutes", self.heatmap_interval_minutes, 1, 60);
        check(&mut errors, "heatmap_retention_days", self.heatmap_retention_days, 1, 365);
        check(&mut errors, "max_region_mb", self.max_region_mb, 1, 1024);
        check(&mut errors, "render_concurrency", self.render_concurrency, 1, 16);
        check(&mut errors, "nether_ceiling", self.nether_ceiling, -64, 320);
        check(&mut errors, "entity_limit", self.entity_limit, 100, 50_000);
        check(&mut errors, "max_delete_chunks", self.max_delete_chunks, 1, 1_048_576);
        check(&mut errors, "tile_cache_mb", self.tile_cache_mb, 64, 1_048_576);
        check(&mut errors, "backup_retention_days", self.backup_retention_days, 1, 365);
        if self.eggs.len() > 1000 {
            errors.push("eggs must list at most 1000 eggs".into());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Whether servers of the egg `egg` may use the map at all.
    pub fn allows_egg(&self, egg: uuid::Uuid) -> bool {
        self.eggs.is_empty() || self.eggs.contains(&egg)
    }
}

#[async_trait::async_trait]
impl SettingsSerializeExt for Settings {
    async fn serialize(
        &self,
        serializer: SettingsSerializer,
    ) -> Result<SettingsSerializer, anyhow::Error> {
        Ok(serializer
            .write_serde_setting("eggs", &self.eggs)?
            .write_serde_setting("heatmap_enabled", &self.heatmap_enabled)?
            .write_serde_setting("heatmap_interval_minutes", &self.heatmap_interval_minutes)?
            .write_serde_setting("heatmap_retention_days", &self.heatmap_retention_days)?
            .write_serde_setting("max_region_mb", &self.max_region_mb)?
            .write_serde_setting("render_concurrency", &self.render_concurrency)?
            .write_serde_setting("nether_ceiling", &self.nether_ceiling)?
            .write_serde_setting("entity_limit", &self.entity_limit)?
            .write_serde_setting("max_delete_chunks", &self.max_delete_chunks)?
            .write_serde_setting("tile_cache_mb", &self.tile_cache_mb)?
            .write_serde_setting("backup_retention_days", &self.backup_retention_days)?)
    }
}

pub struct SettingsDataDeserializer;

#[async_trait::async_trait]
impl SettingsDeserializeExt for SettingsDataDeserializer {
    async fn deserialize_boxed(
        &self,
        deserializer: SettingsDeserializer<'_>,
    ) -> Result<ExtensionSettings, anyhow::Error> {
        let d = Settings::default();
        macro_rules! serde_or {
            ($key:literal, $default:expr) => {
                deserializer.read_serde_setting($key).unwrap_or($default)
            };
        }
        Ok(Box::new(Settings {
            eggs: serde_or!("eggs", d.eggs),
            heatmap_enabled: serde_or!("heatmap_enabled", d.heatmap_enabled),
            heatmap_interval_minutes: serde_or!(
                "heatmap_interval_minutes",
                d.heatmap_interval_minutes
            ),
            heatmap_retention_days: serde_or!("heatmap_retention_days", d.heatmap_retention_days),
            max_region_mb: serde_or!("max_region_mb", d.max_region_mb),
            render_concurrency: serde_or!("render_concurrency", d.render_concurrency),
            nether_ceiling: serde_or!("nether_ceiling", d.nether_ceiling),
            entity_limit: serde_or!("entity_limit", d.entity_limit),
            max_delete_chunks: serde_or!("max_delete_chunks", d.max_delete_chunks),
            tile_cache_mb: serde_or!("tile_cache_mb", d.tile_cache_mb),
            backup_retention_days: serde_or!("backup_retention_days", d.backup_retention_days),
        }))
    }
}

pub async fn load(state: &State) -> Result<Settings, anyhow::Error> {
    let settings = state.settings.get().await?;
    Ok(settings
        .get_extension_settings::<Settings>(crate::PACKAGE)
        .cloned()
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_limits_are_checked() {
        assert!(Settings::default().validate().is_ok());
        let wrong = Settings {
            heatmap_interval_minutes: 0,
            render_concurrency: 99,
            nether_ceiling: 1000,
            ..Settings::default()
        };
        assert_eq!(wrong.validate().unwrap_err().len(), 3);
    }

    #[test]
    fn backup_retention_is_one_day_to_a_year() {
        assert_eq!(Settings::default().backup_retention_days, 7);
        for days in [1, 365] {
            let settings = Settings {
                backup_retention_days: days,
                ..Settings::default()
            };
            assert!(settings.validate().is_ok(), "{days}");
        }
        for days in [0, 366] {
            let settings = Settings {
                backup_retention_days: days,
                ..Settings::default()
            };
            assert_eq!(
                settings.validate().unwrap_err(),
                vec!["backup_retention_days must be between 1 and 365".to_string()]
            );
        }
    }

    #[test]
    fn an_empty_egg_list_allows_every_egg() {
        let egg = uuid::Uuid::from_u128(7);
        assert!(Settings::default().allows_egg(egg));
        let only_other = Settings {
            eggs: vec![uuid::Uuid::from_u128(8)],
            ..Settings::default()
        };
        assert!(!only_other.allows_egg(egg));
    }

    #[test]
    fn incomplete_updates_are_refused() {
        // the admin page sends every field; a missing one is an error, not a silent default
        assert!(serde_json::from_str::<Settings>(r#"{"eggs":[]}"#).is_err());
    }
}
