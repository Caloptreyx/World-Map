//! Server access through Wings (files, power state, console, query tunnels), with the same
//! error mapping as the panel's own routes. Adapted from Caloptreyx/Player-Manager (MIT).
use crate::tunnel::TunnelError;
use axum::http::StatusCode;
use shared::{
    ApiError, State,
    database::Database,
    models::{node::Node, server::Server},
    response::ApiResponse,
};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use wings_api::{
    client::{ApiHttpError, AsyncRequestReader, WingsClient},
    tunnel::{QueryStreamTunnel, QueryUdpTunnel},
};

const LIST_PER_PAGE: u64 = 200;
const LIST_MAX_PAGES: u64 = 50;
/// Longest wait for a file operation Wings runs in the background (copy, decompress).
pub const OPERATION_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

fn wings_status(status: StatusCode, err: wings_api::ApiError) -> ApiResponse {
    ApiResponse::new_serialized(ApiError::new_wings_value(err)).with_status(status)
}

fn tunnel_error(err: ApiHttpError) -> TunnelError {
    TunnelError::failed(match err {
        ApiHttpError::Http(status, err) => {
            format!("Wings could not open a tunnel ({status}): {}", err.error)
        }
        ApiHttpError::WebSocket(err) => format!("Wings could not open a tunnel: {err}"),
        other => format!("Wings could not open a tunnel: {other:?}"),
    })
}

pub struct Wings {
    client: WingsClient,
    node: Node,
    database: Arc<Database>,
    server: uuid::Uuid,
    ignored: Option<Vec<compact_str::CompactString>>,
    address: Option<(String, u16)>,
}

impl Wings {
    pub async fn connect(state: &State, server: &Server) -> Result<Self, ApiResponse> {
        let ignored = server.subuser_ignored_files.clone();
        let node = server.node.fetch_cached(&state.database).await?;
        let client = node
            .api_client(&state.database)
            .await?
            .ignoring(ignored.clone().unwrap_or_default());
        let address = server.allocation.as_ref().and_then(|allocation| {
            let allocation = &allocation.allocation;
            let port = u16::try_from(allocation.port).ok()?;
            let host = match allocation.ip_alias.as_deref() {
                Some(alias) => alias.to_string(),
                None => allocation.ip.ip().to_string(),
            };
            Some((host, port))
        });
        Ok(Self {
            client,
            node,
            database: state.database.clone(),
            server: server.uuid,
            ignored,
            address,
        })
    }

    /// Every entry of the directory (up to [`LIST_MAX_PAGES`] pages); `None` when it does not
    /// exist.
    pub async fn list(
        &self,
        directory: &str,
    ) -> Result<Option<Vec<wings_api::DirectoryEntry>>, ApiResponse> {
        let mut query = wings_api::servers_server_files_list::get::Query {
            directory: Some(directory.into()),
            ignored: self.ignored.clone(),
            per_page: Some(LIST_PER_PAGE),
            ..Default::default()
        };
        let mut entries = Vec::new();
        for page in 1..=LIST_MAX_PAGES {
            query.page = Some(page);
            let response = match self
                .client
                .get_servers_server_files_list(self.server, &query)
                .await
            {
                Ok(response) => response,
                Err(ApiHttpError::Http(StatusCode::NOT_FOUND, _)) => return Ok(None),
                Err(err) => return Err(err.into()),
            };
            let received = response.entries.len() as u64;
            entries.extend(response.entries);
            if received < LIST_PER_PAGE || entries.len() as u64 >= response.total {
                break;
            }
        }
        Ok(Some(entries))
    }

