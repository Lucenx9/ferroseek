// SPDX-License-Identifier: GPL-3.0-or-later
//! Session-bus side of the launcher, on its own thread.
//!
//! zbus's default runtime drives a connection only from inside
//! `zbus::block_on`, so one thread sits in it for the life of the process:
//! it serves the single-instance interface and forwards file searches and
//! opens to the daemon's `io.github.ferroseek.Search1` object. The UI talks to it
//! through channels, never by awaiting zbus futures on its own executor.

use std::path::PathBuf;
use std::time::Duration;

use ferroseek_config::Config;
use iced::futures::StreamExt;
use iced::futures::channel::{mpsc, oneshot};
use zbus::fdo::{RequestNameFlags, RequestNameReply};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDetails {
    pub mime: String,
    pub size: u64,
    pub modified: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub enum TextPreview {
    Text { text: String, truncated: bool },
    Unsupported(String),
}

// File contents must not leak into diagnostic messages.
impl std::fmt::Debug for TextPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text { text, truncated } => {
                f.debug_struct("TextPreview").field("bytes", &text.len()).field("truncated", truncated).finish()
            }
            Self::Unsupported(reason) => f.debug_tuple("Unsupported").field(reason).finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreviewData {
    pub content: Result<TextPreview, String>,
    pub details: Result<FileDetails, String>,
}

/// Validity of a UI preview which may still be waiting behind another request.
#[derive(Clone)]
pub struct PreviewRequest(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Default for PreviewRequest {
    fn default() -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)))
    }
}

impl PreviewRequest {
    pub fn cancel(&self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
    pub fn is_active(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// The daemon (mirrors `ferroseek_engine::ipc`; checked by a test).
pub const DAEMON_BUS_NAME: &str = "io.github.ferroseek.FerroSeek";
pub const SEARCH_PATH: &str = "/io/github/ferroseek/Search";
const SEARCH_IFACE: &str = "io.github.ferroseek.Search1";
pub const CONTROL_PATH: &str = "/io/github/ferroseek/FerroSeek";
pub const CONTROL_IFACE: &str = "io.github.ferroseek.FerroSeek1";
/// The file search service's reveal action.
pub const ACTION_OPEN_PARENT: &str = "open-parent";

pub const LAUNCHER_BUS_NAME: &str = "io.github.ferroseek.Launcher";
pub const LAUNCHER_PATH: &str = "/io/github/ferroseek/Launcher";
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
const STATUS_TIMED_OUT: &str = "Search status timed out. Try again.";
const SEARCH_TIMED_OUT: &str = "File search timed out. Try another query or check index status.";
const OPEN_TIMED_OUT: &str = "No confirmation received before the open request timed out.";

#[derive(Clone, Copy)]
struct Timeouts {
    search: Duration,
    open: Duration,
    status: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { search: Duration::from_secs(5), open: Duration::from_secs(15), status: STATUS_TIMEOUT }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    Failed(String),
    /// The request may have taken effect; it must not be sent again automatically.
    Unconfirmed(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(message) | Self::Unconfirmed(message) => f.write_str(message),
        }
    }
}

fn open_error(error: zbus::Error) -> RunError {
    // Only known pre-dispatch/validation rejections prove the action was not
    // performed. NoReply and transport errors can arrive after it took effect.
    let rejected = matches!(&error, zbus::Error::FDO(e) if matches!(e.as_ref(),
        zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)
        | zbus::fdo::Error::UnknownObject(_) | zbus::fdo::Error::UnknownInterface(_)
        | zbus::fdo::Error::UnknownMethod(_) | zbus::fdo::Error::InvalidArgs(_)
        | zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::SpawnExecFailed(_)));
    if rejected { RunError::Failed(error.to_string()) } else { RunError::Unconfirmed(error.to_string()) }
}

/// One file or folder result from the daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct FileHit {
    /// Opaque match ID (a `file://` URL), passed back to `Open`.
    pub id: String,
    pub path: Option<PathBuf>,
    pub name: String,
    pub icon: String,
    /// Parent directory for display (`~/…`).
    pub subtext: String,
    pub is_dir: bool,
}

/// Read-only engine state and live index coverage from the daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchStatus {
    pub state: String,
    pub files: u64,
    pub dirs: u64,
    pub queued_dirs: u64,
    pub watch_limit_hit: bool,
    pub errors: u64,
    pub roots: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FolderPolicy {
    pub config_path: String,
    pub config: Config,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationChoice {
    pub id: String,
    pub name: String,
    pub icon: String,
}

/// Events the bus thread delivers to the UI.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// Another invocation asked this instance to toggle (close).
    Toggle,
}

enum Request {
    ListDirectory { folder: String, reply: oneshot::Sender<Result<(Vec<FileHit>, bool), String>> },
    Search { query: String, folder: Option<String>, reply: oneshot::Sender<Result<Vec<FileHit>, String>> },
    Run { id: String, action: String, reply: oneshot::Sender<Result<(), RunError>> },
    Applications { id: String, reply: oneshot::Sender<Result<Vec<ApplicationChoice>, String>> },
    TextPreview { id: String, validity: PreviewRequest, reply: oneshot::Sender<Result<PreviewData, String>> },
    FileDetails { id: String, reply: oneshot::Sender<Result<FileDetails, String>> },
    RunWith { id: String, app: String, reply: oneshot::Sender<Result<(), RunError>> },
    Status { reply: oneshot::Sender<Result<SearchStatus, String>> },
    FolderPolicy { reply: oneshot::Sender<Result<Option<FolderPolicy>, String>> },
    ApplyFolders { path: String, roots: Vec<String>, reply: oneshot::Sender<Result<bool, String>> },
}

/// Handle to the bus thread. Cloning shares the thread.
#[derive(Clone)]
pub struct Backend {
    tx: mpsc::UnboundedSender<Request>,
}

pub enum Startup {
    /// This process is the instance; the UI should start.
    Primary(Backend, mpsc::UnboundedReceiver<UiEvent>),
    /// An instance was already running and has been told to toggle.
    Toggled,
}

struct LauncherIface {
    events: mpsc::UnboundedSender<UiEvent>,
}

#[zbus::interface(name = "io.github.ferroseek.Launcher1")]
impl LauncherIface {
    /// Called by a second `ferroseek-launcher` invocation.
    async fn toggle(&self) {
        let _ = self.events.unbounded_send(UiEvent::Toggle);
    }
}

