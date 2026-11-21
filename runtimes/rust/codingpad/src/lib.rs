//! Rust helpers available in CodingPad and in standalone Cargo programs.
use std::fmt::Debug;
use std::io::{self, Read, Write};
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
