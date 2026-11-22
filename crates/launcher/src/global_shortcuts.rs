// SPDX-License-Identifier: GPL-3.0-or-later
//! Own the desktop shortcut session independently of the launcher window.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use iced::futures::future::select;
use iced::futures::{StreamExt, channel::oneshot};
use serde::Serialize;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::names::{OwnedUniqueName, WellKnownName};
use zbus::zvariant::DynamicType;
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Proxy, Variant};

use crate::commands::{self, Builtin};

mod control;
pub(crate) use control::{Operation, Status, perform};

const APP_ID: &str = "io.github.ferroseek.Launcher";
const SERVICE: &str = "io.github.ferroseek.LauncherShortcuts";
const SERVICE_UNIT: &str = "app-io.github.ferroseek.Launcher.service";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const ROOT: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const REGISTRY: &str = "org.freedesktop.host.portal.Registry";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const ASSIGNMENT_TIMEOUT: Duration = Duration::from_secs(180);
const OPEN: &str = "open-launcher";

type Properties = HashMap<String, OwnedVariant>;
type Options<'a> = HashMap<&'static str, Variant<'a>>;
type Shortcuts = Vec<(String, Properties)>;

pub fn run() -> Result<(), String> {
    zbus::block_on(run_monitor())
}

async fn run_monitor() -> Result<(), String> {
    let (stop, stopped) = oneshot::channel();
    let ready = control::SessionState::default();
    let conn = zbus::connection::Builder::session()
        .method_timeout(CALL_TIMEOUT)
        .serve_at(control::PATH, control::Listener::new(ready.clone(), stop))
        .build()
        .await
        .map_err(|error| format!("Could not connect to the session bus: {error}"))?;
    let claim = conn
        .request_name_with_flags(SERVICE, RequestNameFlags::DO_NOT_QUEUE)
        .await
        .map_err(|error| format!("Could not claim the shortcut listener name: {error}"))?;
    if !matches!(claim, RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) {
        return Err("A FerroSeek shortcut listener is already running.".into());
    }

    let monitor = async {
        let owner = portal_owner(&conn).await.map_err(|error| format!("The desktop portal is unavailable: {error}"))?;
        register(&conn, &owner).await?;
        let proxy = Proxy::new(&conn, owner.as_str(), ROOT, SHORTCUTS).await.map_err(|error| error.to_string())?;
        let portal = Portal(proxy);
        let session = portal.create_session().await?;
        portal.listen(&session, &owner, ready).await
    };
    until(monitor, stopped).await
}

async fn until(work: impl Future<Output = Result<(), String>>, stop: impl Future) -> Result<(), String> {
    match select(Box::pin(work), Box::pin(stop)).await {
        iced::futures::future::Either::Left((result, _)) => result,
        iced::futures::future::Either::Right(_) => Ok(()),
    }
}

async fn portal_owner(conn: &Connection) -> zbus::fdo::Result<OwnedUniqueName> {
    let dbus = DBusProxy::new(conn).await?;
    let name = WellKnownName::try_from(PORTAL).expect("valid portal name");
    match dbus.get_name_owner(name.clone().into()).await {
        Err(zbus::fdo::Error::NameHasNoOwner(_)) => {
            dbus.start_service_by_name(name.clone(), 0).await?;
            dbus.get_name_owner(name.into()).await
        }
        owner => owner,
    }
}

async fn register(conn: &Connection, owner: &OwnedUniqueName) -> Result<(), String> {
    let registry = Proxy::new(conn, owner.as_str(), ROOT, REGISTRY).await.map_err(|error| error.to_string())?;
    let registered = registry.call::<_, _, ()>("Register", &(APP_ID, Options::new())).await;
    match registered {
        Ok(()) => Ok(()),
        Err(error) if registry_missing(&error) => {
            let cgroups = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
            if identified_service(&cgroups) {
                Ok(())
            } else {
                Err("This desktop portal needs FerroSeek to run in its user service. Start that service or configure a shortcut in desktop settings.".into())
            }
        }
        Err(error) => {
            Err(format!("Could not identify FerroSeek to the desktop portal. Check the launcher installation. {error}"))
        }
    }
}

fn registry_missing(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.UnknownInterface" | "org.freedesktop.DBus.Error.UnknownMethod"
        ),
        zbus::Error::FDO(error) => {
            matches!(error.as_ref(), zbus::fdo::Error::UnknownInterface(_) | zbus::fdo::Error::UnknownMethod(_))
        }
        _ => false,
    }
}

fn identified_service(cgroups: &str) -> bool {
    cgroups
        .lines()
        .any(|entry| entry.splitn(3, ':').nth(2).and_then(|path| path.rsplit('/').next()) == Some(SERVICE_UNIT))
}

struct Portal<'a>(Proxy<'a>);

