// SPDX-License-Identifier: GPL-3.0-or-later
//! Own the desktop shortcut session independently of the launcher window.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use iced::futures::StreamExt;
use iced::futures::future::select;
use serde::Serialize;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::names::{OwnedUniqueName, WellKnownName};
use zbus::zvariant::DynamicType;
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Proxy, Variant};

use crate::commands::{self, Builtin};

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
    let conn = zbus::connection::Builder::session()
        .method_timeout(CALL_TIMEOUT)
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

    let owner = portal_owner(&conn).await.map_err(|error| format!("The desktop portal is unavailable: {error}"))?;
    register(&conn, &owner).await?;
    let proxy = Proxy::new(&conn, owner.as_str(), ROOT, SHORTCUTS).await.map_err(|error| error.to_string())?;
    let portal = Portal(proxy);
    let session = portal.create_session().await?;
    portal.listen(&session).await
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

    async fn listen(&self, session: &OwnedObjectPath) -> Result<(), String> {
        let session_proxy = self.proxy(session.as_str(), SESSION).await.map_err(|error| error.to_string())?;
        let mut closed = session_proxy.receive_signal("Closed").await.map_err(|error| error.to_string())?;
        let mut owner = self.0.receive_owner_changed().await.map_err(|error| error.to_string())?;
        let events = self.0.receive_signal("Activated").await.map_err(|error| error.to_string())?;
        let monitor = async {
            self.ensure_shortcuts(session).await?;
            println!("Listening for FerroSeek global shortcuts.");
            forward_activations(session, events).await
        };
        match select(Box::pin(monitor), Box::pin(select(closed.next(), owner.next()))).await {
            iced::futures::future::Either::Left((result, _)) => result,
            iced::futures::future::Either::Right(_) => Ok(()),
        }
    }

    async fn ensure_shortcuts(&self, session: &OwnedObjectPath) -> Result<(), String> {
        let list = RequestToken::new()?;
        let properties = self.request("ListShortcuts", &list, &(session, list.options())).await?;
        if contains_known_shortcut(properties)? {
            return Ok(());
        }
        let bind = RequestToken::new()?;
        let properties =
            self.request("BindShortcuts", &bind, &(session, shortcut_definitions(), "", bind.options())).await?;
        if !contains_known_shortcut(properties)? {
            println!("No shortcuts have been assigned. Choose FerroSeek shortcuts in desktop settings.");
        }
        Ok(())
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

fn contains_known_shortcut(mut properties: Properties) -> Result<bool, String> {
    let shortcuts: Shortcuts = properties
        .remove("shortcuts")
        .ok_or("The desktop returned no shortcut list.")?
        .try_into()
        .map_err(|_| "The desktop returned an invalid shortcut list.")?;
    Ok(shortcuts.iter().any(|(id, _)| Target::from_id(id).is_some()))
}

enum Target {
    Open,
    Command(Builtin),
}

impl Target {
    fn from_id(id: &str) -> Option<Self> {
        if id == OPEN { Some(Self::Open) } else { Builtin::from_id(id).map(Self::Command) }
    }

    fn launch(self, executable: &std::path::Path) -> std::io::Result<()> {
        let mut command = Command::new(executable);
        command
            .process_group(0)
            .stdin(Stdio::null())
            .env_remove("XDG_ACTIVATION_TOKEN")
            .env_remove("DESKTOP_STARTUP_ID");
        if let Self::Command(command_id) = self {
            command.args(["--command", command_id.id()]);
        }
        let mut child = command.spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
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
}
