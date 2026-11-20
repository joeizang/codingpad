use std::{
    fmt::{self, Debug, Formatter},
    fs::File,
    io::{self, Read},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

/// Markup that Results renders in its sandboxed HTML viewer.
pub struct Html(String);

pub fn html(markup: impl AsRef<str>) -> Html {
    Html(markup.as_ref().chars().take(200_000).collect())
}

impl Debug for Html {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if !super::in_app() {
            return f.write_str(&self.0);
        }
        f.debug_tuple("__CodingPadHtml").field(&self.0).finish()
    }
}

/// A snapshot of image bytes ready to dump.
pub struct Image {
    mime: &'static str,
    bytes: Vec<u8>,
    name: Option<String>,
}

const MAX_IMAGE: usize = 4 * 1024 * 1024;

/// Read an image from a file, relative to the script's working directory.
pub fn image(path: impl AsRef<Path>) -> io::Result<Image> {
    let path = path.as_ref();
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_IMAGE as u64 + 1)
        .read_to_end(&mut bytes)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    make_image(bytes, name)
}

/// Create an image from encoded PNG, JPEG, GIF, WebP, BMP or SVG bytes.
pub fn image_bytes(bytes: impl AsRef<[u8]>) -> io::Result<Image> {
    let bytes = bytes.as_ref();
    check_size(bytes)?;
    make_image(bytes.to_vec(), None)
}

fn check_size(bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_IMAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Images are limited to 4 MiB",
        ));
    }
    Ok(())
}

fn make_image(bytes: Vec<u8>, name: Option<String>) -> io::Result<Image> {
    check_size(&bytes)?;
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if is_svg(&bytes) {
        "image/svg+xml"
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unknown image format",
        ));
    };
    Ok(Image { mime, bytes, name })
}

fn is_svg(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let mut header = text.trim_start_matches('\u{feff}').trim_start();
    if header.starts_with("<?xml") {
        header = header
            .split_once("?>")
            .map_or("", |(_, rest)| rest.trim_start());
    }
    header
        .strip_prefix("<svg")
        .is_some_and(|rest| rest.starts_with(['>', '/']) || rest.starts_with(char::is_whitespace))
}

impl Debug for Image {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if !super::in_app() {
            return write!(f, "[{} image, {} bytes]", self.mime, self.bytes.len());
        }
        f.debug_tuple("__CodingPadImage")
            .field(&self.mime)
            .field(&self.name.as_deref().unwrap_or(""))
            .field(&base64(&self.bytes))
            .finish()
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 3) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(((second & 15) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

/// A value that can update a Results block while the script is running.
pub struct Live<T> {
    id: u64,
    state: Mutex<LiveState<T>>,
}

struct LiveState<T> {
    value: T,
    revision: u64,
    watched: bool,
}

pub fn live<T: Debug>(value: T) -> Live<T> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    Live {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        state: Mutex::new(LiveState {
            value,
            revision: 0,
            watched: false,
        }),
    }
}

impl<T: Debug> Live<T> {
    /// Replace the displayed value. Pass `&value` to `dump` to keep updating it.
    pub fn set(&self, value: T) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.value = value;
        state.revision += 1;
        if state.watched {
            super::send_debug("live_update", self.id, state.revision, &state.value);
        }
    }
}

impl<T: Debug> Debug for Live<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Ok(mut state) = self.state.try_lock() else {
            return f.write_str("[live value unavailable]");
        };
        if !super::in_app() {
            return state.value.fmt(f);
        }
        state.watched = true;
        f.debug_tuple("__CodingPadLive")
            .field(&self.id)
            .field(&state.revision)
            .field(&state.value)
            .finish()
    }
}
