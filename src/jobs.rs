//! Background jobs for the chunk tools: whole-world filters and every write (deleting, changing,
//! importing chunks, restoring backups) run here instead of inside a request, report their
//! progress in regions, and can be cancelled between regions. Jobs live in the panel's memory:
//! finished ones are kept for an hour for the page to pick up their result.
use axum::http::StatusCode;
use serde::Serialize;
use shared::response::ApiResponse;
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;

/// Finished jobs are forgotten after this many seconds.
const KEEP_FINISHED: i64 = 3600;
/// Jobs listed per server.
const LIST: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Filter,
    Delete,
    Change,
    Import,
    Restore,
}

impl Kind {
    /// Whether the job writes the server's files (one such job per server at a time).
    pub fn writes(self) -> bool {
        self != Self::Filter
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// What the page sees of a job.
#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub id: Uuid,
    pub kind: Kind,
    pub state: JobState,
    pub world: String,
    pub dimension: String,
    pub user: Option<Uuid>,
    /// Unix seconds.
    pub started: i64,
    pub finished: Option<i64>,
    pub progress: Progress,
    pub message: Option<String>,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}

/// Why a job stopped early.
#[derive(Debug)]
pub enum Stop {
    Cancelled,
    Failed(String),
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<&str> for Stop {
    fn from(message: &str) -> Self {
        Self::Failed(message.to_string())
    }
}

/// The text of an error response (its `errors` list), for a job's error message.
pub async fn describe(response: ApiResponse) -> String {
    let status = response.status;
    let body = axum::body::to_bytes(response.body, 64 << 10)
        .await
        .unwrap_or_default();
    #[derive(serde::Deserialize)]
    struct Errors {
        errors: Vec<String>,
    }
    match serde_json::from_slice::<Errors>(&body) {
        Ok(errors) if !errors.errors.is_empty() => errors.errors.join("; "),
        _ => format!("request failed ({status})"),
    }
}

/// A running job's side of its state: progress, messages, cancellation.
pub struct Handle {
    server: Uuid,
    job: Mutex<Job>,
    cancelled: AtomicBool,
}

impl Handle {
    fn lock(&self) -> std::sync::MutexGuard<'_, Job> {
        self.job.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub fn snapshot(&self) -> Job {
        self.lock().clone()
    }

    pub fn progress(&self, done: u64, total: u64) {
        self.lock().progress = Progress { done, total };
    }

