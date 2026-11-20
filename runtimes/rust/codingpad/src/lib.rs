//! Rust helpers available in CodingPad and in standalone Cargo programs.
mod rich;

pub use rich::{html, image, image_bytes, live, Html, Image, Live};

use std::fmt::{self, Debug};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock};

/// Display a Rust value in Results, or pretty-print it in a terminal.
pub fn dump<T: Debug>(value: T) {
    send_debug("debug", 0, 0, &value);
}

fn send_debug<T: Debug>(kind: &str, key: u64, revision: u64, value: &T) {
    use std::fmt::Write;
    let mut text = Preview(String::new());
    let complete = if in_app() {
        write!(text, "{value:?}")
    } else {
        write!(text, "{value:#?}")
    }
    .is_ok();
    if !send(&[
        ("kind", kind),
        ("key", &key.to_string()),
        ("revision", &revision.to_string()),
        ("type", std::any::type_name::<T>()),
        ("text", &text.0),
        ("truncated", if complete { "false" } else { "true" }),
    ]) {
        println!("{}", text.0);
    }
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
pub fn __start() {
    if !in_app() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
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
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok()?;
    socket.write_all(token.as_bytes()).ok()?;
    let mut ack = [0];
    socket.read_exact(&mut ack).ok()?;
    (ack == *b"1").then_some(socket)
}

fn send(fields: &[(&str, &str)]) -> bool {
    static OUTPUT: OnceLock<Mutex<Option<TcpStream>>> = OnceLock::new();
    let Ok(mut output) = OUTPUT.get_or_init(|| Mutex::new(connect())).lock() else {
        return false;
    };
    let Some(socket) = output.as_mut() else {
        return false;
    };
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
    socket.write_all(line.as_bytes()).is_ok()
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
