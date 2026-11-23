//! Who may do what on the map, checked once per route through the panel's own permission
//! manager. Each action needs the extension's permission plus the panel permissions of what it
//! does underneath (reading files, creating, moving and deleting them, sending console
//! commands), and some need the server in a certain power state.
use crate::ops::OperationKind;
use axum::http::StatusCode;
use serde::Serialize;
use shared::{models::user::PermissionManager, response::ApiResponse};

pub const VIEW: &str = "world-map.view";
pub const DELETE_CHUNKS: &str = "world-map.delete-chunks";
pub const EDIT_CHUNKS: &str = "world-map.edit-chunks";
pub const IMPORT_CHUNKS: &str = "world-map.import-chunks";
pub const TELEPORT: &str = "world-map.teleport";
pub const RESET_HEATMAP: &str = "world-map.reset-heatmap";
pub const MANAGE_WORLDS: &str = "world-map.manage-worlds";
pub const MANAGE_PLAYERS: &str = "world-map.manage-players";

const READ_FILES: &str = "files.read-content";
/// Writing new files (the temporary files every write goes through, imported files).
const CREATE_FILES: &str = "files.create";
/// Moving files (originals into the backup folder, temporary files into place).
const UPDATE_FILES: &str = "files.update";
/// Deleting files (region files left empty).
const DELETE_FILES: &str = "files.delete";
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
    /// The map, players, entities, the heatmap, filter scans, overlays and selection CSVs.
    View,
    /// Downloading the selected chunks as region files (MCA Selector's "Export").
    ExportChunks,
    /// Rewriting region files without the selected chunks.
    DeleteChunks,
    /// Changing NBT fields of the selected chunks (MCA Selector's "Change fields").
    EditChunks,
    /// Copying or importing chunks into a dimension, and the upload staging folders.
    ImportChunks,
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
            Self::View | Self::ExportChunks => &[VIEW, READ_FILES],
            Self::DeleteChunks => &[
                VIEW,
                READ_FILES,
                DELETE_CHUNKS,
                CREATE_FILES,
                UPDATE_FILES,
                DELETE_FILES,
            ],
            Self::EditChunks => &[VIEW, READ_FILES, EDIT_CHUNKS, CREATE_FILES, UPDATE_FILES],
            Self::ImportChunks => &[VIEW, READ_FILES, IMPORT_CHUNKS, CREATE_FILES, UPDATE_FILES],
            Self::Teleport => &[VIEW, TELEPORT, CONSOLE],
            Self::ResetHeatmap => &[VIEW, RESET_HEATMAP],
            Self::ManageWorlds => &[VIEW, READ_FILES, MANAGE_WORLDS],
            Self::ManagePlayers => &[VIEW, MANAGE_PLAYERS],
        }
    }

    /// Whether the action rewrites world files, so the server has to be stopped.
    pub fn writes(self) -> bool {
        matches!(
            self,
            Self::DeleteChunks | Self::EditChunks | Self::ImportChunks
        )
    }

    /// Whether the permissions allow the action, regardless of the server's state.
    pub fn granted(self, grants: &impl Grants) -> bool {
        self.missing(grants).is_none()
    }

    /// The first permission the action needs that is not granted.
    fn missing(self, grants: &impl Grants) -> Option<&'static str> {
        self.permissions()
            .iter()
            .copied()
            .find(|permission| !grants.grants(permission))
    }

    /// Checks the permissions, then the power state (`None` when it does not matter or is
    /// unknown: write actions are refused then).
    pub fn check(self, grants: &impl Grants, power: Option<Power>) -> Result<(), Denied> {
        if let Some(missing) = self.missing(grants) {
            return Err(Denied::Permission(missing));
        }
        match (self, power) {
            // a running or starting server would overwrite the region files from memory
            (action, Some(Power::Offline)) if action.writes() => Ok(()),
            (action, _) if action.writes() => Err(Denied::Running),
            (Self::Teleport, Some(Power::Running)) => Ok(()),
            (Self::Teleport, _) => Err(Denied::NotRunning),
            _ => Ok(()),
        }
    }
}

/// The action whose permissions restoring an operation of `kind` needs (`None`: restores
/// themselves cannot be restored).
pub fn restore_action(kind: OperationKind) -> Option<Action> {
    match kind {
        OperationKind::Delete => Some(Action::DeleteChunks),
        OperationKind::Change => Some(Action::EditChunks),
        OperationKind::Import => Some(Action::ImportChunks),
        OperationKind::Restore => None,
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
    pub edit_chunks: bool,
    pub import_chunks: bool,
    pub export_chunks: bool,
    pub teleport: bool,
    pub reset_heatmap: bool,
    pub manage_worlds: bool,
    pub manage_players: bool,
}

pub fn capabilities(grants: &impl Grants) -> Capabilities {
    Capabilities {
        delete_chunks: Action::DeleteChunks.granted(grants),
        edit_chunks: Action::EditChunks.granted(grants),
        import_chunks: Action::ImportChunks.granted(grants),
        export_chunks: Action::ExportChunks.granted(grants),
        teleport: Action::Teleport.granted(grants),
        reset_heatmap: Action::ResetHeatmap.granted(grants),
        manage_worlds: Action::ManageWorlds.granted(grants),
        manage_players: Action::ManagePlayers.granted(grants),
    }
}

/// The panel's own error for a missing permission.
fn permission_denied(permissions: &PermissionManager, permission: &str) -> ApiResponse {
    if let Err(err) = permissions.has_server_permission(permission) {
        return err.into();
    }
    ApiResponse::error(format!("missing permission {permission}"))
        .with_status(StatusCode::FORBIDDEN)
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
        Err(Denied::Permission(permission)) => Err(permission_denied(permissions, permission)),
        Err(Denied::Running) => Err(ApiResponse::error(
            "stop the server first: a running server would write the chunks back",
        )
        .with_status(StatusCode::CONFLICT)),
        Err(Denied::NotRunning) => {
            Err(ApiResponse::error("the server is not running").with_status(StatusCode::CONFLICT))
        }
    }
}

