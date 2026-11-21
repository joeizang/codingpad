//! Rust helpers available in CodingPad and in standalone Cargo programs.
mod cache;
mod database;
mod rich;
mod wire;

pub use cache::{cache, CacheValue};
pub use database::{db, secret, Database, DbParameter, DbRows, DbValue};

pub use rich::{html, image, image_bytes, live, Html, Image, Live};

use std::cell::Cell;
use std::fmt::{self, Debug};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock};

/// Whether CodingPad has asked this run to stop. Long-running scripts can
/// poll this during their work and return to release resources before exiting.
pub fn is_cancelled() -> bool {
    let Some(cancel) = std::env::var_os("CODINGPAD_RUST_CANCEL").map(std::path::PathBuf::from)
    else {
        return false;
    };
    if !cancel.is_file() {
        return false;
    }
    // Distinguish a cooperative exit from one that was already finishing
    // independently when the host requested Stop.
    let _ = std::fs::write(cancel.with_extension("observed"), []);
    true
}

/// Display a Rust value in Results, or pretty-print it in a terminal.
pub fn dump<T: Debug>(value: T) {
    let formatted = format_debug(&value);
    if !send_debug("debug", 0, 0, &formatted) {
        if in_app() {
            // The channel may have closed after formatting. Don't print the
            // private rich-value markers to stdout.
            let _scope = FormatScope(FORMATTING_DUMP.replace(false));
            println!("{value:#?}");
        } else {
            println!("{}", formatted.text);
        }
    }
}

thread_local! {
    static FORMATTING_DUMP: Cell<bool> = const { Cell::new(false) };
}

fn formatting_dump() -> bool {
    FORMATTING_DUMP.get()
}

struct FormatScope(bool);

impl Drop for FormatScope {
    fn drop(&mut self) {
        FORMATTING_DUMP.set(self.0);
    }
}

struct FormattedDebug {
    text: String,
    name: &'static str,
    complete: bool,
}

fn format_debug<T: Debug>(value: &T) -> FormattedDebug {
    use std::fmt::Write;
    let rich = in_app();
    let _scope = FormatScope(FORMATTING_DUMP.replace(rich));
    let mut text = Preview(String::new());
    let complete = if rich {
        write!(text, "{value:?}")
    } else {
        write!(text, "{value:#?}")
    }
    .is_ok();
    FormattedDebug {
        text: text.0,
        name: std::any::type_name::<T>(),
        complete,
    }
}

fn send_debug(kind: &str, key: u64, revision: u64, value: &FormattedDebug) -> bool {
    send(&[
        ("kind", kind),
        ("key", &key.to_string()),
        ("revision", &revision.to_string()),
        ("type", value.name),
        ("text", &value.text),
        ("truncated", if value.complete { "false" } else { "true" }),
    ])
}

struct Preview(String);

impl fmt::Write for Preview {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        // A 4 MiB image needs about 5.34 MiB when encoded.
        const LIMIT: usize = 6 * 1024 * 1024;
        let remaining = LIMIT - self.0.len();
        if text.len() <= remaining {
            self.0.push_str(text);
            return Ok(());
        }
        let mut end = remaining;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.0.push_str(&text[..end]);
        Err(fmt::Error)
    }
}

#[doc(hidden)]
pub struct PanicGuard(bool);

impl Drop for PanicGuard {
    fn drop(&mut self) {
        if self.0 && std::thread::panicking() {
            send(&[("kind", "main_unwound")]);
        }
    }
}

#[doc(hidden)]
pub fn __start() -> PanicGuard {
    if !in_app() {
        return PanicGuard(false);
    }
    #[cfg(windows)]
    if let Some(ready) = std::env::var_os("CODINGPAD_RUST_START") {
        let ready = std::path::PathBuf::from(ready);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready.is_file() {
            if std::time::Instant::now() >= deadline {
                std::process::exit(1);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    let main = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() != main {
            previous(info);
            return;
        }
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("Panic with a non-string payload");
        let location = info.location();
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        if !send(&[
            ("kind", "panic"),
            ("message", message),
            ("file", location.map_or("", |location| location.file())),
            (
                "line",
                &location.map_or(0, |location| location.line()).to_string(),
            ),
            (
                "column",
                &location.map_or(0, |location| location.column()).to_string(),
            ),
            ("trace", &trace),
        ]) {
            previous(info);
        }
    }));
    PanicGuard(true)
}