    /// The file's bytes, at most `max_size` of them (413 beyond); `None` when it does not exist.
    pub async fn read(&self, path: &str, max_size: u64) -> Result<Option<Vec<u8>>, ApiResponse> {
        let reader = match self
            .client
            .get_servers_server_files_contents(
                self.server,
                &wings_api::servers_server_files_contents::get::Query {
                    file: Some(path.into()),
                    max_size: Some(max_size),
                    ignored: self.ignored.clone(),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(reader) => reader,
            Err(ApiHttpError::Http(StatusCode::NOT_FOUND, _)) => return Ok(None),
            Err(ApiHttpError::Http(StatusCode::PAYLOAD_TOO_LARGE, err)) => {
                return Err(wings_status(StatusCode::PAYLOAD_TOO_LARGE, err));
            }
            Err(err) => return Err(err.into()),
        };

        let mut data = Vec::new();
        reader
            .take(max_size.saturating_add(1))
            .read_to_end(&mut data)
            .await?;
        if data.len() as u64 > max_size {
            return Err(ApiResponse::error(format!("{path} is too large"))
                .with_status(StatusCode::PAYLOAD_TOO_LARGE));
        }
        Ok(Some(data))
    }

    /// Writes the file as `user`; the Wings revision id of the new content.
    pub async fn write(
        &self,
        path: &str,
        user: uuid::Uuid,
        data: Vec<u8>,
    ) -> Result<Option<i64>, ApiResponse> {
        match self
            .client
            .post_servers_server_files_write(
                self.server,
                AsyncRequestReader::new(std::io::Cursor::new(data)),
                &wings_api::servers_server_files_write::post::Query {
                    file: Some(path.into()),
                    user: Some(user),
                    ignored: self.ignored.clone(),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(response) => Ok(response.revision_id),
            Err(ApiHttpError::Http(
                status @ (StatusCode::NOT_FOUND | StatusCode::EXPECTATION_FAILED),
                err,
            )) => Err(wings_status(status, err)),
            Err(err) => Err(err.into()),
        }
    }

    /// Moves `from` to `to` (both relative to the server root; a leading `/` is ignored),
    /// creating `to`'s parent folders. Wings never overwrites: `false` when nothing was renamed
    /// (the source is missing or the target exists).
    pub async fn rename(&self, from: &str, to: &str) -> Result<bool, ApiResponse> {
        let request_body = wings_api::servers_server_files_rename::put::RequestBody {
            root: "/".into(),
            files: vec![
                wings_api::servers_server_files_rename::put::RequestBodyFiles {
                    from: from.trim_start_matches('/').into(),
                    to: to.trim_start_matches('/').into(),
                },
            ],
        };
        let extra = wings_api::servers_server_files_rename::put::Extra {
            create_directories: true,
            ..Default::default()
        };
        match self
            .client
            .put_servers_server_files_rename_with(self.server, &request_body, &extra)
            .await
        {
            Ok(response) => Ok(response.renamed == 1),
            Err(ApiHttpError::Http(StatusCode::NOT_FOUND, _)) => Ok(false),
            Err(ApiHttpError::Http(StatusCode::EXPECTATION_FAILED, err)) => {
                Err(wings_status(StatusCode::EXPECTATION_FAILED, err))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Deletes `files` (names below `root`) permanently, not through the trash; how many were
    /// deleted (missing ones count 0).
    pub async fn delete(&self, root: &str, files: &[String]) -> Result<u64, ApiResponse> {
        if files.is_empty() {
            return Ok(0);
        }
        let request_body = wings_api::servers_server_files_delete::post::RequestBody {
            root: root.into(),
            files: files.iter().map(|file| file.as_str().into()).collect(),
        };
        match self
            .client
            .post_servers_server_files_delete(self.server, &request_body)
            .await
        {
            Ok(response) => Ok(response.deleted),
            Err(ApiHttpError::Http(StatusCode::NOT_FOUND, _)) => Ok(0),
            Err(ApiHttpError::Http(StatusCode::EXPECTATION_FAILED, err)) => {
                Err(wings_status(StatusCode::EXPECTATION_FAILED, err))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Creates the folder `name` (may contain `/`) below `root`.
    pub async fn create_directory(&self, root: &str, name: &str) -> Result<(), ApiResponse> {
        let request_body = wings_api::servers_server_files_create_directory::post::RequestBody {
            root: root.into(),
            name: name.into(),
        };
        match self
            .client
            .post_servers_server_files_create_directory(self.server, &request_body)
            .await
        {
            Ok(_) => Ok(()),
            Err(ApiHttpError::Http(
                status @ (StatusCode::NOT_FOUND | StatusCode::EXPECTATION_FAILED),
                err,
            )) => Err(wings_status(status, err)),
            Err(err) => Err(err.into()),
        }
    }

    /// Creates the parent folder of `path` (relative to the server root) when it is missing.
    async fn ensure_parent(&self, path: &str) -> Result<(), ApiResponse> {
        let path = path.trim_matches('/');
        if let Some((parent, _)) = path.rsplit_once('/')
            && self.list(&format!("/{parent}")).await?.is_none()
        {
            self.create_directory("/", parent).await?;
        }
        Ok(())
    }

    /// Copies the file `from` to the new path `to` (both relative to the server root), creating
    /// `to`'s parent folder first; never overwrites (409 when `to` exists). Waits for Wings to
    /// finish, at most [`OPERATION_WAIT`].
    pub async fn copy(&self, from: &str, to: &str) -> Result<(), ApiResponse> {
        self.ensure_parent(to).await?;
        let request_body = wings_api::servers_server_files_copy::post::RequestBody {
            path: format!("/{}", from.trim_start_matches('/')).into(),
            name: Some(format!("/{}", to.trim_start_matches('/')).into()),
            overwrite: false,
            foreground: true,
        };
        match self
            .client
            .post_servers_server_files_copy(self.server, &request_body)
            .await
        {
            Ok(wings_api::servers_server_files_copy::post::Response::Ok(_)) => Ok(()),
            Ok(wings_api::servers_server_files_copy::post::Response::Accepted(_)) => {
                self.wait_for_operations().await
            }
            Err(ApiHttpError::Http(
                status @ (StatusCode::NOT_FOUND
                | StatusCode::CONFLICT
                | StatusCode::EXPECTATION_FAILED),
                err,
            )) => Err(wings_status(status, err)),
            Err(err) => Err(err.into()),
        }
    }

    /// Extracts the archive `file` (a name below `root`) into `root`. Asks Wings to work in the
    /// foreground; when it still runs the extraction in the background, waits until the server
    /// has no file operation left, at most [`OPERATION_WAIT`].
    pub async fn decompress(&self, root: &str, file: &str) -> Result<(), ApiResponse> {
        let request_body = wings_api::servers_server_files_decompress::post::RequestBody {
            root: root.into(),
            file: file.into(),
            foreground: true,
        };
        match self
            .client
            .post_servers_server_files_decompress(self.server, &request_body)
            .await
        {
            Ok(wings_api::servers_server_files_decompress::post::Response::Ok(_)) => Ok(()),
            Ok(wings_api::servers_server_files_decompress::post::Response::Accepted(_)) => {
                self.wait_for_operations().await
            }
            Err(ApiHttpError::Http(
                status @ (StatusCode::NOT_FOUND | StatusCode::EXPECTATION_FAILED),
                err,
            )) => Err(wings_status(status, err)),
            Err(err) => Err(err.into()),
        }
    }

    /// Polls the server's running file operations until none is left (Wings' operation list
    /// carries no identifiers to wait for one in particular); 504 after [`OPERATION_WAIT`].
    async fn wait_for_operations(&self) -> Result<(), ApiResponse> {
        let deadline = tokio::time::Instant::now() + OPERATION_WAIT;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let running = self
                .client
                .get_servers_server_files_operations(self.server)
                .await?;
            if running.operations.is_empty() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ApiResponse::error("Wings did not finish the file operation in time")
                    .with_status(StatusCode::GATEWAY_TIMEOUT));
            }
        }
    }

    /// The server's power state straight from the node (not the panel's 15 second cache), for
    /// checks right before files are changed.
    pub async fn power_state_fresh(&self) -> Result<wings_api::ServerState, ApiResponse> {
        self.client
            .get_servers_utilization()
            .await?
            .get(&self.server)
            .map(|usage| usage.state)
            .ok_or_else(|| {
                ApiResponse::error("the node did not report this server")
                    .with_status(StatusCode::BAD_GATEWAY)
            })
    }

    /// The server's power state from the node's resource report, as the panel shows it (cached
    /// by the panel for 15 seconds). Wings' single-server endpoint is not used: its payload
    /// differs between Wings versions.
    pub async fn power_state(&self) -> Result<wings_api::ServerState, ApiResponse> {
        self.node
            .fetch_server_resources(&self.database)
            .await?
            .get(&self.server)
            .map(|usage| usage.state)
            .ok_or_else(|| {
                ApiResponse::error("the node did not report this server")
                    .with_status(StatusCode::BAD_GATEWAY)
            })
    }

    /// Sends console commands in one request (409 when the server is not running).
    pub async fn commands(&self, commands: &[&str]) -> Result<(), ApiResponse> {
        let request_body = wings_api::servers_server_commands::post::RequestBody {
            commands: commands.iter().map(|&command| command.into()).collect(),
        };
        match self
            .client
            .post_servers_server_commands(self.server, &request_body)
            .await
        {
            Ok(_) => Ok(()),
            Err(ApiHttpError::Http(StatusCode::EXPECTATION_FAILED, err)) => {
                Err(wings_status(StatusCode::CONFLICT, err))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// The last `lines` lines of the console log, as (lossy) text.
    pub async fn logs(&self, lines: u64) -> Result<String, ApiResponse> {
        let mut reader = self
            .client
            .get_servers_server_logs(
                self.server,
                &wings_api::servers_server_logs::get::Query {
                    lines: Some(lines),
                    ..Default::default()
                },
            )
            .await?;
        let mut data = Vec::new();
        reader.read_to_end(&mut data).await?;
        Ok(String::from_utf8_lossy(&data).into_owned())
    }

    /// The port of the primary allocation and the address players use with it.
    pub fn address(&self) -> Option<&(String, u16)> {
        self.address.as_ref()
    }

    /// A TCP tunnel to `port` of the server's container (no timeout of its own).
    pub async fn tcp(&self, port: u16) -> Result<QueryStreamTunnel, TunnelError> {
        self.client
            .open_tunnel_tcp(self.server, port)
            .await
            .map_err(tunnel_error)
    }

    /// A UDP tunnel to `port` of the server's container.
    pub async fn udp(&self, port: u16) -> Result<QueryUdpTunnel, TunnelError> {
        self.client
            .open_tunnel_udp(self.server, port)
            .await
            .map_err(tunnel_error)
    }
}
