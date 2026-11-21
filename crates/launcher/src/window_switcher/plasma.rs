// SPDX-License-Identifier: GPL-3.0-or-later
//! Adapt the optional Plasma window-management global to the shared catalog.

use wayland_client::globals::Global;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Proxy, QueueHandle};
use wayland_protocols_plasma::plasma_window_management::client::{
    org_kde_plasma_window as handle, org_kde_plasma_window_management as manager,
};

use super::{Action, Catalog, Entry};

pub(super) type Manager = manager::OrgKdePlasmaWindowManagement;
pub(super) type Handle = handle::OrgKdePlasmaWindow;

pub(super) fn bind(registry: &WlRegistry, global: &Global, queue: &QueueHandle<Catalog>) -> bool {
    if global.interface != Manager::interface().name || global.version < 13 {
        return false;
    }
    // Recent KWin requires at least v17. Earlier versions can expose v13–16.
    let _: Manager = registry.bind(global.name, global.version.min(17), queue, ());
    true
}

pub(super) struct Window {
    pub handle: Handle,
    initial_state: bool,
    skip_switcher: bool,
}

impl Window {
    pub(super) fn request(&self, action: Action) {
        use manager::State;

        let (flag, enabled) = match action {
            Action::Switch => (State::Active, true),
            Action::Maximize(enabled) => (State::Maximized, enabled),
            Action::Minimize(enabled) => (State::Minimized, enabled),
        };
        let flag = flag as u32;
        self.handle.set_state(flag, if enabled { flag } else { 0 });
    }

    fn update(&mut self, entry: &mut Entry, event: handle::Event) -> bool {
        use manager::State;

        match event {
            handle::Event::TitleChanged { title } => entry.title = title,
            handle::Event::AppIdChanged { app_id } => entry.app_id = app_id,
            handle::Event::StateChanged { flags } => {
                let has = |state: State| flags & state as u32 != 0;
                entry.activated = has(State::Active);
                entry.maximized = has(State::Maximized);
                entry.minimized = has(State::Minimized);
                self.skip_switcher = has(State::Skipswitcher);
            }
            handle::Event::InitialState => self.initial_state = true,
            _ => {}
        }
        self.initial_state && !self.skip_switcher
    }
}

impl Catalog {
    pub(super) fn plasma_manager_event(&mut self, manager: &Manager, event: manager::Event, queue: &QueueHandle<Self>) {
        if let manager::Event::WindowWithUuid { uuid, .. } = event {
            let handle = manager.get_window_by_uuid(uuid, queue, ());
            self.add(super::Window::Plasma(Window { handle, initial_state: false, skip_switcher: false }));
        }
    }

    pub(super) fn plasma_window_event(&mut self, handle: &Handle, event: handle::Event) {
        let Some(&id) = self.handles.get(&handle.id()) else { return };
        if matches!(event, handle::Event::Unmapped) {
            self.close(id);
        } else if let Some(record) = self.windows.get_mut(&id)
            && let super::Window::Plasma(window) = &mut record.window
        {
            let selectable = window.update(&mut record.draft, event);
            self.changed |= record.publish(selectable);
        }
        self.confirm_change();
    }
}