/// Checks only an action's permissions, whatever the power state: for routes that need a write
/// action's rights but touch no world file (creating an upload staging folder, extracting an
/// upload into it).
pub fn authorize_permissions(
    action: Action,
    permissions: &PermissionManager,
) -> Result<(), ApiResponse> {
    match action.missing(permissions) {
        None => Ok(()),
        Some(permission) => Err(permission_denied(permissions, permission)),
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

    fn without(missing: &str) -> Set {
        set(&ALL
            .iter()
            .copied()
            .filter(|p| *p != missing)
            .collect::<Vec<_>>())
    }

    const ALL: &[&str] = &[
        VIEW,
        DELETE_CHUNKS,
        EDIT_CHUNKS,
        IMPORT_CHUNKS,
        TELEPORT,
        RESET_HEATMAP,
        MANAGE_WORLDS,
        MANAGE_PLAYERS,
        READ_FILES,
        CREATE_FILES,
        UPDATE_FILES,
        DELETE_FILES,
        CONSOLE,
        READ_CONSOLE,
    ];

    const WRITES: &[(Action, &[&str])] = &[
        (
            Action::DeleteChunks,
            &[VIEW, READ_FILES, DELETE_CHUNKS, CREATE_FILES, UPDATE_FILES, DELETE_FILES],
        ),
        (
            Action::EditChunks,
            &[VIEW, READ_FILES, EDIT_CHUNKS, CREATE_FILES, UPDATE_FILES],
        ),
        (
            Action::ImportChunks,
            &[VIEW, READ_FILES, IMPORT_CHUNKS, CREATE_FILES, UPDATE_FILES],
        ),
    ];

    #[test]
    fn write_actions_need_every_permission_and_a_stopped_server() {
        let all = set(ALL);
        for &(action, needed) in WRITES {
            assert!(action.writes());
            assert_eq!(action.check(&all, Some(Power::Offline)), Ok(()), "{action:?}");
            for state in [Power::Running, Power::Starting, Power::Stopping] {
                assert_eq!(
                    action.check(&all, Some(state)),
                    Err(Denied::Running),
                    "{action:?} {state:?}"
                );
            }
            // an unknown state never counts as stopped
            assert_eq!(action.check(&all, None), Err(Denied::Running), "{action:?}");
            for &missing in needed {
                assert_eq!(
                    action.check(&without(missing), Some(Power::Offline)),
                    Err(Denied::Permission(missing)),
                    "{action:?} {missing}"
                );
                assert!(!action.granted(&without(missing)));
            }
            // permissions are checked before the power state
            assert_eq!(
                action.check(&without(VIEW), Some(Power::Running)),
                Err(Denied::Permission(VIEW))
            );
            assert_eq!(action.permissions(), needed);
        }
        // only deleting needs files.delete
        assert_eq!(
            Action::EditChunks.check(&without(DELETE_FILES), Some(Power::Offline)),
            Ok(())
        );
        assert_eq!(
            Action::ImportChunks.check(&without(DELETE_FILES), Some(Power::Offline)),
            Ok(())
        );
    }

    #[test]
    fn exporting_reads_files_in_any_power_state() {
        assert!(!Action::ExportChunks.writes());
        for power in [
            None,
            Some(Power::Offline),
            Some(Power::Running),
            Some(Power::Starting),
            Some(Power::Stopping),
        ] {
            assert_eq!(
                Action::ExportChunks.check(&set(&[VIEW, READ_FILES]), power),
                Ok(())
            );
        }
        assert_eq!(
            Action::ExportChunks.check(&set(&[VIEW]), None),
            Err(Denied::Permission(READ_FILES))
        );
        assert_eq!(
            Action::ExportChunks.check(&set(&[READ_FILES]), None),
            Err(Denied::Permission(VIEW))
        );
    }

    #[test]
    fn restores_need_the_action_of_the_restored_kind() {
        assert_eq!(
            restore_action(OperationKind::Delete),
            Some(Action::DeleteChunks)
        );
        assert_eq!(
            restore_action(OperationKind::Change),
            Some(Action::EditChunks)
        );
        assert_eq!(
            restore_action(OperationKind::Import),
            Some(Action::ImportChunks)
        );
        assert_eq!(restore_action(OperationKind::Restore), None);
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
        assert_eq!(Action::Teleport.check(&all, None), Err(Denied::NotRunning));
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
            Action::ResetHeatmap.check(&set(&[VIEW]), None),
            Err(Denied::Permission(RESET_HEATMAP))
        );
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
                edit_chunks: false,
                import_chunks: false,
                export_chunks: true,
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
                edit_chunks: true,
                import_chunks: true,
                export_chunks: true,
                teleport: true,
                reset_heatmap: true,
                manage_worlds: true,
                manage_players: true,
            }
        );
        assert!(may_list_by_console(&everything));
        let editor = without(DELETE_FILES);
        let offered = capabilities(&editor);
        assert!(!offered.delete_chunks);
        assert!(offered.edit_chunks && offered.import_chunks);
    }
}
