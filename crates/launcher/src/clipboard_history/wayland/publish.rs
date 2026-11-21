// SPDX-License-Identifier: GPL-3.0-or-later
//! Own a text selection independently of the launcher's surface and process.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Instant;

use wayland_client::{delegate_noop, globals::GlobalListContents};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1 as ext_source;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1 as wlr_source;

use super::{
    Connection, Device, Dispatch, MAX_TRANSFERS, MIME_TYPES, Protocol, QueueHandle, TEXT_LIMIT, TRANSFER_DEADLINE,
    advance, event_created_child, ext_device, ext_manager, ext_offer, ready, wl_registry, wl_seat, wlr_device,
    wlr_manager, wlr_offer,
};

#[derive(Debug)]
pub struct Lease(ChildStdout);

impl Lease {
    pub fn is_current(&self) -> bool {
        // When the compositor cancels the source, the owner exits and closes this pipe.
        matches!(ready(self.0.as_fd(), 0), Ok(false))
    }
}

pub fn handoff(text: &str) -> io::Result<Lease> {
    validate(text)?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--serve-clipboard")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let lease = match await_selection(&mut child, text.as_bytes()) {
        Ok(lease) => lease,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(lease)
}

fn await_selection(child: &mut Child, text: &[u8]) -> io::Result<Lease> {
    let mut input = child.stdin.take().expect("piped input");
    input.write_all(text)?;
    drop(input);
    let mut response = child.stdout.take().expect("piped response");
    let mut marker = [0];
    if !ready(response.as_fd(), 1200)? {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "The desktop did not accept the clipboard selection."));
    }
    response.read_exact(&mut marker)?;
    if marker != [1] {
        return Err(io::Error::other("Invalid clipboard handoff confirmation."));
    }
    Ok(Lease(response))
}

pub fn serve() -> io::Result<()> {
    let mut text = String::new();
    io::stdin().take(TEXT_LIMIT as u64 + 1).read_to_string(&mut text)?;
    validate(&text)?;
    let (globals, mut queue, protocol) = Protocol::connect()?;
    if globals.contents().clone_list().iter().filter(|global| global.interface == "wl_seat").count() != 1 {
        return Err(io::Error::other("Clipboard ownership requires exactly one input seat."));
    }
    let device = protocol.bind(&globals, &queue.handle())?;
    let mut state = Source { text, selected: false, closed: false, transfers: Vec::new() };
    queue.roundtrip(&mut state).map_err(io::Error::other)?;
    if state.closed {
        return Err(io::Error::other("The desktop does not allow clipboard ownership."));
    }
    state.selected = false;
    device.publish(&queue.handle());
    queue.roundtrip(&mut state).map_err(io::Error::other)?;
    if state.closed || !state.selected {
        return Err(io::Error::other("The desktop rejected the clipboard selection."));
    }
    acknowledge()?;
    while !state.closed {
        advance(&mut queue, &mut state)?;
        state.transfers.retain_mut(|transfer| transfer.write_step(state.text.as_bytes()));
    }
    Ok(())
}

fn acknowledge() -> io::Result<()> {
    let mut response = io::stdout().lock();
    match response.write_all(&[1]).and_then(|()| response.flush()) {
        // The launcher can be closed after sending the text.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

fn validate(text: &str) -> io::Result<()> {
    if super::super::accepts(text) {
        Ok(())
    } else {
        Err(io::Error::other("Clipboard text is empty or exceeds the limit."))
    }
}

struct Source {
    text: String,
    selected: bool,
    closed: bool,
    transfers: Vec<Transfer>,
}

impl Source {
    fn send(&mut self, mime: &str, fd: OwnedFd) {
        if !MIME_TYPES.contains(&mime) || self.transfers.len() == MAX_TRANSFERS {
            return;
        }
        let Some(mut transfer) = Transfer::new(fd) else { return };
        if transfer.write_step(self.text.as_bytes()) {
            self.transfers.push(transfer);
        }
    }
}

struct Transfer {
    file: File,
    offset: usize,
    deadline: Instant,
}

impl Transfer {
    fn new(fd: OwnedFd) -> Option<Self> {
        // The compositor transfers ownership of this pipe's write descriptor.
        let raw = fd.as_raw_fd();
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return None;
        }
        Some(Self { file: File::from(fd), offset: 0, deadline: Instant::now() + TRANSFER_DEADLINE })
    }

    /// Returns true while a reader can still receive more bytes.
    fn write_step(&mut self, bytes: &[u8]) -> bool {
        if Instant::now() >= self.deadline {
            return false;
        }
        match self.file.write(&bytes[self.offset..]) {
            Ok(0) => false,
            Ok(count) => {
                self.offset += count;
                self.offset < bytes.len()
            }
            Err(error) => matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted),
        }
    }
}

impl Device {
    fn publish(&self, queue: &QueueHandle<Source>) {
        match self {
            Self::Standard(manager, device) => {
                let source = manager.create_data_source(queue, ());
                for mime in MIME_TYPES {
                    source.offer(mime.into());
                }
                device.set_selection(Some(&source));
            }
            Self::Wlr(manager, device) => {
                let source = manager.create_data_source(queue, ());
                for mime in MIME_TYPES {
                    source.offer(mime.into());
                }
                device.set_selection(Some(&source));
            }
        }
    }
}

ignore_events!(Source, wl_registry::WlRegistry, GlobalListContents);
delegate_noop!(Source: ignore wl_seat::WlSeat);

macro_rules! dispatch_source {
    ($manager:ident::$Manager:ident, $device:ident::$Device:ident, $offer:ident::$Offer:ident, $source:ident::$Source:ident) => {
        delegate_noop!(Source: ignore $manager::$Manager);
        delegate_noop!(Source: ignore $offer::$Offer);

        impl Dispatch<$device::$Device, ()> for Source {
            fn event(
                state: &mut Self,
                _: &$device::$Device,
                event: $device::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                match event {
                    $device::Event::Selection { id } => {
                        state.selected = id.is_some();
                        if let Some(offer) = id {
                            offer.destroy();
                        }
                    }
                    $device::Event::PrimarySelection { id: Some(offer) } => offer.destroy(),
                    $device::Event::Finished => state.closed = true,
                    _ => {}
                }
            }

            event_created_child!(Source, $device::$Device, [0 => ($offer::$Offer, ())]);
        }

        impl Dispatch<$source::$Source, ()> for Source {
            fn event(
                state: &mut Self,
                source: &$source::$Source,
                event: $source::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                match event {
                    $source::Event::Send { mime_type, fd } => state.send(&mime_type, fd),
                    $source::Event::Cancelled => {
                        state.closed = true;
                        source.destroy();
                    }
                    _ => {}
                }
            }
        }
    };
}

dispatch_source!(
    ext_manager::ExtDataControlManagerV1,
    ext_device::ExtDataControlDeviceV1,
    ext_offer::ExtDataControlOfferV1,
    ext_source::ExtDataControlSourceV1
);
dispatch_source!(
    wlr_manager::ZwlrDataControlManagerV1,
    wlr_device::ZwlrDataControlDeviceV1,
    wlr_offer::ZwlrDataControlOfferV1,
    wlr_source::ZwlrDataControlSourceV1
);
