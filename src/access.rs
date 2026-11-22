//! Who may do what on the map, checked once per route through the panel's own permission
//! manager. Each action needs the extension's permission plus the panel permissions of what it
//! does underneath (reading files, writing them, sending console commands), and some need the
//! server in a certain power state.
use axum::http::StatusCode;
use serde::Serialize;
use shared::{models::user::PermissionManager, response::ApiResponse};

pub const VIEW: &str = "world-map.view";
pub const DELETE_CHUNKS: &str = "world-map.delete-chunks";
pub const TELEPORT: &str = "world-map.teleport";
pub const RESET_HEATMAP: &str = "world-map.reset-heatmap";
pub const MANAGE_WORLDS: &str = "world-map.manage-worlds";
pub const MANAGE_PLAYERS: &str = "world-map.manage-players";

const READ_FILES: &str = "files.read-content";
const WRITE_FILES: &str = "files.update";
const CONSOLE: &str = "control.console";
const READ_CONSOLE: &str = "control.read-console";

pub trait Grants {
    fn grants(&self, permission: &str) -> bool;
}

impl Grants for PermissionManager {
    fn grants(&self, permission: &str) -> bool {
        self.has_server_permission(permission).is_ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Offline,
    Starting,
    Running,
    Stopping,
}

impl From<wings_api::ServerState> for Power {
    fn from(state: wings_api::ServerState) -> Self {
        match state {
            wings_api::ServerState::Offline => Self::Offline,
            wings_api::ServerState::Starting => Self::Starting,
            wings_api::ServerState::Running => Self::Running,
            wings_api::ServerState::Stopping => Self::Stopping,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The map, players, entities and the heatmap.
    View,
    /// Rewriting region files without the selected chunks.
    DeleteChunks,
    /// `tp` through the console.
    Teleport,
    /// Clearing the server's heatmap samples.
    ResetHeatmap,
    /// Adding world folders the map does not find by itself.
    ManageWorlds,
    /// Choosing whether the map shows online and offline players on the server.
    ManagePlayers,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Denied {
    Permission(&'static str),
    /// The server has to be stopped first.
    Running,
    /// The server has to be running.
    NotRunning,
}

impl Action {
    pub fn permissions(self) -> &'static [&'static str] {
        match self {
            Self::View => &[VIEW, READ_FILES],
            Self::DeleteChunks => &[VIEW, READ_FILES, DELETE_CHUNKS, WRITE_FILES],
            Self::Teleport => &[VIEW, TELEPORT, CONSOLE],
            Self::ResetHeatmap => &[VIEW, RESET_HEATMAP],
            Self::ManageWorlds => &[VIEW, READ_FILES, MANAGE_WORLDS],
            Self::ManagePlayers => &[VIEW, MANAGE_PLAYERS],
        }
    }

    /// Whether the permissions allow the action, regardless of the server's state.
    pub fn granted(self, grants: &impl Grants) -> bool {
        self.permissions()
            .iter()
            .all(|permission| grants.grants(permission))
    }

    /// Checks the permissions, then the power state (`None` when it does not matter).
    pub fn check(self, grants: &impl Grants, power: Option<Power>) -> Result<(), Denied> {
        if let Some(missing) = self
            .permissions()
            .iter()
            .find(|permission| !grants.grants(permission))
        {
            return Err(Denied::Permission(*missing));
        }
        match (self, power) {
            // a running or starting server would overwrite the region files from memory
            (Self::DeleteChunks, Some(state)) if state != Power::Offline => Err(Denied::Running),
            (Self::DeleteChunks, None) => Err(Denied::Running),
            (Self::Teleport, Some(Power::Running)) => Ok(()),
            (Self::Teleport, _) => Err(Denied::NotRunning),
            _ => Ok(()),
        }
    }
}

/// Whether the console's answer to `list` may be used to find online players: sending it
/// needs the console, reading the answer the console log.
pub fn may_list_by_console(grants: &impl Grants) -> bool {
    grants.grants(CONSOLE) && grants.grants(READ_CONSOLE)
}

/// What the page may offer the user.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub delete_chunks: bool,
    pub teleport: bool,
    pub reset_heatmap: bool,
    pub manage_worlds: bool,
    pub manage_players: bool,
}

pub fn capabilities(grants: &impl Grants) -> Capabilities {
    Capabilities {
        delete_chunks: Action::DeleteChunks.granted(grants),
        teleport: Action::Teleport.granted(grants),
        reset_heatmap: Action::ResetHeatmap.granted(grants),
        manage_worlds: Action::ManageWorlds.granted(grants),
        manage_players: Action::ManagePlayers.granted(grants),
    }
}

/// Checks an action for a route: the panel's own error for a missing permission, 409 for the
/// wrong power state.
pub fn authorize(
    action: Action,
    permissions: &PermissionManager,
    power: Option<Power>,
) -> Result<(), ApiResponse> {
    match action.check(permissions, power) {
        Ok(()) => Ok(()),
        Err(Denied::Permission(permission)) => {
            permissions.has_server_permission(permission)?;
            Err(ApiResponse::error(format!("missing permission {permission}"))
                .with_status(StatusCode::FORBIDDEN))
        }
        Err(Denied::Running) => Err(ApiResponse::error(
            "stop the server first: a running server would write the chunks back",
        )
        .with_status(StatusCode::CONFLICT)),
        Err(Denied::NotRunning) => {
            Err(ApiResponse::error("the server is not running").with_status(StatusCode::CONFLICT))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    struct Set(HashSet<&'static str>);

    impl Grants for Set {
        fn grants(&self, permission: &str) -> bool {
            self.0.contains(permission)
        }
    }

    fn set(permissions: &[&'static str]) -> Set {
        Set(permissions.iter().copied().collect())
    }

    const ALL: &[&str] = &[
        VIEW,
        DELETE_CHUNKS,
        TELEPORT,
        RESET_HEATMAP,
        MANAGE_WORLDS,
        MANAGE_PLAYERS,
        READ_FILES,
        WRITE_FILES,
        CONSOLE,
        READ_CONSOLE,
    ];

    #[test]
    fn deleting_chunks_needs_every_permission_and_a_stopped_server() {
        let all = set(ALL);
        assert_eq!(
            Action::DeleteChunks.check(&all, Some(Power::Offline)),
            Ok(())
        );
        for state in [Power::Running, Power::Starting, Power::Stopping] {
            assert_eq!(
                Action::DeleteChunks.check(&all, Some(state)),
                Err(Denied::Running)
            );
        }
        // an unknown state never counts as stopped
        assert_eq!(Action::DeleteChunks.check(&all, None), Err(Denied::Running));
        for missing in [VIEW, READ_FILES, DELETE_CHUNKS, WRITE_FILES] {
            let without: Vec<&'static str> =
                ALL.iter().copied().filter(|p| *p != missing).collect();
            assert_eq!(
                Action::DeleteChunks.check(&set(&without), Some(Power::Offline)),
                Err(Denied::Permission(missing)),
                "{missing}"
            );
        }
    }

    #[test]
    fn teleporting_needs_the_console_and_a_running_server() {
        let all = set(ALL);
        assert_eq!(Action::Teleport.check(&all, Some(Power::Running)), Ok(()));
        assert_eq!(
            Action::Teleport.check(&all, Some(Power::Offline)),
            Err(Denied::NotRunning)
        );
        assert_eq!(
            Action::Teleport.check(&all, Some(Power::Starting)),
            Err(Denied::NotRunning)
        );
        assert_eq!(
            Action::Teleport.check(&set(&[VIEW, TELEPORT]), Some(Power::Running)),
            Err(Denied::Permission(CONSOLE))
        );
        assert_eq!(
            Action::Teleport.check(&set(&[VIEW, CONSOLE]), Some(Power::Running)),
            Err(Denied::Permission(TELEPORT))
        );
    }

    #[test]
    fn viewing_needs_file_reads_and_offers_only_what_is_granted() {
        assert_eq!(
            Action::View.check(&set(&[VIEW]), None),
            Err(Denied::Permission(READ_FILES))
        );
        assert_eq!(Action::View.check(&set(&[VIEW, READ_FILES]), None), Ok(()));
        assert_eq!(
            Action::ManageWorlds.check(&set(&[VIEW, MANAGE_WORLDS]), None),
            Err(Denied::Permission(READ_FILES))
        );
        assert_eq!(
            Action::ManageWorlds.check(&set(&[VIEW, READ_FILES]), None),
            Err(Denied::Permission(MANAGE_WORLDS))
        );
        assert_eq!(
            Action::ManagePlayers.check(&set(&[VIEW, READ_FILES]), None),
            Err(Denied::Permission(MANAGE_PLAYERS))
        );
        assert_eq!(
            Action::ManagePlayers.check(&set(&[VIEW, MANAGE_PLAYERS]), None),
            Ok(())
        );
        let viewer = set(&[VIEW, READ_FILES, CONSOLE]);
        assert_eq!(
            capabilities(&viewer),
            Capabilities {
                delete_chunks: false,
                teleport: false,
                reset_heatmap: false,
                manage_worlds: false,
                manage_players: false,
            }
        );
        assert!(!may_list_by_console(&viewer));
        let everything = set(ALL);
        assert_eq!(
            capabilities(&everything),
            Capabilities {
                delete_chunks: true,
                teleport: true,
                reset_heatmap: true,
                manage_worlds: true,
                manage_players: true,
            }
        );
        assert!(may_list_by_console(&everything));
    }
}
