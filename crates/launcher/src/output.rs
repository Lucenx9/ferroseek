// SPDX-License-Identifier: GPL-3.0-or-later
//! Observe entered output bounds without taking over iced's surface.
//!
//! The backend does not expose its output events. An input-transparent 1px child
//! receives its own output Enter, while iced retains the parent and its commits.
use crate::activation::Worker;
use iced::futures::channel::mpsc;
use std::io::Write;
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};

#[derive(Debug, Clone, Copy)]
pub enum Event {
    Mapped,
    Bounds(Option<(u32, u32)>),
}
#[derive(Clone, Default)]
pub struct Observer(Option<Arc<Mutex<State>>>);
struct State {
    tx: mpsc::UnboundedSender<Event>,
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    shm: Option<wl_shm::WlShm>,
    manager: Option<(u32, zxdg_output_manager_v1::ZxdgOutputManagerV1)>,
    parent: Option<wl_surface::WlSurface>,
    probe: Option<Probe>,
    entered: Vec<EnteredOutput>,
    generation: u64,
    known: Option<(u32, u32)>,
    size: (u32, u32),
}
struct EnteredOutput {
    output: wl_output::WlOutput,
    xdg: Option<zxdg_output_v1::ZxdgOutputV1>,
    generation: u64,
    pending: Option<(u32, u32)>,
    bounds: Option<(u32, u32)>,
}
impl Drop for EnteredOutput {
    fn drop(&mut self) {
        if let Some(xdg) = self.xdg.take() {
            xdg.destroy();
        }
    }
}
struct Probe {
    surface: wl_surface::WlSurface,
    role: wl_subsurface::WlSubsurface,
    buffer: wl_buffer::WlBuffer,
    _file: std::fs::File,
}
impl Drop for Probe {
    fn drop(&mut self) {
        self.role.destroy();
        self.surface.destroy();
        self.buffer.destroy();
    }
}
impl Observer {
    pub fn new(size: (u32, u32)) -> (Self, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded();
        (
            Self(Some(Arc::new(Mutex::new(State {
                tx,
                compositor: None,
                subcompositor: None,
                shm: None,
                manager: None,
                parent: None,
                probe: None,
                entered: Vec::new(),
                generation: 0,
                known: None,
                size,
            })))),
            rx,
        )
    }
    pub fn resize(&self, size: (u32, u32)) {
        if let Some(state) = &self.0 {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            state.size = size;
            if let Some(probe) = &state.probe {
                probe.role.set_position((size.0 / 2) as i32, (size.1.min(100) / 2) as i32);
            }
        }
    }
    pub fn surface(&self, parent: wl_surface::WlSurface, queue: &QueueHandle<Worker>) {
        if let Some(state) = &self.0 {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            if state.parent.as_ref() != Some(&parent) {
                state.probe = None;
                state.entered.clear();
                state.notify(None);
                state.parent = Some(parent);
            }
            state.map(queue);
        }
    }
    pub fn registry(
        &self,
        registry: &wl_registry::WlRegistry,
        event: &wl_registry::Event,
        queue: &QueueHandle<Worker>,
    ) {
        let Some(state) = &self.0 else { return };
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            wl_registry::Event::Global { name, interface, .. } if interface == "wl_compositor" => {
                state.compositor = Some(registry.bind(*name, 1, queue, ()))
            }
            wl_registry::Event::Global { name, interface, .. } if interface == "wl_subcompositor" => {
                state.subcompositor = Some(registry.bind(*name, 1, queue, ()))
            }
            wl_registry::Event::Global { name, interface, .. } if interface == "wl_shm" => {
                state.shm = Some(registry.bind(*name, 1, queue, ()))
            }
            wl_registry::Event::Global { name, interface, version } if interface == "zxdg_output_manager_v1" => {
                state.manager = Some((*name, registry.bind(*name, (*version).min(2), queue, ())));
                state.bind_outputs(queue);
            }
            wl_registry::Event::GlobalRemove { name } if state.manager.as_ref().is_some_and(|(id, _)| id == name) => {
                if let Some((_, manager)) = state.manager.take() {
                    manager.destroy();
                }
                for output in &mut state.entered {
                    if let Some(xdg) = output.xdg.take() {
                        xdg.destroy();
                    }
                    output.pending = None;
                    output.bounds = None;
                }
                state.notify(None);
            }
            _ => {}
        }
        state.map(queue);
    }
    pub fn disconnect(&self) {
        if let Some(state) = &self.0 {
            state.lock().unwrap_or_else(|e| e.into_inner()).probe = None;
        }
    }
}
impl State {
    fn map(&mut self, queue: &QueueHandle<Worker>) {
        if self.probe.is_some() || self.manager.is_none() {
            return;
        }
        let (Some(parent), Some(compositor), Some(subcompositor), Some(shm)) =
            (&self.parent, &self.compositor, &self.subcompositor, &self.shm)
        else {
            return;
        };
        let file = match tempfile::tempfile().and_then(|mut file| {
            file.write_all(&[0; 4])?;
            Ok(file)
        }) {
            Ok(file) => file,
            Err(error) => {
                log::warn!("Cannot observe launcher output: {error}");
                return;
            }
        };
        let pool = shm.create_pool(file.as_fd(), 4, queue, ());
        let buffer = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, queue, ());
        pool.destroy();
        let surface = compositor.create_surface(queue, ());
        let region = compositor.create_region(queue, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        let role = subcompositor.get_subsurface(&surface, parent, queue, ());
        role.set_desync();
        role.set_position((self.size.0 / 2) as i32, (self.size.1.min(100) / 2) as i32);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage(0, 0, 1, 1);
        surface.commit();
        self.probe = Some(Probe { surface, role, buffer, _file: file });
        // Iced applies the parent commit, including the new child association.
        let _ = self.tx.unbounded_send(Event::Mapped);
    }
    fn bind_outputs(&mut self, queue: &QueueHandle<Worker>) {
        self.entered.retain(|member| member.output.is_alive());
        if let Some((_, manager)) = &self.manager {
            for member in &mut self.entered {
                if member.xdg.is_none() {
                    self.generation = self.generation.wrapping_add(1);
                    member.generation = self.generation;
                    member.xdg = Some(manager.get_xdg_output(&member.output, queue, member.generation));
                }
            }
        }
    }
    fn update_bounds(&mut self) {
        self.entered.retain(|member| member.output.is_alive());
        // An oversized surface can enter neighbouring outputs, in any order.
        // Fit all observed bounds until the compositor's target is the only
        // remaining member; choosing one Enter can sustain the wrong overlap.
        let bounds = self
            .entered
            .iter()
            .filter_map(|member| member.bounds)
            .reduce(|(w, h), (next_w, next_h)| (w.min(next_w), h.min(next_h)));
        self.notify(bounds);
    }
    fn notify(&mut self, bounds: Option<(u32, u32)>) {
        if self.known != bounds {
            self.known = bounds;
            let _ = self.tx.unbounded_send(Event::Bounds(bounds));
        }
    }
}
impl Dispatch<wl_surface::WlSurface, ()> for Worker {
    fn event(
        state: &mut Self,
        surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let Some(inner) = &state.output.0 else { return };
        let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.probe.as_ref().is_some_and(|probe| probe.surface == *surface) {
            return;
        }
        match event {
            wl_surface::Event::Enter { output } => {
                if !inner.entered.iter().any(|member| member.output == output) {
                    inner.entered.push(EnteredOutput { output, xdg: None, generation: 0, pending: None, bounds: None });
                }
                inner.bind_outputs(queue);
            }
            wl_surface::Event::Leave { output } => {
                inner.entered.retain(|member| member.output != output);
                inner.update_bounds();
            }
            _ => {}
        }
    }
}
impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u64> for Worker {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        generation: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(inner) = &state.output.0 else { return };
        let mut inner = inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(member) =
            inner.entered.iter_mut().find(|member| member.generation == *generation && member.xdg.is_some())
        else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalSize { width, height } if width > 0 && height > 0 => {
                member.pending = Some((width as u32, height as u32))
            }
            zxdg_output_v1::Event::Done => {
                member.bounds = member.pending;
                inner.update_bounds();
            }
            _ => {}
        }
    }
}
wayland_client::delegate_noop!(Worker:ignore wl_subcompositor::WlSubcompositor);
wayland_client::delegate_noop!(Worker:ignore wl_subsurface::WlSubsurface);
wayland_client::delegate_noop!(Worker:ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Worker:ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Worker:ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Worker:ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);
