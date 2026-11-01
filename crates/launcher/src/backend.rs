// SPDX-License-Identifier: GPL-3.0-or-later
//! Session-bus side of the launcher, on its own thread.
//!
//! zbus's default runtime drives a connection only from inside
//! `zbus::block_on`, so one thread sits in it for the life of the process:
//! it serves the single-instance interface and forwards file searches and
//! opens to the daemon's `io.github.ferroseek.Search1` object. The UI talks to it
//! through channels, never by awaiting zbus futures on its own executor.

use std::path::PathBuf;

use iced::futures::StreamExt;
use iced::futures::channel::{mpsc, oneshot};
use zbus::fdo::{RequestNameFlags, RequestNameReply};

/// The daemon (mirrors `ferroseek_engine::ipc`; checked by a test).
pub const DAEMON_BUS_NAME: &str = "io.github.ferroseek.FerroSeek";
pub const SEARCH_PATH: &str = "/io/github/ferroseek/Search";
const SEARCH_IFACE: &str = "io.github.ferroseek.Search1";
/// The file search service's reveal action.
pub const ACTION_OPEN_PARENT: &str = "open-parent";

pub const LAUNCHER_BUS_NAME: &str = "io.github.ferroseek.Launcher";
pub const LAUNCHER_PATH: &str = "/io/github/ferroseek/Launcher";

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

/// Events the bus thread delivers to the UI.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// Another invocation asked this instance to toggle (close).
    Toggle,
}

enum Request {
    Search { query: String, reply: oneshot::Sender<Result<Vec<FileHit>, String>> },
    Run { id: String, action: String, reply: oneshot::Sender<Result<(), String>> },
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
    }
}

type FileResult = (String, String, String, String, bool);

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_names_match_the_engine() {
        assert_eq!(DAEMON_BUS_NAME, ferroseek_engine::ipc::BUS_NAME);
        assert_eq!(SEARCH_PATH, ferroseek_engine::ipc::SEARCH_PATH);
        assert_eq!(SEARCH_IFACE, ferroseek_engine::ipc::SEARCH_IFACE);
    }

    #[test]
    fn remote_match_conversion() {
        let h = to_hit(("file:///home/u/Docs/a%20b".into(), "a b".into(), "folder".into(), "~/Docs".into(), true));
        assert_eq!(h.path.as_deref(), Some(std::path::Path::new("/home/u/Docs/a b")));
        assert_eq!(h.subtext, "~/Docs");
        assert!(h.is_dir);
    }
}
