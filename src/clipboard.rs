//! Terminal clipboard copy via OSC 52.
//!
//! OSC 52 is a terminal escape sequence that sets the system clipboard from
//! inside a TUI. Supported by iTerm2, kitty, `WezTerm`, Alacritty, xterm (with
//! `allowSendEvents`), tmux (with `set -g set-clipboard on`) and most modern
//! emulators — and it works over SSH, unlike calling `xclip`/`pbcopy`.
//!
//! Format: `ESC ] 52 ; c ; <base64(content)> BEL`
//!
//! We intentionally do **not** add a native-clipboard crate dependency. `OSC 52`
//! covers remote and local use uniformly, and if the terminal silently
//! discards the sequence the worst case is that the status message lies — no
//! crash, no security surprise.

use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Maximum bytes we'll attempt to copy. Some terminals truncate OSC 52 at
/// 8KB–100KB; we refuse to send pathologically large blocks rather than
/// silently corrupting the clipboard.
const MAX_COPY_BYTES: usize = 512 * 1024;

/// Write the OSC 52 "set clipboard" escape sequence for `content` to `out`.
/// Returns the number of content bytes copied, or `None` if the content
/// exceeds the safety cap.
pub fn copy_to_clipboard<W: Write>(out: &mut W, content: &str) -> io::Result<Option<usize>> {
    let bytes = content.as_bytes();
    if bytes.len() > MAX_COPY_BYTES {
        return Ok(None);
    }
    let encoded = base64_encode(bytes);
    // ESC ] 52 ; c ; <base64> BEL
    //   'c' = clipboard selection (system clipboard).
    //   BEL (0x07) terminates the OSC sequence.
    out.write_all(b"\x1b]52;c;")?;
    out.write_all(encoded.as_bytes())?;
    out.write_all(b"\x07")?;
    out.flush()?;
    Ok(Some(bytes.len()))
}

/// Convenience wrapper: copy to the system clipboard, using whichever
/// transport is most likely to succeed. When running locally and a native
/// clipboard tool is available (`pbcopy`, `wl-copy`, `xclip`, `xsel`), use
/// it directly — native tools set the real OS clipboard deterministically.
/// Otherwise (remote session, or no native tool found) fall back to OSC 52,
/// which the terminal may or may not honor. Uses a write-then-flush lock so
/// the escape sequence can't interleave with other terminal output.
pub fn copy(content: &str) -> io::Result<Option<usize>> {
    if !is_ssh_session() {
        if let Some(n) = try_native_copy(content) {
            return Ok(Some(n));
        }
    }
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    copy_to_clipboard(&mut handle, content)
}

/// Try each locally available clipboard command in turn; return the first
/// byte-count that succeeds, or `None` if nothing in the environment looks
/// like a GUI clipboard or none of the candidate binaries are installed.
fn try_native_copy(content: &str) -> Option<usize> {
    if content.len() > MAX_COPY_BYTES {
        return None;
    }
    for (cmd, args) in native_copy_candidates() {
        if let Ok(true) = run_pipe(cmd, args, content) {
            return Some(content.len());
        }
    }
    None
}

/// Ordered list of `(command, args)` pairs to try for native clipboard copy.
/// Order reflects platform likelihood: macOS ships `pbcopy`, Wayland sessions
/// expose `wl-copy`, X11 sessions expose `xclip`/`xsel`. Tools whose binary
/// isn't present fail-fast on `spawn`.
fn native_copy_candidates() -> Vec<(&'static str, &'static [&'static str])> {
    let mut out: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if cfg!(target_os = "macos") {
        out.push(("pbcopy", &[]));
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        out.push(("wl-copy", &[]));
    }
    if std::env::var_os("DISPLAY").is_some() {
        out.push(("xclip", &["-selection", "clipboard"]));
        out.push(("xsel", &["--clipboard", "--input"]));
    }
    if cfg!(target_os = "windows") {
        // `clip.exe` reads stdin on Windows; effectively the native path.
        out.push(("clip", &[]));
    }
    out
}

fn run_pipe(cmd: &str, args: &[&str], content: &str) -> io::Result<bool> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(content.as_bytes())?;
    }
    Ok(child.wait()?.success())
}

/// Detect whether we're running inside an SSH session. OSC 52 *can* work over
/// SSH in principle, but many setups (tmux without `set -g set-clipboard on`,
/// stripped escape sequences in the middle hop, terminals that silently drop
/// the sequence when nested) break it. We use this to hide the copy hint so
/// users aren't misled into expecting it to work.
pub fn is_ssh_session() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_CLIENT").is_some()
        || std::env::var_os("SSH_TTY").is_some()
}

/// Minimal standard-alphabet base64 encoder (RFC 4648). Inlined to avoid a
/// dependency and to keep the attack surface small — we only ever emit this
/// into the user's terminal, never parse untrusted input.
fn base64_encode(input: &[u8]) -> String {
    const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n =
            (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8) | u32::from(input[i + 2]);
        out.push(ALPH[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPH[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPH[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPH[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = u32::from(input[i]) << 16;
        out.push(ALPH[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPH[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = (u32::from(input[i]) << 16) | (u32::from(input[i + 1]) << 8);
        out.push(ALPH[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPH[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPH[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64_rfc_vectors() {
        // RFC 4648 §10 test vectors.
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn emits_osc52_envelope() {
        let mut buf: Vec<u8> = Vec::new();
        let n = copy_to_clipboard(&mut buf, "hi").unwrap();
        assert_eq!(n, Some(2));
        // Starts with ESC ] 52 ; c ; and ends with BEL.
        assert_eq!(&buf[..7], b"\x1b]52;c;");
        assert_eq!(*buf.last().unwrap(), 0x07);
    }

    #[test]
    fn oversized_content_is_rejected() {
        let huge = "a".repeat(MAX_COPY_BYTES + 1);
        let mut buf: Vec<u8> = Vec::new();
        let n = copy_to_clipboard(&mut buf, &huge).unwrap();
        assert_eq!(n, None);
        assert!(buf.is_empty(), "rejected input should not emit any bytes");
    }
}
