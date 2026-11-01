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

use iced::futures::StreamExt;
use iced::futures::channel::{mpsc, oneshot};
use zbus::fdo::{RequestNameFlags, RequestNameReply};

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

/// Events the bus thread delivers to the UI.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// Another invocation asked this instance to toggle (close).
    Toggle,
}

enum Request {
    Search { query: String, reply: oneshot::Sender<Result<Vec<FileHit>, String>> },
    Run { id: String, action: String, reply: oneshot::Sender<Result<(), String>> },
    Status { reply: oneshot::Sender<Result<SearchStatus, String>> },
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
                serve(conn, rx).await;
            });
        })
        .map_err(|e| format!("cannot start bus thread: {e}"))?;
    ready_rx.recv().map_err(|_| "bus thread exited during startup".to_string())?
}

async fn serve(conn: zbus::Connection, mut rx: mpsc::UnboundedReceiver<Request>) {
    while let Some(mut req) = rx.next().await {
        // Keystrokes queue searches faster than they may complete; only the
        // newest pending one is worth running. Dropping a superseded reply
        // sender cancels its waiting UI task.
        loop {
            match rx.try_recv() {
                Ok(next @ Request::Search { .. }) if matches!(req, Request::Search { .. }) => req = next,
                Ok(other) => {
                    handle(&conn, req).await;
                    req = other;
                }
                _ => break,
            }
        }
        handle(&conn, req).await;
    }
}

async fn handle(conn: &zbus::Connection, req: Request) {
    match req {
        Request::Search { query, reply } => {
            let r = search(conn, &query).await;
            let _ = reply.send(r);
        }
        Request::Run { id, action, reply } => {
            let r = conn
                .call_method(
                    Some(DAEMON_BUS_NAME),
                    SEARCH_PATH,
                    Some(SEARCH_IFACE),
                    "Open",
                    &(id.as_str(), action.as_str(), ""),
                )
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = reply.send(r);
        }
        Request::Status { reply } => {
            let r = file_search_status(conn).await;
            let _ = reply.send(r);
        }
    }
}

type FileResult = (String, String, String, String, bool);
type StatusResult = (String, u64, u64, u64, bool, u64, Vec<String>);

async fn file_search_status(conn: &zbus::Connection) -> Result<SearchStatus, String> {
    // Diagnostics must release the serial dispatcher even if the service
    // never replies. Keep search/open calls on their existing timeout policy.
    let runtime = zruntime::SharedRuntime::current().map_err(|e| e.to_string())?;
    let request = conn.call_method(Some(DAEMON_BUS_NAME), CONTROL_PATH, Some(CONTROL_IFACE), "FileSearchStatus", &());
    let msg = runtime
        .timeout(STATUS_TIMEOUT, request)
        .await
        .map_err(|_| STATUS_TIMED_OUT.to_string())?
        .map_err(|e| e.to_string())?;
    let status: StatusResult = msg.body().deserialize().map_err(|e| e.to_string())?;
    Ok(to_status(status))
}

fn to_status((state, files, dirs, queued_dirs, watch_limit_hit, errors, roots): StatusResult) -> SearchStatus {
    SearchStatus { state, files, dirs, queued_dirs, watch_limit_hit, errors, roots }
}

async fn search(conn: &zbus::Connection, query: &str) -> Result<Vec<FileHit>, String> {
    // Calling the daemon's name D-Bus-activates it when it is not running.
    let msg = conn
        .call_method(Some(DAEMON_BUS_NAME), SEARCH_PATH, Some(SEARCH_IFACE), "Search", &(query,))
        .await
        .map_err(|e| e.to_string())?;
    let matches: Vec<FileResult> = msg.body().deserialize().map_err(|e| e.to_string())?;
    Ok(matches.into_iter().map(to_hit).collect())
}

fn to_hit((id, name, icon, subtext, is_dir): FileResult) -> FileHit {
    let path = crate::uri::path_from_file_url(&id);
    FileHit { subtext, path, name, icon, is_dir, id }
}

impl Backend {
    #[cfg(test)]
    pub fn disconnected() -> Self {
        let (tx, _rx) = mpsc::unbounded();
        Self { tx }
    }

    /// File results for `query`; `Err` if the daemon is unreachable. A
    /// superseded search resolves to `Ok(None)`.
    pub async fn search(self, query: String) -> Result<Option<Vec<FileHit>>, String> {
        let (reply, rx) = oneshot::channel();
        self.tx.unbounded_send(Request::Search { query, reply }).map_err(|_| "bus thread stopped".to_string())?;
        match rx.await {
            Ok(r) => r.map(Some),
            Err(oneshot::Canceled) => Ok(None),
        }
    }

    /// Asks the daemon to open a match (`action` `""`) or reveal it
    /// (`ACTION_OPEN_PARENT`). The daemon only opens indexed paths.
    pub async fn run(self, id: String, action: &'static str) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .unbounded_send(Request::Run { id, action: action.to_string(), reply })
            .map_err(|_| "bus thread stopped".to_string())?;
        rx.await.map_err(|_| "bus thread stopped".to_string())?
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
        async fn search(&self, query: &str) -> Vec<FileResult> {
            assert_eq!(query, "report");
            vec![("file:///synthetic/report.txt".into(), "report.txt".into(), "".into(), "/synthetic".into(), false)]
        }

        async fn open(&self, id: &str, action: &str, _token: &str) {
            self.0.send((id.into(), action.into())).unwrap();
        }
    }

    struct TestBus(std::process::Child);

    impl Drop for TestBus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn stalled_status_releases_queued_search_and_open_requests() {
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

        zbus::block_on(async {
            let (opened_tx, opened_rx) = std::sync::mpsc::channel();
            let _service = zbus::connection::Builder::address(address.trim())
                .unwrap()
                .name(DAEMON_BUS_NAME)
                .serve_at(CONTROL_PATH, StalledStatus)
                .serve_at(SEARCH_PATH, SearchService(opened_tx))
                .build()
                .await
                .unwrap();
            let conn = zbus::connection::Builder::address(address.trim())
                .unwrap()
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
            tx.unbounded_send(Request::Search { query: "report".into(), reply: search_tx }).unwrap();
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
            iced::futures::future::join(serve(conn, rx), verify).await;
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