impl Portal<'_> {
    async fn proxy(&self, path: &str, interface: &'static str) -> zbus::Result<Proxy<'_>> {
        Proxy::new(self.0.connection(), self.0.destination().clone(), path.to_owned(), interface).await
    }

    async fn create_session(&self) -> Result<OwnedObjectPath, String> {
        let request = RequestToken::new()?;
        let mut options = request.options();
        let session_token = RequestToken::new()?;
        options.insert("session_handle_token", Variant::from(session_token.0.as_str()));
        let reply = self.request("CreateSession", &request, &(options,)).await?;
        let session = reply
            .get("session_handle")
            .and_then(|value| <&str>::try_from(value).ok())
            .ok_or("The desktop returned no shortcut session.")?;
        let expected = self.path("session", &session_token)?;
        if session != expected {
            return Err("The desktop returned an unexpected shortcut session.".into());
        }
        OwnedObjectPath::try_from(session).map_err(|error| error.to_string())
    }

    async fn request<B>(&self, method: &str, token: &RequestToken, body: &B) -> Result<Properties, String>
    where
        B: Serialize + DynamicType,
    {
        let path = self.path("request", token)?;
        let request = self.proxy(&path, REQUEST).await.map_err(|error| error.to_string())?;
        // A portal can send Response before the method reply arrives.
        let mut responses = request.receive_signal("Response").await.map_err(|error| error.to_string())?;
        let actual: OwnedObjectPath =
            self.0.call(method, body).await.map_err(|error| format!("Global shortcuts are unavailable. {error}"))?;
        if actual.as_str() != path {
            return Err("The desktop returned an unexpected shortcut request.".into());
        }
        await_response(method, &mut responses).await
    }

    fn path(&self, kind: &str, token: &RequestToken) -> Result<String, String> {
        let name = self.0.connection().unique_name().ok_or("The session bus did not identify FerroSeek.")?;
        let sender = name.as_str().trim_start_matches(':').replace('.', "_");
        Ok(format!("{ROOT}/{kind}/{sender}/{}", token.0))
    }

    async fn listen(
        &self,
        session: &OwnedObjectPath,
        portal_owner: &OwnedUniqueName,
        state: control::SessionState,
    ) -> Result<(), String> {
        let session_proxy = self.proxy(session.as_str(), SESSION).await.map_err(|error| error.to_string())?;
        let mut closed = session_proxy.receive_signal("Closed").await.map_err(|error| error.to_string())?;
        let mut owner = self.0.receive_owner_changed().await.map_err(|error| error.to_string())?;
        let events = self.0.receive_signal("Activated").await.map_err(|error| error.to_string())?;
        let changes = self.0.receive_signal("ShortcutsChanged").await.map_err(|error| error.to_string())?;
        let monitor = async {
            let version = self.0.get_property::<u32>("version").await.unwrap_or(1).max(1);
            let shortcuts = self.bind_shortcuts(session).await?;
            let labels = binding_labels(shortcuts);
            if labels.is_empty() {
                println!("No shortcut labels were reported. Choose FerroSeek shortcuts in desktop settings.");
            }
            state.ready(control::Session {
                portal: portal_owner.clone(),
                handle: session.clone(),
                version,
                bindings: labels,
            });
            println!("Listening for FerroSeek global shortcuts.");
            until(forward_activations(session, events), self.watch_bindings(session, changes, state)).await
        };
        until(monitor, select(closed.next(), owner.next())).await
    }

    async fn watch_bindings(
        &self,
        session: &OwnedObjectPath,
        mut changes: zbus::proxy::SignalStream<'_>,
        state: control::SessionState,
    ) {
        while let Some(signal) = changes.next().await {
            let Ok((path, _)) = signal.body().deserialize::<(OwnedObjectPath, Shortcuts)>() else { continue };
            if &path == session {
                match self.list_shortcuts(session).await {
                    Ok(shortcuts) => state.update_bindings(binding_labels(shortcuts)),
                    Err(error) => eprintln!("ferroseek-launcher: could not refresh shortcut assignments: {error}"),
                }
            }
        }
    }

    async fn list_shortcuts(&self, session: &OwnedObjectPath) -> Result<Shortcuts, String> {
        let list = RequestToken::new()?;
        let properties = self.request("ListShortcuts", &list, &(session, list.options())).await?;
        decode_shortcuts(properties)
    }

    async fn bind_shortcuts(&self, session: &OwnedObjectPath) -> Result<Shortcuts, String> {
        // ListShortcuts can report prior assignments before this session binds its actions.
        let bind = RequestToken::new()?;
        let properties =
            self.request("BindShortcuts", &bind, &(session, shortcut_definitions(), "", bind.options())).await?;
        decode_shortcuts(properties)
    }
}

struct RequestToken(String);

impl RequestToken {
    fn new() -> Result<Self, String> {
        let mut bytes = [0; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut bytes))
            .map_err(|error| format!("Could not create a shortcut request ID: {error}"))?;
        Ok(Self(format!("ferroseek_{:032x}", u128::from_ne_bytes(bytes))))
    }

    fn options(&self) -> Options<'_> {
        HashMap::from([("handle_token", Variant::from(self.0.as_str()))])
    }
}

