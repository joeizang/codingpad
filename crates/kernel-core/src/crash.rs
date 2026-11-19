//! "Report on GitHub" crash reports without telemetry (D31).
//!
//! A crash never sends anything. CodingPad prepares a GitHub issue carrying the
//! app version, the platform and the error details, and the user reviews and
//! submits it in the browser. Scripts and connection details are never read on
//! this path. The user's home folder and credential-shaped values are redacted,
//! and the backtrace stays on the local crash screen rather than in a public
//! issue.
//!
//! A fatal panic is written to a small file in the app data folder so the next
//! launch can offer the same report, because the process that panicked may be
//! gone before it can draw anything. A clean exit discards that file.

use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    fs,
    path::{Path, PathBuf},
};

/// The GitHub form that receives a reviewed crash report.
pub const NEW_ISSUE_URL: &str = "https://github.com/joeizang/codingpad/issues/new";

/// The crash file left behind by a panic, relative to the app data folder.
pub const PENDING_FILE: &str = "pending-crash.json";

/// Long error text is cut here so a report stays readable.
const MAX_MESSAGE: usize = 2_000;
const MAX_LOCATION: usize = 300;
const MAX_BACKTRACE: usize = 4_000;
const MAX_TITLE: usize = 80;
/// The shortest message a report keeps when the link would otherwise be too long.
const MAX_FALLBACK_MESSAGE: usize = 300;

/// Keys whose assigned value never belongs in a public issue.
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "token",
    "secret",
    "api_key",
    "apikey",
    "access_key",
    "accesskey",
    "private_key",
    "connection_string",
    "connectionstring",
];

/// Browsers and servers disagree about long request lines; stay well under the
/// smallest common limit. A report that would be longer drops its backtrace,
/// then keeps only a short message, rather than producing an unusable link.
const MAX_URL: usize = 7_000;

/// The error details a report may carry. Nothing else is included.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashDetails {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backtrace: Option<String>,
}

impl CrashDetails {
    /// Error details from a message and optional position/backtrace.
    pub fn new(
        message: impl Into<String>,
        location: Option<String>,
        backtrace: Option<String>,
    ) -> Self {
        Self {
            message: message.into(),
            location,
            backtrace,
        }
        .normalized()
    }

    /// Trim and bound every field so a report cannot grow without limit.
    fn normalized(self) -> Self {
        let message = limited(&self.message, MAX_MESSAGE);
        let location = self
            .location
            .as_deref()
            .map(|text| limited(first_line(text), MAX_LOCATION))
            .filter(|text| !text.is_empty());
        let backtrace = self
            .backtrace
            .as_deref()
            .map(|text| limited(text, MAX_BACKTRACE))
            .filter(|text| !text.is_empty());
        Self {
            message: if message.is_empty() {
                "CodingPad stopped with an unexpected error.".to_owned()
            } else {
                message
            },
            location,
            backtrace,
        }
    }
}

/// A reviewed issue, ready to open in the browser. Only the URL leaves Rust.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrashReport {
    pub title: String,
    pub body: String,
    pub url: String,
}

/// The running build, as shown in the crash screen and the issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashEnvironment {
    pub version: String,
    pub platform: String,
    pub arch: String,
}

impl CrashEnvironment {
    /// The platform line that appears in a report, for example `linux (x86_64)`.
    pub fn platform_line(&self) -> String {
        format!("{} ({})", self.platform, self.arch)
    }
}

/// Describe the running build. The version comes from the app, not from the
/// core crate, so an installer and its report always agree.
pub fn environment(version: &str) -> CrashEnvironment {
    CrashEnvironment {
        version: version.to_owned(),
        platform: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
    }
}

/// Build the pre-filled issue. Nothing here opens a connection: the caller
/// shows the link and the user decides whether to submit it.
pub fn report(details: &CrashDetails, environment: &CrashEnvironment) -> CrashReport {
    let home = dirs::home_dir();
    report_with_home(details, environment, home.as_deref())
}

/// Build a report without the parts that could identify the user's account or
/// carry a credential. The backtrace stays on the local crash screen: it is the
/// most likely place for source paths and production values to leak.
fn report_with_home(
    details: &CrashDetails,
    environment: &CrashEnvironment,
    home: Option<&Path>,
) -> CrashReport {
    let mut details = details.clone().normalized();
    details.message = redact(&details.message, home);
    details.location = details.location.as_deref().map(|place| redact(place, home));
    details.backtrace = None;
    let report = compose(&details, environment);
    if report.url.len() <= MAX_URL {
        return report;
    }
    // An unusually long or non-ASCII message: keep a readable prefix.
    details.message = limited(&details.message, MAX_FALLBACK_MESSAGE);
    compose(&details, environment)
}

