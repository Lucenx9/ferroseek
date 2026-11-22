// SPDX-License-Identifier: GPL-3.0-or-later
//! Use a separate executable for the Plasma window-management permission.

use std::env;
use std::future::Future;
use std::io::{self, BufRead, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::futures::{
    FutureExt,
    channel::{mpsc, oneshot},
    future::Either,
};

use super::ipc::{self, Request, Response};
use super::{Action, DISCONNECTED, Session, UNCONFIRMED, Update};

pub enum Client {
    Direct(Session),
    Plasma(Helper),
}

impl Client {
    pub fn start() -> Result<(Self, mpsc::UnboundedReceiver<Update>), String> {
        let plasma = env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|desktop| desktop.split(':').any(|name| name.eq_ignore_ascii_case("KDE")));
        // A helper needs its own Wayland connection. It cannot borrow the
        // launcher's socket when it was started with a preconnected descriptor.
        let can_launch_helper = plasma && env::var_os("WAYLAND_SOCKET").is_none();
        if can_launch_helper && let Ok((helper, updates)) = Helper::start() {
            return Ok((Self::Plasma(helper), updates));
        }
        Session::start().map(|(session, updates)| (Self::Direct(session), updates)).map_err(str::to_owned)
    }

    pub fn perform(&mut self, id: u64, action: Action) -> impl Future<Output = Result<(), String>> + use<> {
        match self {
            Self::Direct(session) => {
                Either::Left(session.perform(id, action).map(|reply| reply.map_err(str::to_owned)))
            }
            Self::Plasma(helper) => Either::Right(helper.perform(id, action)),
        }
    }
}

type Reply = oneshot::Sender<Result<(), String>>;
type Pending = Arc<Mutex<Option<(u64, Reply)>>>;

pub struct Helper {
    input: ChildStdin,
    pending: Pending,
    next_request: u64,
}

impl Helper {
    fn start() -> io::Result<(Self, mpsc::UnboundedReceiver<Update>)> {
        let path = env::current_exe()?.with_file_name(ipc::HELPER);
        let mut process = Process(Command::new(path).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?);
        let input = process.0.stdin.take().expect("piped helper stdin");
        let output = process.0.stdout.take().expect("piped helper stdout");
        let (send, receive) = mpsc::unbounded();
        let pending = Pending::default();
        let replies = pending.clone();
        std::thread::Builder::new().name("plasma-windows".into()).spawn(move || {
            read_responses(BufReader::new(output), &send, &replies);
            replies.lock().unwrap().take();
            drop(process);
        })?;
        Ok((Self { input, pending, next_request: 0 }, receive))
    }

    fn perform(&mut self, window: u64, action: Action) -> impl Future<Output = Result<(), String>> + use<> {
        self.next_request += 1;
        let request = self.next_request;
        let pending = self.pending.clone();
        let (reply, receive) = oneshot::channel();
        pending.lock().unwrap().replace((request, reply));
        let sent = ipc::send(&mut self.input, &Request { request, window, action }, ipc::REQUEST_LIMIT);
        async move {
            let result = wait_for_reply(sent, receive).await;
            take_reply(&pending, request);
            result
        }
    }
}

async fn wait_for_reply(sent: io::Result<()>, receive: oneshot::Receiver<Result<(), String>>) -> Result<(), String> {
    sent.map_err(|_| DISCONNECTED)?;
    let runtime = zruntime::SharedRuntime::current().map_err(|_| DISCONNECTED)?;
    // The server already bounds confirmation at two seconds. Leave time for
    // its reply to cross the pipe before treating the helper as unresponsive.
    runtime.timeout(Duration::from_secs(3), receive).await.map_err(|_| UNCONFIRMED)?.map_err(|_| DISCONNECTED)?
}

fn take_reply(pending: &Pending, request: u64) -> Option<Reply> {
    let mut pending = pending.lock().unwrap();
    if pending.as_ref().is_some_and(|(id, _)| *id == request) { pending.take().map(|(_, reply)| reply) } else { None }
}

fn read_responses(mut input: impl BufRead, updates: &mpsc::UnboundedSender<Update>, pending: &Pending) {
    let mut line = Vec::new();
    let mut windows = Vec::new();
    while let Ok(Some(response)) = ipc::receive::<Response>(&mut input, &mut line, ipc::RESPONSE_LIMIT) {
        match response {
            Response::Windows { entries, complete } => {
                windows.extend(entries);
                if complete && updates.unbounded_send(Update::Windows(std::mem::take(&mut windows))).is_err() {
                    return;
                }
            }
            Response::Unavailable(error) => {
                let _ = updates.unbounded_send(Update::Unavailable(error));
                return;
            }
            Response::Finished { request, error } => {
                if let Some(reply) = take_reply(pending, request) {
                    let _ = reply.send(error.map_or(Ok(()), Err));
                }
            }
        }
    }
    let _ = updates.unbounded_send(Update::Unavailable(DISCONNECTED.into()));
}

// Only the reader owns the process, so cleanup never waits on the UI thread
// when the view closes its side of the pipe.
struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_reply_cannot_consume_the_next_request() {
        let (send, mut receive) = oneshot::channel();
        let pending = Arc::new(Mutex::new(Some((2, send))));
        assert!(take_reply(&pending, 1).is_none());

        take_reply(&pending, 2).unwrap().send(Ok(())).unwrap();
        assert_eq!(receive.try_recv().unwrap(), Some(Ok(())));
        assert!(take_reply(&pending, 2).is_none());
    }

    #[test]
    fn only_complete_window_snapshots_are_delivered() {
        use super::super::Entry;

        let mut input = Vec::new();
        for (id, complete) in [(1, false), (2, true), (3, false)] {
            let entries = [Entry { id, ..Entry::default() }];
            ipc::send(&mut input, &Response::Windows { entries, complete }, ipc::RESPONSE_LIMIT).unwrap();
        }
        let (send, mut updates) = mpsc::unbounded();
        read_responses(input.as_slice(), &send, &Pending::default());
        let Update::Windows(windows) = updates.try_recv().unwrap() else { panic!("expected a complete snapshot") };
        assert_eq!(windows.iter().map(|window| window.id).collect::<Vec<_>>(), [1, 2]);
        assert!(matches!(updates.try_recv(), Ok(Update::Unavailable(_))));
        assert!(updates.try_recv().is_err());
    }
}