/// Connects to the session bus and claims the single-instance name.
pub fn start() -> Result<Startup, String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<Startup, String>>();
    std::thread::Builder::new()
        .name("bus".into())
        .spawn(move || {
            zbus::block_on(async move {
                let conn = match zbus::Connection::session().await {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("session bus unavailable: {e}")));
                        return;
                    }
                };
                let (ev_tx, ev_rx) = mpsc::unbounded();
                // Export before owning the name, so a caller that sees the
                // name can always reach the object.
                if let Err(e) = conn.object_server().at(LAUNCHER_PATH, LauncherIface { events: ev_tx }).await {
                    let _ = ready_tx.send(Err(format!("cannot export {LAUNCHER_PATH}: {e}")));
                    return;
                }
                let reply = conn.request_name_with_flags(LAUNCHER_BUS_NAME, RequestNameFlags::DO_NOT_QUEUE).await;
                match reply {
                    Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
                    // zbus reports `Exists` as `Error::NameTaken`.
                    Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {
                        let r = conn
                            .call_method(
                                Some(LAUNCHER_BUS_NAME),
                                LAUNCHER_PATH,
                                Some("io.github.ferroseek.Launcher1"),
                                "Toggle",
                                &(),
                            )
                            .await;
                        let _ = ready_tx.send(match r {
                            Ok(_) => Ok(Startup::Toggled),
                            Err(e) => Err(format!("another launcher owns {LAUNCHER_BUS_NAME} but did not answer: {e}")),
                        });
                        return;
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("cannot own {LAUNCHER_BUS_NAME}: {e}")));
                        return;
                    }
                }
                let (tx, rx) = mpsc::unbounded();
                if ready_tx.send(Ok(Startup::Primary(Backend { tx }, ev_rx))).is_err() {
                    return;
                }
                serve(conn, rx, Timeouts::default()).await;
            });
        })
        .map_err(|e| format!("cannot start bus thread: {e}"))?;
    ready_rx.recv().map_err(|_| "bus thread exited during startup".to_string())?
}

async fn serve(conn: zbus::Connection, mut rx: mpsc::UnboundedReceiver<Request>, timeouts: Timeouts) {
    let mut pending = std::collections::VecDeque::new();
    loop {
        if pending.is_empty() {
            let Some(request) = rx.next().await else { break };
            pending.push_back(request);
        }
        while let Ok(next) = rx.try_recv() {
            // Preview reads have no side effects. Only the latest queued target
            // matters, including when search requests sit between selections.
            if matches!(next, Request::TextPreview { .. }) {
                pending.retain(|request| !matches!(request, Request::TextPreview { .. }));
            }
            if matches!(next, Request::Search { .. } | Request::ListDirectory { .. })
                && matches!(pending.back(), Some(Request::Search { .. } | Request::ListDirectory { .. }))
            {
                pending.pop_back();
            }
            pending.push_back(next);
        }
        if let Some(request) = pending.pop_front() {
            handle(&conn, request, timeouts).await;
        }
    }
}

async fn load_text_preview(conn: &zbus::Connection, id: String, timeout: Duration) -> Result<TextPreview, String> {
    let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
    let response = runtime
        .timeout(
            timeout,
            conn.call_method(Some(DAEMON_BUS_NAME), SEARCH_PATH, Some(SEARCH_IFACE), "TextPreview", &(id,)),
        )
        .await
        .map_err(|_| "Text preview timed out. Refresh to try again.".to_string())?;
    let message = match response {
        Err(zbus::Error::FDO(error)) if matches!(error.as_ref(), zbus::fdo::Error::NotSupported(_)) => {
            let zbus::fdo::Error::NotSupported(reason) = *error else { unreachable!() };
            return Ok(TextPreview::Unsupported(reason));
        }
        Err(zbus::Error::MethodError(name, detail, _))
            if name.as_str() == "org.freedesktop.DBus.Error.NotSupported" =>
        {
            return Ok(TextPreview::Unsupported(
                detail.unwrap_or_else(|| "This file type has no text preview.".into()),
            ));
        }
        other => other.map_err(|e| e.to_string())?,
    };
    let (text, truncated) = message.body().deserialize().map_err(|e| e.to_string())?;
    Ok(TextPreview::Text { text, truncated })
}

async fn load_file_details(conn: &zbus::Connection, id: String, timeout: Duration) -> Result<FileDetails, String> {
    let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
    let message = runtime
        .timeout(
            timeout,
            conn.call_method(Some(DAEMON_BUS_NAME), SEARCH_PATH, Some(SEARCH_IFACE), "FileDetails", &(id,)),
        )
        .await
        .map_err(|_| "File details timed out. Refresh to try again.".to_string())?
        .map_err(|e| e.to_string())?;
    let (mime, size, modified) = message.body().deserialize().map_err(|e| e.to_string())?;
    Ok(FileDetails { mime, size, modified })
}