/// Lay out one issue with exactly the details it is given.
fn compose(details: &CrashDetails, environment: &CrashEnvironment) -> CrashReport {
    let title = format!(
        "Crash report: {}",
        limited(first_line(&details.message), MAX_TITLE)
    );
    let mut body = String::new();
    body.push_str("### What happened\n\n");
    body.push_str(&code_block(&details.message));
    body.push_str("\n\n### Environment\n\n");
    body.push_str(&format!(
        "- CodingPad {}\n- {}\n",
        environment.version,
        environment.platform_line()
    ));
    if let Some(location) = &details.location {
        body.push_str("\n### Where\n\n");
        body.push_str(&code_block(location));
        body.push('\n');
    }
    if let Some(backtrace) = &details.backtrace {
        body.push_str("\n### Backtrace\n\n");
        body.push_str(&code_block(backtrace));
        body.push('\n');
    }
    body.push_str(
        "\n_CodingPad sent nothing automatically. This report holds only the app \
         version, the platform and the error details above. Review it before \
         submitting and remove anything you would not want public._\n",
    );
    let mut url = url::Url::parse(NEW_ISSUE_URL).expect("the issue form URL is valid");
    url.query_pairs_mut()
        .append_pair("title", &title)
        .append_pair("body", &body)
        .append_pair("labels", "bug");
    CrashReport {
        title,
        body,
        url: url.into(),
    }
}

/// Keep only the first line of a multi-line message, for titles and locations.
fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or_default()
}

/// Trim, then cut to `max` characters, marking a cut with an ellipsis.
fn limited(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

/// A Markdown code fence that survives backticks inside the text.
fn code_block(text: &str) -> String {
    let longest_run = text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run.max(2) + 1);
    format!("{fence}\n{}\n{fence}", text.trim_end())
}

/// Remove the user's home folder and credential-shaped values from text that
/// may reach a public issue. This is best effort: the crash screen still asks
/// the user to review the issue before submitting.
fn redact(text: &str, home: Option<&Path>) -> String {
    let mut text = text.to_owned();
    if let Some(home) = home {
        let home = home.to_string_lossy();
        if !home.is_empty() {
            text = text.replace(home.as_ref(), "~");
            if home.contains('\\') {
                text = text.replace(&home.replace('\\', "/"), "~");
            }
        }
    }
    let text = redact_url_userinfo(&text);
    redact_assignments(&text)
}

