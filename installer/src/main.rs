//! tent-setup: a single executable that installs tent for the current user
//! without network access. `tent-setup --help` for options.

// No console window when double-clicked on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{self, Read};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

const PAYLOAD: &[u8] = include_bytes!(env!("TENT_PAYLOAD"));
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The embedded, zstd-compressed tent binary.
fn unpack() -> io::Result<Vec<u8>> {
    let mut bin = Vec::with_capacity(32 << 20);
    ruzstd::decoding::StreamingDecoder::new(PAYLOAD)
        .map_err(|e| io::Error::other(format!("corrupt payload: {e}")))?
        .read_to_end(&mut bin)?;
    Ok(bin)
}

fn main() {
    #[cfg(unix)]
    unix::main();
    #[cfg(windows)]
    windows::main();
}
