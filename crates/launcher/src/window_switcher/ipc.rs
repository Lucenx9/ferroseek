// SPDX-License-Identifier: GPL-3.0-or-later
//! Messages on the private stdio of the Plasma window helper.

use std::io::{self, BufRead, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{Action, Update};

pub const HELPER: &str = "ferroseek-plasma-windows";
pub const REQUEST_LIMIT: u64 = 4096;
pub const RESPONSE_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
pub struct Request {
    pub request: u64,
    pub window: u64,
    pub action: Action,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum Response {
    Catalog(Update),
    Finished { request: u64, error: Option<String> },
}

pub fn receive<T: DeserializeOwned>(input: &mut impl BufRead, line: &mut Vec<u8>, limit: u64) -> io::Result<Option<T>> {
    line.clear();
    if input.take(limit).read_until(b'\n', line)? == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "incomplete helper message"));
    }
    Ok(Some(serde_json::from_slice(line)?))
}

pub fn send(output: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    output.write_all(&line)?;
    output.flush()
}
