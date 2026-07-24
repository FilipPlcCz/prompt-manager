//! prompt-manager-mcp – MCP server (stdio transport, JSON-RPC 2.0).
//!
//! Exposes the Prompt Manager library to LLM clients (Claude Code,
//! Claude Desktop, ...). Newline-delimited JSON-RPC on stdin/stdout.
//!
//! Library path resolution (first match wins):
//!   1. --library <path> argument
//!   2. PROMPT_MANAGER_LIBRARY environment variable
//!   3. settings.json written by the desktop app
//!   4. %APPDATA%/PromptManager/library (or ~/.config/PromptManager/library)

use pm_mcp::{default_library_root, handle_message};
use std::io::{BufRead, Write};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut library: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--library" => library = args.next(),
            "--help" | "-h" => {
                eprintln!("prompt-manager-mcp [--library <path>]");
                return;
            }
            _ => {}
        }
    }
    let root = library
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_library_root);
    eprintln!("prompt-manager-mcp: library = {}", root.display());

    // one-off reconciliation of variables.json with the recipes (no-op when
    // the library is already consistent)
    if let Ok(lib) = pm_core::Library::open(&root) {
        if let Err(e) = lib.migrate() {
            eprintln!("prompt-manager-mcp: library migration failed: {}", e);
        }
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_message(&line, &root) {
            let mut out = stdout.lock();
            let _ = out.write_all(reply.as_bytes());
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
    }
}
