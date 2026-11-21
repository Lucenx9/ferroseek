// SPDX-License-Identifier: GPL-3.0-or-later
//! Own the Wayland catalog requested by the installed launcher on Plasma.

use std::io;
use std::process::ExitCode;

use ferroseek_launcher::window_switcher::ipc::{self, Request, Response};
use ferroseek_launcher::window_switcher::{Session, Update};
use iced::futures::{StreamExt, channel::mpsc, future};

fn main() -> ExitCode {
    match zbus::block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ferroseek-plasma-windows: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> io::Result<()> {
    let (session, updates) = match Session::start() {
        Ok(session) => session,
        Err(error) => return publish(Response::Catalog(Update::Unavailable(error.into()))),
    };
    let (send, requests) = mpsc::unbounded();
    std::thread::Builder::new().name("window-requests".into()).spawn(move || receive_requests(send))?;
    match future::select(Box::pin(publish_catalog(updates)), Box::pin(handle_requests(session, requests))).await {
        future::Either::Left((result, _)) | future::Either::Right((result, _)) => result,
    }
}

fn receive_requests(send: mpsc::UnboundedSender<Request>) {
    let mut input = io::stdin().lock();
    let mut line = Vec::new();
    while let Ok(Some(request)) = ipc::receive(&mut input, &mut line, ipc::REQUEST_LIMIT) {
        if send.unbounded_send(request).is_err() {
            return;
        }
    }
}

async fn publish_catalog(mut updates: mpsc::UnboundedReceiver<Update>) -> io::Result<()> {
    while let Some(update) = updates.next().await {
        let unavailable = matches!(update, Update::Unavailable(_));
        publish(Response::Catalog(update))?;
        if unavailable {
            break;
        }
    }
    Ok(())
}

async fn handle_requests(session: Session, mut requests: mpsc::UnboundedReceiver<Request>) -> io::Result<()> {
    while let Some(Request { request, window, action }) = requests.next().await {
        let error = session.perform(window, action).await.err().map(str::to_owned);
        publish(Response::Finished { request, error })?;
    }
    Ok(())
}

fn publish(response: Response) -> io::Result<()> {
    ipc::send(&mut io::stdout().lock(), &response)
}