async fn await_response(method: &str, signals: &mut zbus::proxy::SignalStream<'_>) -> Result<Properties, String> {
    let runtime = zruntime::SharedRuntime::current().map_err(|error| error.to_string())?;
    let timeout = if method == "BindShortcuts" { ASSIGNMENT_TIMEOUT } else { CALL_TIMEOUT };
    let response = runtime
        .timeout(timeout, signals.next())
        .await
        .map_err(|_| "The desktop did not finish setting up shortcuts. Try again.")?
        .ok_or("The desktop closed the shortcut request.")?;
    match response.body().deserialize::<(u32, Properties)>().map_err(|error| error.to_string())? {
        (0, properties) => Ok(properties),
        (1, _) => Err("Global shortcut setup was canceled.".into()),
        _ => Err("The desktop declined global shortcut setup.".into()),
    }
}

fn shortcut_definitions() -> Vec<(&'static str, Options<'static>)> {
    std::iter::once((OPEN, "Open FerroSeek"))
        .chain(commands::ALL.into_iter().map(|command| (command.id(), command.title())))
        .map(|(id, title)| (id, HashMap::from([("description", Variant::from(title))])))
        .collect()
}

fn decode_shortcuts(mut properties: Properties) -> Result<Shortcuts, String> {
    properties
        .remove("shortcuts")
        .ok_or("The desktop returned no shortcut list.")?
        .try_into()
        .map_err(|_| "The desktop returned an invalid shortcut list.".into())
}

fn binding_labels(shortcuts: Shortcuts) -> Vec<(String, String)> {
    let mut labels: Vec<_> = shortcuts
        .into_iter()
        .filter_map(|(id, properties)| {
            let title = match Target::from_id(&id)? {
                Target::Open => "Open FerroSeek",
                Target::Command(command) => command.title(),
            };
            let trigger = <&str>::try_from(properties.get("trigger_description")?).ok()?.trim();
            (!trigger.is_empty()).then(|| (title.to_owned(), trigger.to_owned()))
        })
        .collect();
    labels.sort_unstable();
    labels
}

enum Target {
    Open,
    Command(Builtin),
}

impl Target {
    fn from_id(id: &str) -> Option<Self> {
        if id == OPEN { Some(Self::Open) } else { Builtin::from_id(id).map(Self::Command) }
    }

    fn launch(self, executable: &Path) -> std::io::Result<()> {
        match self {
            Self::Open => spawn_launcher(executable, &[]),
            Self::Command(command) => spawn_launcher(executable, &["--command", command.id()]),
        }
    }
}

fn spawn_launcher(executable: &Path, args: &[&str]) -> std::io::Result<()> {
    let mut child = Command::new(executable)
        .args(args)
        .process_group(0)
        .stdin(Stdio::null())
        .env_remove("XDG_ACTIVATION_TOKEN")
        .env_remove("DESKTOP_STARTUP_ID")
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

async fn forward_activations(
    session: &OwnedObjectPath,
    mut events: zbus::proxy::SignalStream<'_>,
) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    while let Some(signal) = events.next().await {
        let Ok((path, id, _, _)) = signal.body().deserialize::<(OwnedObjectPath, String, u64, Properties)>() else {
            continue;
        };
        if &path != session {
            continue;
        }
        let Some(target) = Target::from_id(&id) else { continue };
        if let Err(error) = target.launch(&executable) {
            eprintln!("ferroseek-launcher: could not open from a global shortcut: {error}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_portals_only_use_the_known_application_service_identity() {
        let prefix = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/";
        assert!(identified_service(&format!("{prefix}{SERVICE_UNIT}\n")));
        assert!(!identified_service(&format!("{prefix}another-app.service\n")));
        assert!(!identified_service(&format!("{prefix}{SERVICE_UNIT}.backup\n")));
    }

    #[test]
    fn requested_ids_resolve_to_launcher_targets() {
        assert!(matches!(Target::from_id(OPEN), Some(Target::Open)));
        for command in commands::ALL {
            assert!(matches!(Target::from_id(command.id()), Some(Target::Command(id)) if id == command));
        }
        assert!(Target::from_id("--private").is_none());
    }

    #[test]
    fn status_uses_desktop_labels_only_for_known_shortcuts() {
        let trigger = |label: &'static str| {
            HashMap::from([("trigger_description".into(), Variant::from(label).try_into().unwrap())])
        };
        assert_eq!(
            binding_labels(vec![
                (Builtin::SwitchWindows.id().into(), trigger("")),
                (OPEN.into(), trigger("Ctrl+Alt+Space")),
                ("unknown".into(), trigger("Ctrl+Alt+T")),
                (Builtin::ClipboardHistory.id().into(), Properties::new()),
            ]),
            [("Open FerroSeek".into(), "Ctrl+Alt+Space".into())]
        );
    }
}