    pub fn message(&self, message: impl Into<String>) {
        self.lock().message = Some(message.into());
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// `Err(Stop::Cancelled)` once the user cancelled: call between regions.
    pub fn check(&self) -> Result<(), Stop> {
        if self.is_cancelled() {
            Err(Stop::Cancelled)
        } else {
            Ok(())
        }
    }
}

static JOBS: LazyLock<Mutex<HashMap<Uuid, Arc<Handle>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn jobs() -> std::sync::MutexGuard<'static, HashMap<Uuid, Arc<Handle>>> {
    JOBS.lock().unwrap_or_else(|err| err.into_inner())
}

fn forget_old(jobs: &mut HashMap<Uuid, Arc<Handle>>) {
    let now = chrono::Utc::now().timestamp();
    jobs.retain(|_, handle| {
        handle
            .lock()
            .finished
            .is_none_or(|finished| now - finished < KEEP_FINISHED)
    });
}

/// Starts `work` as a job of `server`. A writing job is refused (409) while another writing job
/// of the same server runs. `work` returns the job's result, or why it stopped; a cancelled job
/// keeps the partial result it returns through `Ok` when it checks [`Handle::is_cancelled`]
/// itself.
pub fn start<F, Fut>(
    server: Uuid,
    user: Option<Uuid>,
    kind: Kind,
    world: &str,
    dimension: &str,
    work: F,
) -> Result<Job, ApiResponse>
where
    F: FnOnce(Arc<Handle>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<serde_json::Value, Stop>> + Send + 'static,
{
    let handle = {
        let mut jobs = jobs();
        forget_old(&mut jobs);
        if kind.writes()
            && jobs.values().any(|other| {
                other.server == server && {
                    let job = other.lock();
                    job.state == JobState::Running && job.kind.writes()
                }
            })
        {
            return Err(ApiResponse::error(
                "another chunk operation is running on this server: wait for it or cancel it",
            )
            .with_status(StatusCode::CONFLICT));
        }
        let handle = Arc::new(Handle {
            server,
            job: Mutex::new(Job {
                id: Uuid::new_v4(),
                kind,
                state: JobState::Running,
                world: world.to_string(),
                dimension: dimension.to_string(),
                user,
                started: chrono::Utc::now().timestamp(),
                finished: None,
                progress: Progress::default(),
                message: None,
                error: None,
                result: None,
            }),
            cancelled: AtomicBool::new(false),
        });
        jobs.insert(handle.lock().id, handle.clone());
        handle
    };
    let job = handle.snapshot();
    let runner = handle.clone();
    tokio::spawn(async move {
        // a panic inside the work still finishes the job
        let outcome = tokio::spawn(work(runner.clone())).await;
        let mut job = runner.lock();
        job.finished = Some(chrono::Utc::now().timestamp());
        // a message describes the running work, not the outcome
        job.message = None;
        match outcome {
            Ok(Ok(result)) => {
                job.state = if runner.is_cancelled() {
                    JobState::Cancelled
                } else {
                    JobState::Done
                };
                job.result = Some(result);
            }
            Ok(Err(Stop::Cancelled)) => job.state = JobState::Cancelled,
            Ok(Err(Stop::Failed(error))) => {
                tracing::warn!(job = %job.id, "world map job failed: {error}");
                job.state = JobState::Failed;
                job.error = Some(error);
            }
            Err(err) => {
                tracing::error!(job = %job.id, "world map job crashed: {err}");
                job.state = JobState::Failed;
                job.error = Some("the job crashed; see the panel log".into());
            }
        }
    });
    Ok(job)
}

/// The job `id` of `server`.
pub fn get(server: Uuid, id: Uuid) -> Option<Job> {
    jobs()
        .get(&id)
        .filter(|handle| handle.server == server)
        .map(|handle| handle.snapshot())
}

/// The server's jobs, newest first.
pub fn list(server: Uuid) -> Vec<Job> {
    let mut jobs = jobs();
    forget_old(&mut jobs);
    let mut list: Vec<Job> = jobs
        .values()
        .filter(|handle| handle.server == server)
        .map(|handle| handle.snapshot())
        .collect();
    list.sort_by_key(|job| std::cmp::Reverse(job.started));
    list.truncate(LIST);
    list
}

/// Asks the job to stop after the region it is working on.
pub fn cancel(server: Uuid, id: Uuid) -> Option<Job> {
    let jobs = jobs();
    let handle = jobs.get(&id).filter(|handle| handle.server == server)?;
    handle.cancelled.store(true, Ordering::Relaxed);
    Some(handle.snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn jobs_finish_report_and_refuse_a_second_writer() {
        let server = Uuid::new_v4();
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let first = start(
            server,
            None,
            Kind::Delete,
            "world",
            "minecraft:overworld",
            |handle| async move {
                handle.progress(1, 2);
                let _ = wait.await;
                Ok(serde_json::json!({ "chunks": 3 }))
            },
        )
        .unwrap();
        assert_eq!(first.state, JobState::Running);
        // a second writer on the same server waits its turn, a filter may run alongside
        assert!(
            start(
                server,
                None,
                Kind::Change,
                "world",
                "minecraft:overworld",
                |_| async { Ok(serde_json::Value::Null) }
            )
            .is_err()
        );
        let filter = start(
            server,
            None,
            Kind::Filter,
            "world",
            "minecraft:overworld",
            |_| async { Err(Stop::Failed("broken".into())) },
        )
        .unwrap();
        // another server is not affected
        assert!(
            start(Uuid::new_v4(), None, Kind::Change, "w", "d", |_| async {
                Err(Stop::Cancelled)
            })
            .is_ok()
        );
        release.send(()).unwrap();
        for _ in 0..100 {
            if get(server, first.id).unwrap().state != JobState::Running
                && get(server, filter.id).unwrap().state != JobState::Running
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let first = get(server, first.id).unwrap();
        assert_eq!(first.state, JobState::Done);
        assert_eq!(first.progress, Progress { done: 1, total: 2 });
        assert_eq!(first.result, Some(serde_json::json!({ "chunks": 3 })));
        let filter = get(server, filter.id).unwrap();
        assert_eq!(filter.state, JobState::Failed);
        assert_eq!(filter.error.as_deref(), Some("broken"));
        assert_eq!(list(server).len(), 2);
        // jobs of other servers stay invisible
        assert!(get(Uuid::new_v4(), first.id).is_none());
        assert!(cancel(Uuid::new_v4(), first.id).is_none());
    }

    #[tokio::test]
    async fn cancelling_stops_between_regions() {
        let server = Uuid::new_v4();
        let job = start(server, None, Kind::Filter, "w", "d", |handle| async move {
            for _ in 0..1000 {
                handle.check()?;
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            Ok(serde_json::Value::Null)
        })
        .unwrap();
        cancel(server, job.id).unwrap();
        for _ in 0..200 {
            if get(server, job.id).unwrap().state != JobState::Running {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(get(server, job.id).unwrap().state, JobState::Cancelled);
    }
}
