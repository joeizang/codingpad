//! Rust helpers available in CodingPad and in standalone Cargo programs.
use std::fmt::Debug;
use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock};

/// Display a Rust value in Results, or pretty-print it in a terminal.
pub fn dump<T: Debug>(value: T) {
    let text = format!("{value:#?}");
    if !send(&[
        ("kind", "debug"),
        ("type", std::any::type_name::<T>()),
        ("text", &text),
    ]) {
        println!("{text}");
    }
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
    socket.write_all(token.as_bytes()).ok()?;
    Some(socket)
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
