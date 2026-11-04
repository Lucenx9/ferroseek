// SPDX-License-Identifier: GPL-3.0-or-later
//! Observe the standardized desktop color preference on the existing bus thread.
use crate::backend::UiEvent;
use iced::futures::{StreamExt, channel::mpsc};
use std::time::Duration;
use zbus::{OwnedVariant, Variant};
const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";

fn dark(value: OwnedVariant) -> Option<bool> {
    let mut value: Variant<'_> = value.into();
    while let Variant::Variant(inner) = value {
        value = *inner;
    }
    match u32::try_from(value).ok()? {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}
pub async fn watch(conn: zbus::Connection, tx: mpsc::UnboundedSender<UiEvent>) {
    let Ok(runtime) = zruntime::SharedRuntime::current() else { return };
    let mut previous = None;
    let mut publish = |value| {
        if previous != Some(value) {
            previous = Some(value);
            tx.unbounded_send(UiEvent::DesktopAppearance(value)).is_ok()
        } else {
            !tx.is_closed()
        }
    };
    loop {
        let proxy = zbus::Proxy::new(&conn, DESTINATION, PATH, INTERFACE).await;
        if let Ok(proxy) = proxy {
            // Subscribe before reading so a concurrent desktop change is retained.
            if let Ok(mut changes) = proxy.receive_signal("SettingChanged").await {
                let read = async {
                    match proxy.call::<_, _, OwnedVariant>("ReadOne", &(NAMESPACE, "color-scheme")).await {
                        Err(zbus::Error::MethodError(name, _, _))
                            if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" =>
                        {
                            proxy.call("Read", &(NAMESPACE, "color-scheme")).await
                        }
                        other => other,
                    }
                };
                let value = match runtime.timeout(Duration::from_secs(2), read).await {
                    Ok(Ok(value)) => dark(value),
                    _ => {
                        if !publish(None) {
                            return;
                        }
                        runtime.sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                if !publish(value) {
                    return;
                }
                while let Some(message) = changes.next().await {
                    if let Ok((namespace, key, value)) = message.body().deserialize::<(String, String, OwnedVariant)>()
                        && namespace == NAMESPACE
                        && key == "color-scheme"
                        && !publish(dark(value))
                    {
                        return;
                    }
                }
            }
        }
        if !publish(None) {
            return;
        }
        runtime.sleep(Duration::from_secs(2)).await;
    }
}
