//! Tauri command surface. Kept intentionally tiny for the P0 scaffold: a single
//! `ping` round-trip that proves the JS -> Rust IPC path and the tauri-specta
//! binding-generation pipeline end to end.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Reply returned by [`ping`]. Derives `specta::Type` so tauri-specta can emit a
/// matching TypeScript interface into `apps/mdreader/ui/src/bindings.ts`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PingResponse {
    /// The message echoed back to the caller.
    pub echoed: String,
    /// The desktop shell's crate version.
    pub version: String,
}

/// Echoes `msg` back to the webview alongside the shell version.
#[tauri::command]
#[specta::specta]
pub fn ping(msg: String) -> PingResponse {
    PingResponse {
        echoed: msg,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}
