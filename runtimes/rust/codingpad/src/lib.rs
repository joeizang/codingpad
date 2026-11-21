//! Rust helpers available in CodingPad and in standalone Cargo programs.
mod rich;

pub use rich::{html, image, image_bytes, live, Html, Image, Live};

use std::cell::Cell;
use std::fmt::{self, Debug};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock};

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

fn send(fields: &[(&str, &str)]) -> bool {
    static OUTPUT: OnceLock<Mutex<Option<TcpStream>>> = OnceLock::new();
    let Ok(mut output) = OUTPUT.get_or_init(|| Mutex::new(connect())).lock() else {
        return false;
    };
    if output.is_none() {
        return false;
    }
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
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
    send_frame(&mut output, &line)
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