fn folder_read_error(error: zbus::Error) -> String {
    // Keep transport/interface details out of the normal recovery message.
    let denied = matches!(&error, zbus::Error::FDO(cause) if matches!(cause.as_ref(), zbus::fdo::Error::AccessDenied(_)))
        || matches!(&error, zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied");
    let outdated = matches!(&error, zbus::Error::FDO(cause) if matches!(cause.as_ref(), zbus::fdo::Error::UnknownMethod(_)))
        || matches!(&error, zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod");
    if denied {
        "Folder changed or is unavailable. Refresh or go back.".into()
    } else if outdated {
        "Folder navigation requires an updated daemon.".into()
    } else {
        "Could not load the folder. Check index status or refresh.".into()
    }
}

async fn handle(conn: &zbus::Connection, req: Request, timeouts: Timeouts) {
    match req {
        Request::ListDirectory { folder, reply } => {
            let result = async {
                let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
                let message = runtime
                    .timeout(
                        timeouts.search,
                        conn.call_method(
                            Some(DAEMON_BUS_NAME),
                            SEARCH_PATH,
                            Some(SEARCH_IFACE),
                            "ListDirectory",
                            &(folder,),
                        ),
                    )
                    .await
                    .map_err(|_| "Folder listing timed out. Refresh to try again.".to_string())?
                    .map_err(folder_read_error)?;
                let (entries, truncated): (Vec<FileResult>, bool) =
                    message.body().deserialize().map_err(|e| e.to_string())?;
                Ok((entries.into_iter().map(to_hit).collect(), truncated))
            }
            .await;
            let _ = reply.send(result);
        }
        Request::Search { query, folder, reply } => {
            let r = search(conn, &query, folder.as_deref(), timeouts.search).await;
            let _ = reply.send(r);
        }
        Request::Run { id, action, reply } => {
            let _ = reply.send(run(conn, "Open", &id, &action, timeouts.open).await);
        }
        Request::RunWith { id, app, reply } => {
            let _ = reply.send(run(conn, "OpenWith", &id, &app, timeouts.open).await);
        }
        Request::TextPreview { id, validity, reply } => {
            if !validity.is_active() {
                return;
            }
            let (content, details) = iced::futures::future::join(
                load_text_preview(conn, id.clone(), timeouts.status),
                load_file_details(conn, id, timeouts.status),
            )
            .await;
            let _ = reply.send(Ok(PreviewData { content, details }));
        }
        Request::FileDetails { id, reply } => {
            let _ = reply.send(load_file_details(conn, id, timeouts.status).await);
        }
        Request::Applications { id, reply } => {
            let r = async {
                let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
                let body = (id,);
                let message = runtime
                    .timeout(
                        timeouts.status,
                        conn.call_method(Some(DAEMON_BUS_NAME), SEARCH_PATH, Some(SEARCH_IFACE), "Applications", &body),
                    )
                    .await
                    .map_err(|_| "Application list timed out. Try again.".to_string())?
                    .map_err(|e| e.to_string())?;
                let apps: Vec<(String, String, String)> = message.body().deserialize().map_err(|e| e.to_string())?;
                Ok(apps.into_iter().map(|(id, name, icon)| ApplicationChoice { id, name, icon }).collect())
            }
            .await;
            let _ = reply.send(r);
        }
        Request::Status { reply } => {
            let r = file_search_status(conn, timeouts.status).await;
            let _ = reply.send(r);
        }
        Request::FolderPolicy { reply } => {
            let _ = reply.send(folder_policy(conn).await);
        }
        Request::ApplyFolders { path, roots, reply } => {
            let body = (path, roots);
            let r = async {
                let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
                let message = runtime
                    .timeout(
                        Duration::from_secs(30),
                        conn.call_method(
                            Some(DAEMON_BUS_NAME),
                            CONTROL_PATH,
                            Some(CONTROL_IFACE),
                            "ReloadFolders",
                            &body,
                        ),
                    )
                    .await
                    .map_err(|_| {
                        "The service did not confirm within 30 seconds. Check index status or save and apply again."
                            .to_string()
                    })?
                    .map_err(|e| e.to_string())?;
                message.body().deserialize::<bool>().map_err(|e| e.to_string())
            }
            .await;
            let _ = reply.send(r);
        }
    }
}

async fn run(
    conn: &zbus::Connection,
    method: &str,
    id: &str,
    argument: &str,
    timeout: Duration,
) -> Result<(), RunError> {
    let body = (id, argument, "");
    let request = conn.call_method(Some(DAEMON_BUS_NAME), SEARCH_PATH, Some(SEARCH_IFACE), method, &body);
    match zruntime::SharedRuntime::current() {
        Ok(runtime) => match runtime.timeout(timeout, request).await {
            Ok(result) => result.map(|_| ()).map_err(open_error),
            Err(_) => Err(RunError::Unconfirmed(OPEN_TIMED_OUT.into())),
        },
        Err(e) => Err(RunError::Failed(e.to_string())),
    }
}

async fn folder_policy(conn: &zbus::Connection) -> Result<Option<FolderPolicy>, String> {
    let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
    // A unique owner cannot be activated: viewing settings must not start an
    // unconfigured service and its default home-directory scan.
    let owner = runtime
        .timeout(
            STATUS_TIMEOUT,
            conn.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(DAEMON_BUS_NAME,),
            ),
        )
        .await
        .map_err(|_| STATUS_TIMED_OUT.to_string())?;
    let owner = match owner {
        Ok(message) => message.body().deserialize::<String>().map_err(|e| e.to_string())?,
        Err(zbus::Error::FDO(e)) if matches!(e.as_ref(), zbus::fdo::Error::NameHasNoOwner(_)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let message = runtime
        .timeout(
            STATUS_TIMEOUT,
            conn.call_method(Some(owner.as_str()), CONTROL_PATH, Some(CONTROL_IFACE), "FolderPolicy", &()),
        )
        .await
        .map_err(|_| STATUS_TIMED_OUT.to_string())?
        .map_err(|e| e.to_string())?;
    let (config_path, text): (String, String) = message.body().deserialize().map_err(|e| e.to_string())?;
    Ok(Some(FolderPolicy { config_path, config: Config::parse(&text).map_err(|e| e.to_string())? }))
}

type FileResult = (String, String, String, String, bool);
type StatusResult = (String, u64, u64, u64, bool, u64, Vec<String>);

async fn file_search_status(conn: &zbus::Connection, timeout: Duration) -> Result<SearchStatus, String> {
    let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
    let request = conn.call_method(Some(DAEMON_BUS_NAME), CONTROL_PATH, Some(CONTROL_IFACE), "FileSearchStatus", &());
    let msg = runtime
        .timeout(timeout, request)
        .await
        .map_err(|_| STATUS_TIMED_OUT.to_string())?
        .map_err(|e| e.to_string())?;
    let status: StatusResult = msg.body().deserialize().map_err(|e| e.to_string())?;
    Ok(to_status(status))
}

fn to_status((state, files, dirs, queued_dirs, watch_limit_hit, errors, roots): StatusResult) -> SearchStatus {
    SearchStatus { state, files, dirs, queued_dirs, watch_limit_hit, errors, roots }
}

async fn search(
    conn: &zbus::Connection,
    query: &str,
    folder: Option<&str>,
    timeout: Duration,
) -> Result<Vec<FileHit>, String> {
    // Calling the daemon's name D-Bus-activates it when it is not running.
    let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
    let msg = if let Some(folder) = folder {
        runtime
            .timeout(
                timeout,
                conn.call_method(
                    Some(DAEMON_BUS_NAME),
                    SEARCH_PATH,
                    Some(SEARCH_IFACE),
                    "SearchWithin",
                    &(folder, query),
                ),
            )
            .await
    } else {
        runtime
            .timeout(
                timeout,
                conn.call_method(Some(DAEMON_BUS_NAME), SEARCH_PATH, Some(SEARCH_IFACE), "Search", &(query,)),
            )
            .await
    }
    .map_err(|_| SEARCH_TIMED_OUT.to_string())?
    .map_err(|error| if folder.is_some() { folder_read_error(error) } else { error.to_string() })?;
    let matches: Vec<FileResult> = msg.body().deserialize().map_err(|e| e.to_string())?;
    Ok(matches.into_iter().map(to_hit).collect())
}

fn to_hit((id, name, icon, subtext, is_dir): FileResult) -> FileHit {
    let path = crate::uri::path_from_file_url(&id);
    FileHit { subtext, path, name, icon, is_dir, id }
}

impl Backend {
    pub async fn list_directory(self, folder: String) -> Result<(Vec<FileHit>, bool), String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .unbounded_send(Request::ListDirectory { folder, reply })
            .map_err(|_| "bus thread stopped".to_string())?;
        rx.await.map_err(|_| "bus thread stopped".to_string())?
    }

    pub async fn text_preview(self, id: String, validity: PreviewRequest) -> Result<PreviewData, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .unbounded_send(Request::TextPreview { id, validity, reply })
            .map_err(|_| "bus thread stopped".to_string())?;
        rx.await.map_err(|_| "bus thread stopped".to_string())?
    }

    pub async fn file_details(self, id: String) -> Result<FileDetails, String> {
        let (reply, rx) = oneshot::channel();
        self.tx.unbounded_send(Request::FileDetails { id, reply }).map_err(|_| "bus thread stopped".to_string())?;
        rx.await.map_err(|_| "bus thread stopped".to_string())?
    }

    pub async fn applications(self, id: String) -> Result<Vec<ApplicationChoice>, String> {
        let (reply, rx) = oneshot::channel();
        self.tx.unbounded_send(Request::Applications { id, reply }).map_err(|_| "bus thread stopped".to_string())?;
        rx.await.map_err(|_| "bus thread stopped".to_string())?
    }

    pub async fn run_with(self, id: String, app: String) -> Result<(), RunError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .unbounded_send(Request::RunWith { id, app, reply })
            .map_err(|_| RunError::Failed("bus thread stopped".into()))?;
        rx.await.map_err(|_| RunError::Unconfirmed("bus thread stopped before confirming the open request".into()))?
    }

    pub async fn folder_policy(self) -> Result<Option<FolderPolicy>, String> {
        let (reply, rx) = oneshot::channel();
        self.tx.unbounded_send(Request::FolderPolicy { reply }).map_err(|_| "bus thread stopped".to_string())?;
        rx.await.map_err(|_| "bus thread stopped".to_string())?
    }

    pub async fn apply_folders(self, path: String, roots: Vec<String>) -> Result<bool, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .unbounded_send(Request::ApplyFolders { path, roots, reply })
            .map_err(|_| "bus thread stopped".to_string())?;
        rx.await.map_err(|_| "bus thread stopped before confirming folder application".to_string())?
    }
    #[cfg(test)]
    pub fn disconnected() -> Self {
        let (tx, _rx) = mpsc::unbounded();
        Self { tx }
    }

    /// File results for `query`; `Err` if the daemon is unreachable. A
    /// superseded search resolves to `Ok(None)`.
    pub async fn search(self, query: String) -> Result<Option<Vec<FileHit>>, String> {
        self.search_in(None, query).await
    }

    pub async fn search_within(self, folder: String, query: String) -> Result<Option<Vec<FileHit>>, String> {
        self.search_in(Some(folder), query).await
    }

    async fn search_in(self, folder: Option<String>, query: String) -> Result<Option<Vec<FileHit>>, String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .unbounded_send(Request::Search { query, folder, reply })
            .map_err(|_| "bus thread stopped".to_string())?;
        match rx.await {
            Ok(r) => r.map(Some),
            Err(oneshot::Canceled) => Ok(None),
        }
    }

    /// Asks the daemon to open a match (`action` `""`) or reveal it
    /// (`ACTION_OPEN_PARENT`). The daemon only opens indexed paths.
    pub async fn run(self, id: String, action: &'static str) -> Result<(), RunError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .unbounded_send(Request::Run { id, action: action.to_string(), reply })
            .map_err(|_| RunError::Failed("bus thread stopped".into()))?;
        rx.await.map_err(|_| RunError::Unconfirmed("bus thread stopped before confirming the open request".into()))?
    }

    pub async fn status(self) -> Result<SearchStatus, String> {
        let (reply, rx) = oneshot::channel();
        self.tx.unbounded_send(Request::Status { reply }).map_err(|_| "bus thread stopped".to_string())?;
        rx.await.map_err(|_| "bus thread stopped".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StalledStatus;

    #[zbus::interface(name = "io.github.ferroseek.FerroSeek1")]
    impl StalledStatus {
        async fn file_search_status(&self) -> (String, u64, u64, u64, bool, u64, Vec<String>) {
            std::future::pending().await
        }
    }

    struct SearchService(std::sync::mpsc::Sender<(String, String)>);

    #[zbus::interface(name = "io.github.ferroseek.Search1")]
    impl SearchService {
        async fn text_preview(&self, id: &str) -> zbus::fdo::Result<(String, bool)> {
            self.0.send((id.into(), "preview".into())).unwrap();
            match id {
                "stall" => std::future::pending().await,
                "reject" => Err(zbus::fdo::Error::AccessDenied("file is unavailable".into())),
                "binary" => Err(zbus::fdo::Error::NotSupported("not UTF-8 text".into())),
                _ => Ok(("synthetic text".into(), true)),
            }
        }

        async fn file_details(&self, id: &str) -> zbus::fdo::Result<(String, u64, i64)> {
            self.0.send((id.into(), "details".into())).unwrap();
            match id {
                "stall" => std::future::pending().await,
                "reject" => Err(zbus::fdo::Error::AccessDenied("file is unavailable".into())),
                "metadata-error" => Err(zbus::fdo::Error::Failed("metadata unavailable".into())),
                _ => Ok(("application/pdf".into(), 123, -1)),
            }
        }

        async fn list_directory(&self, folder: &str) -> zbus::fdo::Result<(Vec<FileResult>, bool)> {
            self.0.send((folder.into(), "list".into())).unwrap();
            match folder {
                "stall" => std::future::pending().await,
                "old-daemon" => Err(zbus::fdo::Error::UnknownMethod("ListDirectory".into())),
                "reject" => Err(zbus::fdo::Error::AccessDenied("folder unavailable".into())),
                _ => Ok((
                    vec![(
                        "file:///synthetic/report.txt".into(),
                        "report.txt".into(),
                        String::new(),
                        "/synthetic".into(),
                        false,
                    )],
                    true,
                )),
            }
        }

        async fn search_within(&self, folder: &str, query: &str) -> zbus::fdo::Result<Vec<FileResult>> {
            self.0.send((folder.into(), query.into())).unwrap();
            match folder {
                "stall" => std::future::pending().await,
                "reject" => Err(zbus::fdo::Error::AccessDenied("unavailable directory".into())),
                _ => self.search(query).await,
            }
        }

        async fn search(&self, query: &str) -> zbus::fdo::Result<Vec<FileResult>> {
            match query {
                "stall" => return std::future::pending().await,
                "slow" => zruntime::SharedRuntime::current().unwrap().sleep(Duration::from_millis(20)).await,
                "reject" => return Err(zbus::fdo::Error::Failed("test search failure".into())),
                _ => {}
            }
            Ok(vec![(
                "file:///synthetic/report.txt".into(),
                "report.txt".into(),
                "".into(),
                "/synthetic".into(),
                false,
            )])
        }

        async fn open_with(&self, id: &str, application: &str, _token: &str) -> zbus::fdo::Result<()> {
            self.0.send((id.into(), application.into())).unwrap();
            Err(zbus::fdo::Error::SpawnExecFailed("test executable rejection".into()))
        }

        async fn open(&self, id: &str, action: &str, _token: &str) -> zbus::fdo::Result<()> {
            self.0.send((id.into(), action.into())).unwrap();
            match action {
                "stall" => std::future::pending().await,
                "slow" => {
                    zruntime::SharedRuntime::current().unwrap().sleep(Duration::from_millis(20)).await;
                    Ok(())
                }
                "reject" => Err(zbus::fdo::Error::AccessDenied("test rejection".into())),
                _ => Ok(()),
            }
        }
    }

    struct TestBus(std::process::Child);

    impl Drop for TestBus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn private_bus() -> (tempfile::TempDir, TestBus, String) {
        use std::io::BufRead;
        use std::process::{Command, Stdio};

        let temp = tempfile::tempdir().unwrap();
        let services = temp.path().join("services");
        std::fs::create_dir(&services).unwrap();
        let config = temp.path().join("bus.conf");
        std::fs::write(&config, format!(r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth>
  <servicedir>{}</servicedir>
  <policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy>
</busconfig>"#, services.display())).unwrap();
        // Explicit addresses and an empty activation directory exclude the
        // user's session bus, services and index from this regression.
        let mut bus = TestBus(
            Command::new("dbus-daemon")
                .arg("--config-file")
                .arg(&config)
                .args(["--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut address = String::new();
        std::io::BufReader::new(bus.0.stdout.take().unwrap()).read_line(&mut address).unwrap();
        assert!(!address.trim().is_empty(), "private bus did not start");

        (temp, bus, address)
    }

    #[test]
    fn stalled_status_releases_queued_search_and_open_requests() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (opened_tx, opened_rx) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(CONTROL_PATH, StalledStatus)
                .serve_at(SEARCH_PATH, SearchService(opened_tx))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim())
                // A watchdog makes a missing production bound fail, not hang.
                .method_timeout(Duration::from_secs(10))
                .build()
                .await
                .unwrap();
            let (tx, rx) = mpsc::unbounded();
            let (status_tx, status_rx) = oneshot::channel();
            let (search_tx, search_rx) = oneshot::channel();
            let (run_tx, run_rx) = oneshot::channel();
            tx.unbounded_send(Request::Status { reply: status_tx }).unwrap();
            tx.unbounded_send(Request::Search { folder: None, query: "report".into(), reply: search_tx }).unwrap();
            tx.unbounded_send(Request::Run {
                id: "file:///synthetic/report.txt".into(),
                action: ACTION_OPEN_PARENT.into(),
                reply: run_tx,
            })
            .unwrap();
            drop(tx);
            let verify = async {
                assert_eq!(status_rx.await.unwrap().unwrap_err(), STATUS_TIMED_OUT);
                assert_eq!(search_rx.await.unwrap().unwrap()[0].name, "report.txt");
                run_rx.await.unwrap().unwrap();
            };
            iced::futures::future::join(serve(conn, rx, Timeouts::default()), verify).await;
            assert_eq!(
                opened_rx.try_recv().unwrap(),
                ("file:///synthetic/report.txt".into(), ACTION_OPEN_PARENT.into())
            );
        });
    }

    #[test]
    fn daemon_names_match_the_engine() {
        assert_eq!(DAEMON_BUS_NAME, ferroseek_engine::ipc::BUS_NAME);
        assert_eq!(SEARCH_PATH, ferroseek_engine::ipc::SEARCH_PATH);
        assert_eq!(SEARCH_IFACE, ferroseek_engine::ipc::SEARCH_IFACE);
        assert_eq!(CONTROL_PATH, ferroseek_engine::ipc::CONTROL_PATH);
        assert_eq!(CONTROL_IFACE, ferroseek_engine::ipc::CONTROL_IFACE);
    }

    #[test]
    fn stalled_search_and_open_release_the_queue_without_retrying_open() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (opened_tx, opened_rx) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(opened_tx))
                .build()
                .await
                .unwrap();
            let timeouts = Timeouts {
                search: Duration::from_millis(500),
                open: Duration::from_millis(500),
                ..Timeouts::default()
            };
            for stalled_search in [true, false] {
                let conn = zbus::connection::Builder::address(address.trim())
                    .method_timeout(Duration::from_secs(2))
                    .build()
                    .await
                    .unwrap();
                let (tx, rx) = mpsc::unbounded();
                let (first_tx, first_rx) = oneshot::channel();
                let (run_tx, run_rx) = oneshot::channel();
                let (last_tx, last_rx) = oneshot::channel();
                if stalled_search {
                    tx.unbounded_send(Request::Search { folder: None, query: "stall".into(), reply: first_tx })
                        .unwrap();
                    tx.unbounded_send(Request::Run {
                        id: "file:///synthetic/report.txt".into(),
                        action: "slow".into(),
                        reply: run_tx,
                    })
                    .unwrap();
                } else {
                    // A read before and after the stalled open also proves the
                    // dispatcher remains usable when the open has no outcome.
                    tx.unbounded_send(Request::Search { folder: None, query: "slow".into(), reply: first_tx }).unwrap();
                    tx.unbounded_send(Request::Run {
                        id: "file:///synthetic/report.txt".into(),
                        action: "stall".into(),
                        reply: run_tx,
                    })
                    .unwrap();
                }
                tx.unbounded_send(Request::Search { folder: None, query: "report".into(), reply: last_tx }).unwrap();
                drop(tx);
                let verify = async {
                    let first = first_rx.await.unwrap();
                    let opened = run_rx.await.unwrap();
                    if stalled_search {
                        assert_eq!(first.unwrap_err(), SEARCH_TIMED_OUT);
                        opened.unwrap();
                    } else {
                        assert_eq!(first.unwrap()[0].name, "report.txt");
                        assert_eq!(opened, Err(RunError::Unconfirmed(OPEN_TIMED_OUT.into())));
                    }
                    assert_eq!(last_rx.await.unwrap().unwrap()[0].name, "report.txt");
                };
                iced::futures::future::join(serve(conn, rx, timeouts), verify).await;
                let (_, action) = opened_rx.try_recv().unwrap();
                assert_eq!(action, if stalled_search { "slow" } else { "stall" });
                assert!(opened_rx.try_recv().is_err(), "an open must never be retried automatically");
            }
        });
    }

    #[test]
    fn rejected_and_disconnected_opens_have_distinct_outcomes() {
        let (_temp, mut bus, address) = private_bus();
        zbus::block_on(async {
            let (opened_tx, opened_rx) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(opened_tx))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim())
                .method_timeout(Duration::from_secs(2))
                .build()
                .await
                .unwrap();
            let (tx, rx) = mpsc::unbounded();
            let backend = Backend { tx };
            let verify = async {
                assert!(matches!(
                    backend.clone().run("file:///synthetic/report.txt".into(), "reject").await,
                    Err(RunError::Failed(_))
                ));
                assert_eq!(opened_rx.try_recv().unwrap().1, "reject");
                for application in ["first.desktop", "second.desktop"] {
                    assert!(matches!(
                        backend.clone().run_with("file:///synthetic/report.txt".into(), application.into()).await,
                        Err(RunError::Failed(_))
                    ));
                    assert_eq!(opened_rx.try_recv().unwrap().1, application);
                }
                let open = backend.run("file:///synthetic/report.txt".into(), "stall");
                let disconnect = async {
                    let runtime = zruntime::SharedRuntime::current().unwrap();
                    for _ in 0..200 {
                        if let Ok((_, action)) = opened_rx.try_recv() {
                            assert_eq!(action, "stall");
                            bus.0.kill().unwrap();
                            return;
                        }
                        runtime.sleep(Duration::from_millis(5)).await;
                    }
                    panic!("open request was not dispatched");
                };
                let (result, ()) = iced::futures::future::join(open, disconnect).await;
                assert!(matches!(result, Err(RunError::Unconfirmed(_))));
                assert!(opened_rx.try_recv().is_err());
            };
            iced::futures::future::join(serve(conn, rx, Timeouts::default()), verify).await;
        });
    }

    #[test]
    fn file_details_timeout_releases_the_bus_queue_without_retrying() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (tx, requests) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(tx))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim())
                .method_timeout(Duration::from_secs(2))
                .build()
                .await
                .unwrap();
            let (tx, rx) = mpsc::unbounded();
            let backend = Backend { tx };
            let verify = async {
                assert_eq!(
                    backend.clone().file_details("stall".into()).await.unwrap_err(),
                    "File details timed out. Refresh to try again."
                );
                assert_eq!(requests.try_recv().unwrap().0, "stall");
                assert!(backend.clone().file_details("reject".into()).await.is_err());
                assert_eq!(requests.try_recv().unwrap().0, "reject");
                assert_eq!(
                    backend.clone().file_details("file:///synthetic/report.pdf".into()).await.unwrap(),
                    FileDetails { mime: "application/pdf".into(), size: 123, modified: -1 }
                );
                assert_eq!(requests.try_recv().unwrap().0, "file:///synthetic/report.pdf");
                assert!(requests.try_recv().is_err(), "reads must not retry automatically");
                assert!(backend.search("report".into()).await.unwrap().is_some());
            };
            iced::futures::future::join(
                serve(conn, rx, Timeouts { status: Duration::from_millis(40), ..Timeouts::default() }),
                verify,
            )
            .await;
        });
    }

    #[test]
    fn a_cancelled_queued_preview_never_reads_content_or_metadata() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (observed, requests) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(observed))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim()).build().await.unwrap();
            let (tx, rx) = mpsc::unbounded();
            let (search, search_reply) = oneshot::channel();
            tx.unbounded_send(Request::Search { query: "report".into(), folder: None, reply: search }).unwrap();
            let (reply, cancelled) = oneshot::channel();
            let validity = PreviewRequest::default();
            tx.unbounded_send(Request::TextPreview { id: "cancelled".into(), validity: validity.clone(), reply })
                .unwrap();
            validity.cancel();
            drop(tx);
            let verify = async {
                assert!(search_reply.await.unwrap().is_ok());
                assert!(cancelled.await.is_err());
                assert!(requests.try_recv().is_err(), "neither preview RPC may start after cancellation");
            };
            iced::futures::future::join(serve(conn, rx, Timeouts::default()), verify).await;
        });
    }

    #[test]
    fn search_intent_order_survives_reversed_future_polling() {
        use iced::futures::FutureExt;

        for listing in [false, true] {
            let (_temp, _bus, address) = private_bus();
            zbus::block_on(async {
                let (observed, requests) = std::sync::mpsc::channel();
                let _service = zbus::connection::Builder::address(address.trim())
                    .name(DAEMON_BUS_NAME)
                    .serve_at(SEARCH_PATH, SearchService(observed))
                    .build()
                    .await
                    .unwrap();
                let conn = zbus::connection::Builder::address(address.trim()).build().await.unwrap();
                let (tx, rx) = mpsc::unbounded();
                let backend = Backend { tx };
                let older = backend.clone().search_within("obsolete".into(), "report".into());
                let newer = if listing {
                    backend.clone().list_directory("latest".into()).map(|result| result.map(|_| true)).boxed_local()
                } else {
                    backend
                        .clone()
                        .search_within("latest".into(), "report".into())
                        .map(|result| result.map(|files| files.is_some()))
                        .boxed_local()
                };
                drop(backend);
                // Prime both completions in reverse order before serving the queue.
                // Scheduling must not change which UI intent supersedes the other.
                let replies = iced::futures::future::join(newer, older);
                iced::futures::pin_mut!(replies);
                assert!(iced::futures::poll!(&mut replies).is_pending());
                let (_, (newer, older)) =
                    iced::futures::future::join(serve(conn, rx, Timeouts::default()), replies).await;
                assert!(newer.unwrap(), "latest intent was discarded (listing={listing})");
                assert!(older.unwrap().is_none(), "obsolete intent survived (listing={listing})");
                assert_eq!(
                    requests.try_iter().collect::<Vec<_>>(),
                    [("latest".into(), if listing { "list" } else { "report" }.into())]
                );
            });
        }
    }

    #[test]
    fn queued_searches_and_lists_coalesce_without_crossing_open_requests() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (observed, requests) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(observed))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim()).build().await.unwrap();
            let (tx, rx) = mpsc::unbounded();
            let (blocked, blocked_reply) = oneshot::channel();
            tx.unbounded_send(Request::Run { id: "blocked".into(), action: "stall".into(), reply: blocked }).unwrap();
            let (old_list, old_list_reply) = oneshot::channel();
            tx.unbounded_send(Request::ListDirectory { folder: "obsolete-list".into(), reply: old_list }).unwrap();
            let (old_search, old_search_reply) = oneshot::channel();
            tx.unbounded_send(Request::Search {
                query: "report".into(),
                folder: Some("obsolete-search".into()),
                reply: old_search,
            })
            .unwrap();
            let (first, first_reply) = oneshot::channel();
            tx.unbounded_send(Request::ListDirectory { folder: "before-open".into(), reply: first }).unwrap();
            let (open, open_reply) = oneshot::channel();
            tx.unbounded_send(Request::Run { id: "open-barrier".into(), action: String::new(), reply: open }).unwrap();
            let (obsolete, obsolete_reply) = oneshot::channel();
            tx.unbounded_send(Request::ListDirectory { folder: "obsolete-after".into(), reply: obsolete }).unwrap();
            let (latest, latest_reply) = oneshot::channel();
            tx.unbounded_send(Request::Search {
                query: "report".into(),
                folder: Some("after-open".into()),
                reply: latest,
            })
            .unwrap();
            drop(tx);
            let verify = async {
                assert!(blocked_reply.await.unwrap().is_err());
                assert!(old_list_reply.await.is_err() && old_search_reply.await.is_err());
                assert!(first_reply.await.unwrap().is_ok());
                assert!(open_reply.await.unwrap().is_ok());
                assert!(obsolete_reply.await.is_err());
                assert!(latest_reply.await.unwrap().is_ok());
                assert_eq!(
                    requests.try_iter().collect::<Vec<_>>(),
                    [
                        ("blocked".into(), "stall".into()),
                        ("before-open".into(), "list".into()),
                        ("open-barrier".into(), String::new()),
                        ("after-open".into(), "report".into())
                    ]
                );
            };
            iced::futures::future::join(
                serve(conn, rx, Timeouts { open: Duration::from_millis(40), ..Timeouts::default() }),
                verify,
            )
            .await;
        });
    }

    #[test]
    fn directory_deadlines_release_the_queue_and_preserve_truncation() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (observed, requests) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(observed))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim()).build().await.unwrap();
            let (tx, rx) = mpsc::unbounded();
            let backend = Backend { tx };
            let verify = async {
                assert_eq!(
                    backend.clone().list_directory("stall".into()).await.unwrap_err(),
                    "Folder listing timed out. Refresh to try again."
                );
                assert_eq!(
                    backend.clone().list_directory("reject".into()).await.unwrap_err(),
                    "Folder changed or is unavailable. Refresh or go back."
                );
                assert_eq!(
                    backend.clone().list_directory("old-daemon".into()).await.unwrap_err(),
                    "Folder navigation requires an updated daemon."
                );
                let (files, more) = backend.clone().list_directory("ready".into()).await.unwrap();
                assert!(more);
                assert_eq!(files[0].path.as_deref(), Some(std::path::Path::new("/synthetic/report.txt")));
                assert_eq!(
                    requests.try_iter().map(|(id, _)| id).collect::<Vec<_>>(),
                    ["stall", "reject", "old-daemon", "ready"]
                );
                assert!(backend.search("report".into()).await.unwrap().is_some());
            };
            iced::futures::future::join(
                serve(conn, rx, Timeouts { search: Duration::from_millis(40), ..Timeouts::default() }),
                verify,
            )
            .await;
        });
    }

    #[test]
    fn preview_queue_keeps_only_the_latest_target_across_pending_searches() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (observed, requests) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(observed))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim()).build().await.unwrap();
            let (tx, rx) = mpsc::unbounded();
            let (old, old_reply) = oneshot::channel();
            tx.unbounded_send(Request::TextPreview {
                id: "old".into(),
                validity: PreviewRequest::default(),
                reply: old,
            })
            .unwrap();
            let (search, search_reply) = oneshot::channel();
            tx.unbounded_send(Request::Search { query: "report".into(), folder: None, reply: search }).unwrap();
            let (latest, latest_reply) = oneshot::channel();
            tx.unbounded_send(Request::TextPreview {
                id: "latest".into(),
                validity: PreviewRequest::default(),
                reply: latest,
            })
            .unwrap();
            drop(tx);
            let verify = async {
                assert!(old_reply.await.is_err(), "superseded targets must never reach the service");
                assert!(search_reply.await.unwrap().is_ok());
                let data = latest_reply.await.unwrap().unwrap();
                assert!(data.content.is_ok() && data.details.is_ok());
                let received: Vec<_> = requests.try_iter().collect();
                assert_eq!(received.len(), 2);
                assert!(received.iter().all(|(id, _)| id == "latest"));
            };
            iced::futures::future::join(serve(conn, rx, Timeouts::default()), verify).await;
        });
    }

    #[test]
    fn text_preview_timeout_releases_the_bus_queue_without_retrying() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (tx, requests) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(tx))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim())
                .method_timeout(Duration::from_secs(2))
                .build()
                .await
                .unwrap();
            let (tx, rx) = mpsc::unbounded();
            let backend = Backend { tx };
            let verify = async {
                let stalled = backend.clone().text_preview("stall".into(), PreviewRequest::default()).await.unwrap();
                assert_eq!(stalled.content.unwrap_err(), "Text preview timed out. Refresh to try again.");
                assert!(stalled.details.is_err());
                let rejected = backend.clone().text_preview("reject".into(), PreviewRequest::default()).await.unwrap();
                assert!(rejected.content.is_err() && rejected.details.is_err());
                let ready = backend.clone().text_preview("ready".into(), PreviewRequest::default()).await.unwrap();
                assert_eq!(
                    ready.content.unwrap(),
                    TextPreview::Text { text: "synthetic text".into(), truncated: true }
                );
                assert_eq!(
                    ready.details.unwrap(),
                    FileDetails { mime: "application/pdf".into(), size: 123, modified: -1 }
                );
                assert_eq!(
                    backend
                        .clone()
                        .text_preview("binary".into(), PreviewRequest::default())
                        .await
                        .unwrap()
                        .content
                        .unwrap(),
                    TextPreview::Unsupported("not UTF-8 text".into())
                );
                let separate =
                    backend.clone().text_preview("metadata-error".into(), PreviewRequest::default()).await.unwrap();
                assert!(
                    separate.content.is_ok() && separate.details.is_err(),
                    "metadata failure does not discard text"
                );
                let received: Vec<_> = requests.try_iter().collect();
                for id in ["stall", "reject", "ready", "binary", "metadata-error"] {
                    assert_eq!(
                        received.iter().filter(|(target, _)| target == id).count(),
                        2,
                        "exactly one content and one metadata call per target"
                    );
                }
                assert_eq!(received.len(), 10, "reads must not retry automatically");
                assert!(backend.search("report".into()).await.unwrap().is_some());
            };
            iced::futures::future::join(
                serve(conn, rx, Timeouts { status: Duration::from_millis(40), ..Timeouts::default() }),
                verify,
            )
            .await;
        });
    }

    #[test]
    fn folder_queries_capture_scope_and_text_and_never_fall_back_to_global_search() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let (tx, requests) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .name(DAEMON_BUS_NAME)
                .serve_at(SEARCH_PATH, SearchService(tx))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim())
                .method_timeout(Duration::from_secs(2))
                .build()
                .await
                .unwrap();
            let (tx, rx) = mpsc::unbounded();
            let backend = Backend { tx };
            let verify = async {
                let folder = "file:///synthetic/Reports%20%C3%A8";
                assert!(backend.clone().search_within(folder.into(), "a/b report".into()).await.unwrap().is_some());
                assert_eq!(requests.try_recv().unwrap(), (folder.into(), "a/b report".into()));
                assert!(backend.clone().search_within("reject".into(), "report".into()).await.is_err());
                assert_eq!(requests.try_recv().unwrap().0, "reject");
                assert_eq!(
                    backend.clone().search_within("stall".into(), "report".into()).await.unwrap_err(),
                    SEARCH_TIMED_OUT
                );
                assert_eq!(requests.try_recv().unwrap().0, "stall");
                assert!(requests.try_recv().is_err(), "no implicit retries");
                assert!(backend.search("report".into()).await.unwrap().is_some());
            };
            iced::futures::future::join(
                serve(conn, rx, Timeouts { search: Duration::from_millis(40), ..Timeouts::default() }),
                verify,
            )
            .await;
        });
    }

    #[test]
    fn viewing_folder_settings_does_not_activate_an_absent_service() {
        let (_temp, _bus, address) = private_bus();
        zbus::block_on(async {
            let conn = zbus::connection::Builder::address(address.trim()).build().await.unwrap();
            assert!(folder_policy(&conn).await.unwrap().is_none());
            assert!(folder_policy(&conn).await.unwrap().is_none());
        });
    }

    #[test]
    fn no_reply_is_not_misreported_as_a_rejected_open() {
        assert!(matches!(
            open_error(zbus::fdo::Error::NoReply("connection lost".into()).into()),
            RunError::Unconfirmed(_)
        ));
        assert!(matches!(open_error(zbus::fdo::Error::InvalidArgs("bad URL".into()).into()), RunError::Failed(_)));
        assert!(matches!(
            open_error(zbus::fdo::Error::SpawnExecFailed("executable is unavailable".into()).into()),
            RunError::Failed(_)
        ));
        assert!(matches!(
            open_error(zbus::fdo::Error::Failed("opener may have started".into()).into()),
            RunError::Unconfirmed(_)
        ));
    }

    #[test]
    fn remote_match_conversion() {
        let h = to_hit(("file:///home/u/Docs/a%20b".into(), "a b".into(), "folder".into(), "~/Docs".into(), true));
        assert_eq!(h.path.as_deref(), Some(std::path::Path::new("/home/u/Docs/a b")));
        assert_eq!(h.subtext, "~/Docs");
        assert!(h.is_dir);
    }

    #[test]
    fn status_conversion_preserves_counts_flags_and_root_boundaries() {
        let status = to_status(("paused".into(), 23, 5, 2, true, 7, vec!["/a:b".into(), "/c".into()]));
        assert_eq!(
            status,
            SearchStatus {
                state: "paused".into(),
                files: 23,
                dirs: 5,
                queued_dirs: 2,
                watch_limit_hit: true,
                errors: 7,
                roots: vec!["/a:b".into(), "/c".into()],
            }
        );
    }

    #[test]
    fn status_returns_daemon_and_disconnection_errors() {
        zbus::block_on(async {
            assert_eq!(Backend::disconnected().status().await.unwrap_err(), "bus thread stopped");
            let (tx, mut requests) = mpsc::unbounded();
            let backend = Backend { tx };
            let respond = async move {
                let Some(Request::Status { reply }) = requests.next().await else { panic!("expected status request") };
                let _ = reply.send(Err("File-search status is unavailable. Try again.".into()));
            };
            let (result, ()) = iced::futures::future::join(backend.status(), respond).await;
            assert_eq!(result.unwrap_err(), "File-search status is unavailable. Try again.");
        });
    }
}

#[cfg(test)]
mod preview_diagnostics {
    #[test]
    fn debug_does_not_expose_file_contents() {
        let preview = super::TextPreview::Text { text: "PRIVATE_FILE_CONTENTS".into(), truncated: false };
        assert!(!format!("{preview:?}").contains("PRIVATE_FILE_CONTENTS"));
    }
}
