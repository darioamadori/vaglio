//! Copying to the system clipboard.

use std::io::Write;
use std::process::{Command, Stdio};

/// `pbcopy` on macOS, `wl-copy` or `xclip` elsewhere.
pub fn copy(text: &str) -> bool {
    let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    };
    tools.iter().any(|(tool, args)| {
        let Ok(mut child) = Command::new(tool).args(*args).stdin(Stdio::piped()).stderr(Stdio::null()).spawn() else {
            return false;
        };
        let wrote = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        wrote && child.wait().is_ok_and(|s| s.success())
    })
}
