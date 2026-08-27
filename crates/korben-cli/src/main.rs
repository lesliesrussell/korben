//! The `korben` toolchain: one executable for the whole standard workflow.

// korben-6bc

mod commands;
mod repl;
mod ui;

use std::process::ExitCode;

/// Evaluation is recursive over the AST, so the toolchain runs on a thread with
/// a large stack. `Interp::max_depth` still bounds recursion and reports it as a
/// diagnostic rather than letting the process abort.
const STACK_SIZE: usize = 512 * 1024 * 1024;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let worker = std::thread::Builder::new()
        .name("korben".to_string())
        .stack_size(STACK_SIZE)
        .spawn(move || commands::run(&args));
    match worker
        .and_then(|handle| handle.join().map_err(|_| std::io::Error::other("worker panicked")))
    {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
