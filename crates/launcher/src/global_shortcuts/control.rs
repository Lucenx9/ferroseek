// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings access to the separately owned shortcut listener.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use iced::futures::channel::oneshot;
use zbus::fdo::{self, DBusProxy};
use zbus::names::OwnedUniqueName;
use zbus::object_server::ResponseDispatchNotifier;
use zbus::{Connection, OwnedObjectPath, Proxy, Variant};

use super::{CALL_TIMEOUT, Options, ROOT, SERVICE, SERVICE_UNIT, SHORTCUTS};

pub(super) const PATH: &str = "/io/github/ferroseek/LauncherShortcuts";
const INTERFACE: &str = "io.github.ferroseek.LauncherShortcuts1";
const SERVICE_WAIT: Duration = Duration::from_secs(4);
type Bindings = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Stopped,
    Starting,
    Ready { version: u32, bindings: Bindings },
}

pub(crate) enum Operation {
    Read,
    Start,
    Configure(String),
    Stop,
}

#[derive(Default, Clone)]
pub(super) struct SessionState(Arc<Mutex<Option<Session>>>);

pub(super) struct Session {
    pub portal: OwnedUniqueName,
    pub handle: OwnedObjectPath,
    pub version: u32,
    pub bindings: Bindings,
}

impl SessionState {
    fn lock(&self) -> MutexGuard<'_, Option<Session>> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn ready(&self, session: Session) {
        *self.lock() = Some(session);
    }

    pub(super) fn update_bindings(&self, bindings: Bindings) {
        if let Some(session) = self.lock().as_mut() {
            session.bindings = bindings;
        }
    }

    fn configurable(&self) -> fdo::Result<(OwnedUniqueName, OwnedObjectPath)> {
        let state = self.lock();
        let session = state.as_ref().ok_or_else(|| fdo::Error::Failed("Shortcuts are still starting.".into()))?;
        if session.version < 2 {
            return Err(fdo::Error::NotSupported("Change shortcuts in your desktop settings.".into()));
        }
        Ok((session.portal.clone(), session.handle.clone()))
    }
}

pub(super) struct Listener {
    state: SessionState,
    stop: Mutex<Option<oneshot::Sender<()>>>,
}

impl Listener {
    pub(super) fn new(state: SessionState, stop: oneshot::Sender<()>) -> Self {
        Self { state, stop: Mutex::new(Some(stop)) }
    }
}

#[zbus::interface(name = "io.github.ferroseek.LauncherShortcuts1")]
impl Listener {
    fn status(&self) -> (u32, Bindings) {
        self.state
            .lock()
            .as_ref()
            .map_or_else(|| (0, Vec::new()), |session| (session.version, session.bindings.clone()))
    }

    async fn configure(&self, token: &str, #[zbus(connection)] conn: &Connection) -> fdo::Result<()> {
        let (portal, session) = self.state.configurable()?;
        let proxy = Proxy::new(conn, portal.as_str(), ROOT, SHORTCUTS).await?;
        let mut options = Options::new();
        if !token.is_empty() {
            options.insert("activation_token", Variant::from(token));
        }
        // The layer-shell surface cannot be exported as a dialog parent.
        Ok(proxy.call("ConfigureShortcuts", &(session, "", options)).await?)
    }

    fn stop(&self) -> ResponseDispatchNotifier<()> {
        let (response, dispatched) = ResponseDispatchNotifier::new(());
        if let Some(stop) = self.stop.lock().unwrap_or_else(|error| error.into_inner()).take() {
            // Keep the connection alive until the reply has been sent.
            std::thread::spawn(move || {
                zbus::block_on(dispatched);
                let _ = stop.send(());
            });
        }
        response
    }
}

pub(crate) async fn perform(operation: Operation) -> Result<Status, String> {
    zruntime::unblock(move || zbus::block_on(request(operation))).await
}

