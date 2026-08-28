//! The Korben language server.

// korben-efd

pub mod analysis;
pub mod json;
pub mod position;
pub mod rpc;
pub mod server;

use std::io::{BufReader, BufWriter};
use std::path::PathBuf;

/// Serve the Language Server Protocol on stdin and stdout.
pub fn serve() -> Result<(), String> {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = BufReader::new(stdin.lock());
    let mut output = BufWriter::new(stdout.lock());
    server::Server::new(root).serve(&mut input, &mut output)
}