fn in_app() -> bool {
    std::env::var_os("CODINGPAD_RUST_OUTPUT").is_some()
}

#[doc(hidden)]
pub fn __finish<T: Debug>(value: T) {
    if std::any::type_name::<T>() != "()" {
        dump(value);
    }
}

fn connect() -> Option<TcpStream> {
    let address = std::env::var("CODINGPAD_RUST_OUTPUT").ok()?;
    let token = std::env::var("CODINGPAD_RUST_TOKEN").ok()?;
    let mut socket = TcpStream::connect(address).ok()?;
    socket.set_nodelay(true).ok()?;
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok()?;
    socket.write_all(token.as_bytes()).ok()?;
    Some(socket)
}

static OUTPUT: OnceLock<Mutex<Option<TcpStream>>> = OnceLock::new();
const MAX_REPLY: usize = 8 * 1024 * 1024;

fn send(fields: &[(&str, &str)]) -> bool {
    let Ok(mut output) = OUTPUT.get_or_init(|| Mutex::new(connect())).lock() else {
        return false;
    };
    if output.is_none() {
        return false;
    }
    flush_streams();
    send_frame(&mut output, &frame(fields))
}

fn flush_streams() {
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
}

fn frame(fields: &[(&str, &str)]) -> String {
    let mut line = String::from("{");
    for (index, (key, value)) in fields.iter().enumerate() {
        if index > 0 {
            line.push(',');
        }
        quoted(&mut line, key);
        line.push(':');
        quoted(&mut line, value);
    }
    line.push_str("}\n");
    line
}

fn request(kind: &str, value: &str, parameters: &str) -> Result<Vec<u8>, String> {
    if value.len().saturating_add(parameters.len()) > 1024 * 1024 {
        return Err("The Rust request exceeds 1 MiB.".into());
    }
    let disconnected = "The CodingPad host connection is unavailable.";
    let mut output = OUTPUT
        .get_or_init(|| Mutex::new(connect()))
        .lock()
        .map_err(|_| disconnected)?;
    let socket = output.as_mut().ok_or(disconnected)?;
    flush_streams();
    let line = frame(&[("kind", kind), ("value", value), ("parameters", parameters)]);
    let mut exchange = || -> io::Result<(u8, Vec<u8>)> {
        // Wait as long as the host stays connected. Stop interrupts the request
        // through the host and closing the script drops the socket.
        socket.set_read_timeout(None)?;
        socket.write_all(line.as_bytes())?;
        let mut header = [0; 5];
        socket.read_exact(&mut header)?;
        let length = u32::from_le_bytes(header[1..].try_into().unwrap()) as usize;
        if length > MAX_REPLY || header[0] > 1 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut bytes = vec![0; length];
        socket.read_exact(&mut bytes)?;
        socket.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        Ok((header[0], bytes))
    };
    match exchange() {
        Ok((0, bytes)) => Ok(bytes),
        Ok((_, bytes)) => Err(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) => {
            *output = None;
            Err(format!("{disconnected} {error}"))
        }
    }
}

fn send_frame(output: &mut Option<TcpStream>, line: &str) -> bool {
    let Some(socket) = output.as_mut() else {
        return false;
    };
    let written = socket.write_all(line.as_bytes()).is_ok();
    let mut ack = [0];
    if !written || socket.read_exact(&mut ack).is_err() || ack != *b"1" {
        // A late ack must never be accepted for a subsequent frame.
        *output = None;
    }
    // The host may already have rendered a complete frame. Avoid echoing it
    // to stdout even if its ack was lost.
    written
}

// The built-in crate compiles offline and uses only the standard library.
fn quoted(output: &mut String, text: &str) {
    use std::fmt::Write;
    output.push('"');
    for character in text.chars() {
        match character {
            '"' | '\\' => {
                output.push('\\');
                output.push(character);
            }
            '\0'..='\u{1f}' => {
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            _ => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn a_missing_ack_retires_the_channel_without_echoing_the_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut host, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut output = Some(socket);

        assert!(send_frame(&mut output, "first\n"));
        assert!(output.is_none());
        assert!(!send_frame(&mut output, "second\n"));
        let mut received = String::new();
        host.read_to_string(&mut received).unwrap();
        assert_eq!(received, "first\n");
    }
}