/// `scheme://user:pass@host` becomes `scheme://[redacted]@host`.
fn redact_url_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("://") {
        out.push_str(&rest[..index + 3]);
        rest = &rest[index + 3..];
        let end = rest
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(
                        character,
                        '/' | '?' | '#' | '"' | '\'' | ')' | ']' | '}' | ','
                    )
            })
            .unwrap_or(rest.len());
        let authority = &rest[..end];
        if let Some(at) = authority.rfind('@') {
            out.push_str("[redacted]");
            out.push_str(&authority[at..]);
        } else {
            out.push_str(authority);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// `Password=hunter2`, `token: abc` and similar assignments keep their key but
/// lose their value.
fn redact_assignments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_alphabetic() {
            index += next_char_len(text, index);
            continue;
        }
        if index > 0 {
            let previous = bytes[index - 1];
            if previous.is_ascii_alphanumeric() || previous == b'_' {
                index += next_char_len(text, index);
                continue;
            }
        }
        let mut after_key = None;
        for key in SECRET_KEYS {
            let Some(part) = text.get(index..index + key.len()) else {
                continue;
            };
            if !part.eq_ignore_ascii_case(key) {
                continue;
            }
            let end = index + key.len();
            let boundary =
                end == bytes.len() || (!bytes[end].is_ascii_alphanumeric() && bytes[end] != b'_');
            if boundary {
                after_key = Some(end);
                break;
            }
        }
        let Some(mut cursor) = after_key else {
            index += next_char_len(text, index);
            continue;
        };
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == bytes.len() || !matches!(bytes[cursor], b'=' | b':') {
            index = cursor.max(index + 1);
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = bytes
            .get(cursor)
            .copied()
            .filter(|byte| matches!(byte, b'"' | b'\''));
        if quote.is_some() {
            cursor += 1;
        }
        let value_start = cursor;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            let ends = match quote {
                Some(quote) => byte == quote,
                None => {
                    byte.is_ascii_whitespace()
                        || matches!(byte, b',' | b';' | b'&' | b')' | b']' | b'}' | b'?' | b'#')
                }
            };
            if ends {
                break;
            }
            cursor += 1;
        }
        if cursor > value_start {
            out.push_str(&text[copied..value_start]);
            out.push_str("[redacted]");
            copied = cursor;
            index = cursor;
        } else {
            index = value_start;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// The byte length of the character at `index`, or one for a broken index.
fn next_char_len(text: &str, index: usize) -> usize {
    text.get(index..)
        .and_then(|rest| rest.chars().next())
        .map(char::len_utf8)
        .unwrap_or(1)
}

/// Write the crash a panic just caused, for the next launch to offer.
pub fn write_pending(data_dir: &Path, details: &CrashDetails) -> std::io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let bytes = serde_json::to_vec(&details.clone().normalized())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(data_dir.join(PENDING_FILE), bytes)
}

/// Take the crash left by an earlier panic. The file is removed even when it
/// cannot be read, so a corrupt record cannot block the app forever.
pub fn take_pending(data_dir: &Path) -> Option<CrashDetails> {
    let path = data_dir.join(PENDING_FILE);
    let bytes = fs::read(&path).ok()?;
    let _ = fs::remove_file(&path);
    serde_json::from_slice::<CrashDetails>(&bytes)
        .ok()
        .map(CrashDetails::normalized)
}

/// Forget a recorded crash after a clean exit. A panic caught by a worker does
/// not kill the app, so its record must not greet the next launch.
pub fn clear_pending(data_dir: &Path) {
    let _ = fs::remove_file(data_dir.join(PENDING_FILE));
}

/// The desktop app data folder Tauri uses, available before the app is built so
/// a panic in the builder is recorded too. Mirrors `app.path().app_data_dir()`
/// on desktop, where the config sets no directory override.
pub fn default_data_dir(identifier: &str) -> Option<PathBuf> {
    dirs::data_dir().map(|directory| directory.join(identifier))
}

/// A message from a panic payload, or a readable fallback.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "CodingPad panicked with a message it could not read.".to_owned()
    }
}