async fn request(operation: Operation) -> Result<Status, String> {
    let conn = zbus::connection::Builder::session()
        .method_timeout(CALL_TIMEOUT)
        .build()
        .await
        .map_err(|_| "Could not connect to the desktop. Refresh to try again.")?;
    let proxy = listener(&conn).await.map_err(read_error)?;
    match operation {
        Operation::Read => read(proxy).await.map_err(read_error),
        Operation::Start => match proxy {
            Some(proxy) => read(Some(proxy)).await.map_err(read_error),
            None => {
                start(&conn).await?;
                await_status(&conn, true).await
            }
        },
        Operation::Configure(token) => {
            let proxy = proxy.ok_or("Global shortcuts have stopped. Refresh to check their status.")?;
            proxy
                .call::<_, _, ()>("Configure", &(token,))
                .await
                .map_err(|_| "Could not open shortcut configuration. Try again or use your desktop settings.")?;
            read(Some(proxy)).await.map_err(read_error)
        }
        Operation::Stop => {
            if let Some(proxy) = proxy {
                proxy
                    .call::<_, _, ()>("Stop", &())
                    .await
                    .map_err(|_| "Could not stop global shortcuts. Refresh to check their status.")?;
                await_status(&conn, false).await
            } else {
                Ok(Status::Stopped)
            }
        }
    }
}

fn read_error(error: zbus::Error) -> String {
    eprintln!("ferroseek-launcher: could not read the global shortcut listener status: {error}");
    "Could not read shortcut status. Refresh to try again.".into()
}

async fn listener(conn: &Connection) -> zbus::Result<Option<Proxy<'_>>> {
    let bus = DBusProxy::new(conn).await?;
    match bus.get_name_owner(SERVICE.try_into()?).await {
        Ok(owner) => Ok(Some(Proxy::new(conn, owner, PATH, INTERFACE).await?)),
        Err(fdo::Error::NameHasNoOwner(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn read(proxy: Option<Proxy<'_>>) -> zbus::Result<Status> {
    let Some(proxy) = proxy else { return Ok(Status::Stopped) };
    let (version, bindings) = proxy.call("Status", &()).await?;
    Ok(if version == 0 { Status::Starting } else { Status::Ready { version, bindings } })
}

async fn await_status(conn: &Connection, running: bool) -> Result<Status, String> {
    let deadline = Instant::now() + SERVICE_WAIT;
    while Instant::now() < deadline {
        let proxy = listener(conn).await.map_err(read_error)?;
        if proxy.is_some() == running {
            return read(proxy).await.map_err(read_error);
        }
        // Runs on an offloaded worker, so waiting does not block the UI.
        std::thread::sleep(Duration::from_millis(100));
    }
    let message = if running {
        "Global shortcuts could not start. Check desktop support and the launcher installation."
    } else {
        "Global shortcuts are still running. Refresh to check their status."
    };
    Err(message.into())
}

async fn start(conn: &Connection) -> Result<(), String> {
    let manager =
        Proxy::new(conn, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager")
            .await
            .map_err(|_| "Could not reach the desktop service manager.")?;
    match manager.call::<_, _, OwnedObjectPath>("StartUnit", &(SERVICE_UNIT, "fail")).await {
        Ok(_) => Ok(()),
        Err(error) if service_missing(&error) => start_direct(),
        Err(error) => {
            eprintln!("ferroseek-launcher: could not start the shortcut service: {error}");
            Err("Could not start the desktop shortcut service. Check the launcher installation.".into())
        }
    }
}

fn service_missing(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner"
                | "org.freedesktop.systemd1.NoSuchUnit"
        ),
        zbus::Error::FDO(error) => {
            matches!(error.as_ref(), fdo::Error::ServiceUnknown(_) | fdo::Error::NameHasNoOwner(_))
        }
        _ => false,
    }
}

fn start_direct() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| "Could not find the launcher executable.")?;
    super::spawn_launcher(&executable, &["--global-shortcuts"])
        .map_err(|_| "Could not start the global shortcut listener.".into())
}