/// Save a fatal panic for the next launch, then let the normal hook print it.
/// Panics on worker threads are usually caught by the runtime and the app keeps
/// running, so they are not recorded as crashes.
pub fn install_panic_hook(data_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            let location = info
                .location()
                .map(|place| format!("{}:{}:{}", place.file(), place.line(), place.column()));
            let details = CrashDetails::new(
                panic_message(info.payload()),
                location,
                Some(std::backtrace::Backtrace::force_capture().to_string()),
            );
            if let Err(error) = write_pending(&data_dir, &details) {
                eprintln!("CodingPad could not save the crash details: {error}");
            }
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_environment() -> CrashEnvironment {
        CrashEnvironment {
            version: "0.1.0".into(),
            platform: "linux".into(),
            arch: "x86_64".into(),
        }
    }

    fn query(report: &CrashReport) -> std::collections::HashMap<String, String> {
        url::Url::parse(&report.url)
            .unwrap()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    #[test]
    fn a_report_holds_the_version_platform_and_error_only() {
        let details =
            CrashDetails::new("index out of bounds", Some("src/main.rs:42:9".into()), None);
        let report = report(&details, &linux_environment());
        assert!(report.url.starts_with(NEW_ISSUE_URL));
        let query = query(&report);
        assert_eq!(query["labels"], "bug");
        assert!(query["title"].starts_with("Crash report: index out of bounds"));
        let body = &query["body"];
        assert!(body.contains("index out of bounds"));
        assert!(body.contains("src/main.rs:42:9"));
        assert!(body.contains("- CodingPad 0.1.0"));
        assert!(body.contains("- linux (x86_64)"));
        assert!(body.contains("sent nothing automatically"));
    }

    #[test]
    fn a_report_never_carries_a_script_or_a_secret() {
        // The builder only sees the details it is given. A secret that lives in
        // a script, a connection or the vault is nowhere in its input.
        let details = CrashDetails::new("the window could not be created", None, None);
        let report = report(&details, &linux_environment());
        let everything = format!("{}\n{}", report.title, report.body);
        for absent in [
            "def main(",
            "connection=northwind",
            "password",
            "secret(",
            "SELECT * FROM",
        ] {
            assert!(
                !everything.contains(absent),
                "{absent} leaked into {everything}"
            );
        }
    }

    #[test]
    fn long_details_are_cut_and_the_title_stays_one_line() {
        let message = format!("first line\n{}", "x".repeat(5_000));
        let details = CrashDetails::new(
            message,
            Some(format!("{}:1:1", "y".repeat(1_000))),
            Some("z".repeat(9_000)),
        );
        let report = report(&details, &linux_environment());
        assert_eq!(report.title, "Crash report: first line");
        assert!(report.title.chars().count() <= MAX_TITLE + "Crash report: ".len());
        assert!(report.body.contains('…'));
        assert!(report.body.chars().count() < 9_000);
    }

    #[test]
    fn a_report_stays_within_a_usable_link() {
        let details = CrashDetails::new("boom", Some("a.rs:1:1".into()), Some("z\n".repeat(3_000)));
        let report = report(&details, &linux_environment());
        assert!(report.url.len() <= MAX_URL);
        assert!(report.body.contains("boom"));
        assert!(report.body.contains("a.rs:1:1"));
        assert!(!report.body.contains("z\nz\n"));
    }

    #[test]
    fn the_home_folder_and_credentials_never_reach_the_issue() {
        let details = CrashDetails::new(
            "connect postgres://admin:hunter2@db.local/shop failed at /home/ada/apps/src/pool.rs:9",
            None,
            None,
        );
        let report = report_with_home(&details, &linux_environment(), Some(Path::new("/home/ada")));
        assert!(!report.body.contains("/home/ada"));
        assert!(report.body.contains("~/apps/src/pool.rs:9"));
        assert!(!report.body.contains("hunter2"));
        assert!(report.body.contains("postgres://[redacted]@db.local/shop"));
    }

    #[test]
    fn credential_assignments_lose_their_values() {
        let details = CrashDetails::new(
            "connect failed: Server=db;Password=hunter2;User Id=sa token: abc123",
            None,
            None,
        );
        let report = report(&details, &linux_environment());
        assert!(!report.body.contains("hunter2"));
        assert!(!report.body.contains("abc123"));
        assert!(report.body.contains("Password=[redacted]"));
        assert!(report.body.contains("token: [redacted]"));
    }

    #[test]
    fn a_backtrace_stays_on_the_local_crash_screen() {
        let details = CrashDetails::new(
            "boom",
            None,
            Some("at /home/ada/.cargo/registry/src/crash.rs:1:1".into()),
        );
        let report = report_with_home(&details, &linux_environment(), Some(Path::new("/home/ada")));
        assert!(!report.body.contains("Backtrace"));
        assert!(!report.body.contains(".cargo/registry"));
    }

    #[test]
    fn a_non_ascii_message_is_shortened_to_fit() {
        let details = CrashDetails::new("é".repeat(2_000), None, None);
        let report = report(&details, &linux_environment());
        assert!(report.url.len() <= MAX_URL);
        assert!(report.body.contains('é'));
    }

    #[test]
    fn an_empty_message_still_reads_sensibly() {
        let report = report(&CrashDetails::default(), &linux_environment());
        assert!(report.title.contains("unexpected error"));
        assert!(report.body.contains("unexpected error"));
    }

    #[test]
    fn backticks_in_the_error_cannot_break_the_code_block() {
        let details = CrashDetails::new("a ``` fence", None, None);
        let report = report(&details, &linux_environment());
        assert!(report.body.contains("````\na ``` fence\n````"));
    }

    #[test]
    fn a_pending_crash_is_taken_once() {
        let directory = tempfile::tempdir().unwrap();
        let details = CrashDetails::new("boom", Some("a.rs:1:1".into()), None);
        write_pending(directory.path(), &details).unwrap();
        assert_eq!(take_pending(directory.path()), Some(details));
        assert_eq!(take_pending(directory.path()), None);
    }

    #[test]
    fn a_corrupt_pending_crash_is_discarded() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(PENDING_FILE), b"not json").unwrap();
        assert_eq!(take_pending(directory.path()), None);
        assert!(!directory.path().join(PENDING_FILE).exists());
    }

    #[test]
    fn panic_payloads_become_readable_messages() {
        let text = "plain message";
        assert_eq!(panic_message(&text), "plain message");
        let owned = String::from("owned message");
        assert_eq!(panic_message(&owned), "owned message");
        assert!(panic_message(&42_u8).contains("could not read"));
    }

    #[test]
    fn the_environment_line_names_the_platform_and_arch() {
        let expected = format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH);
        assert_eq!(super::environment("0.1.0").platform_line(), expected);
    }
}
