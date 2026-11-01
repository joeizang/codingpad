//! Script store: the library and pinned folders, plain native files and the
//! single-line `codingpad:` script header.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const HEADER_MARKER: &str = "codingpad:";

/// The CodingPad-only settings stored in a script's leading comment line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptHeader {
    /// The original line, unchanged, so saving never rewrites it.
    pub line: String,
    pub settings: BTreeMap<String, String>,
}

/// Parse the first line of a script as a `codingpad:` header.
///
/// The header is the top line of the file, written as a Python `#` comment or
/// a C#/Rust/TypeScript `//` comment. Settings are whitespace-separated
/// `key=value` pairs, where a value may be quoted.
pub fn parse_header(source: &str) -> Option<ScriptHeader> {
    let first = source.lines().next()?;
    let first = first.strip_prefix('\u{feff}').unwrap_or(first);
    let trimmed = first.trim_start();
    let rest = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("//"))?
        .trim_start();
    let settings_text = rest.strip_prefix(HEADER_MARKER)?;
    Some(ScriptHeader {
        line: first.to_string(),
        settings: parse_settings(settings_text),
    })
}

fn parse_settings(text: &str) -> BTreeMap<String, String> {
    let mut settings = BTreeMap::new();
    for token in tokenize(text) {
        let Some((key, value)) = token.split_once('=') else {
            if !token.is_empty() {
                settings.insert(token, String::new());
            }
            continue;
        };
        if !key.is_empty() {
            settings.insert(key.to_string(), value.to_string());
        }
    }
    settings
}

// Split on whitespace while keeping quoted values together.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote: Option<char> = None;
    for character in text.chars() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => token.push(character),
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character.is_whitespace() => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            None => token.push(character),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

/// The sidebar's stored locations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySettings {
    pub library_path: String,
    #[serde(default)]
    pub pinned_folders: Vec<String>,
}

impl LibrarySettings {
    pub fn new(library_path: impl Into<String>) -> Self {
        Self {
            library_path: library_path.into(),
            pinned_folders: Vec::new(),
        }
    }
}

/// The default library folder: `Documents/CodingPad`.
pub fn default_library_path(documents: Option<&Path>) -> PathBuf {
    match documents {
        Some(documents) => documents.join("CodingPad"),
        None => PathBuf::from("CodingPad"),
    }
}

/// Load settings, falling back to a fresh default when the file is absent or
/// unreadable. A missing library is not an error: the caller creates it.
pub fn load_settings(path: &Path, default_library: &Path) -> LibrarySettings {
    let settings = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<LibrarySettings>(&text).ok());
    let mut settings =
        settings.unwrap_or_else(|| LibrarySettings::new(default_library.to_string_lossy()));
    if settings.library_path.trim().is_empty() {
        settings.library_path = default_library.to_string_lossy().into_owned();
    }
    let mut seen = HashSet::new();
    let library_path = settings.library_path.clone();
    settings.pinned_folders.retain(|folder| {
        !folder.trim().is_empty() && folder != &library_path && seen.insert(folder.clone())
    });
    settings
}

/// Write through a temporary file in the destination folder, then rename it
/// over the original so a crash or full disk cannot leave a truncated file.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Could not create a temporary file: {error}"))?;
    temp.write_all(contents)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    temp.persist(path)
        .map_err(|error| format!("Could not save {}: {}", path.display(), error.error))?;
    Ok(())
}

fn backup_corrupt_settings(path: &Path) {
    if let Ok(bytes) = fs::read(path) {
        if serde_json::from_slice::<LibrarySettings>(&bytes).is_err() {
            let mut backup = path.as_os_str().to_os_string();
            backup.push(".bak");
            let _ = fs::copy(path, PathBuf::from(backup));
        }
    }
}

pub fn save_settings(path: &Path, settings: &LibrarySettings) -> Result<(), String> {
    if settings.library_path.trim().is_empty() {
        return Err("The library folder cannot be empty".into());
    }
    let text = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("Could not encode settings: {error}"))?;
    backup_corrupt_settings(path);
    write_atomic(path, text.as_bytes())
}

/// One directory entry for the sidebar tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// List a folder for the sidebar: directories first, then files, both sorted
/// case-insensitively. Hidden entries are skipped.
pub fn list_folder(path: &Path) -> Result<Vec<FolderEntry>, String> {
    let read = fs::read_dir(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let mut entries: Vec<FolderEntry> = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let is_dir = entry.file_type().ok()?.is_dir();
            Some(FolderEntry {
                name,
                path: entry.path().to_string_lossy().into_owned(),
                is_dir,
            })
        })
        .collect();
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}

/// A script read from disk, with its parsed header.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptFile {
    pub path: String,
    pub name: String,
    pub source: String,
    pub header: Option<ScriptHeader>,
}

fn script_file(path: &Path, source: String) -> ScriptFile {
    ScriptFile {
        path: path.to_string_lossy().into_owned(),
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        header: parse_header(&source),
        source,
    }
}

pub fn read_script(path: &Path) -> Result<ScriptFile, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    Ok(script_file(path, source))
}

/// Write a plain native file exactly as edited. The header line is part of the
/// source, so it is preserved untouched.
pub fn write_script(path: &Path, source: &str) -> Result<ScriptFile, String> {
    write_atomic(path, source.as_bytes())?;
    Ok(script_file(path, source.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_a_python_header_and_keeps_the_original_line() {
        let header = parse_header("# codingpad: connection=northwind\nprint(1)\n").unwrap();
        assert_eq!(header.line, "# codingpad: connection=northwind");
        assert_eq!(header.settings.get("connection").unwrap(), "northwind");
    }

    #[test]
    fn parses_csharp_style_headers_and_quoted_values() {
        let header = parse_header("// codingpad: connection=\"north wind\" theme=dark\n").unwrap();
        assert_eq!(header.settings.get("connection").unwrap(), "north wind");
        assert_eq!(header.settings.get("theme").unwrap(), "dark");
    }

    #[test]
    fn accepts_a_byte_order_mark_and_indentation() {
        let header = parse_header("\u{feff}  # codingpad: connection=one\n").unwrap();
        assert_eq!(header.line, "  # codingpad: connection=one");
    }

    #[test]
    fn ignores_scripts_without_a_header_on_the_first_line() {
        assert!(parse_header("print(1)\n# codingpad: connection=one\n").is_none());
        assert!(parse_header("# a normal comment\n").is_none());
        assert!(parse_header("").is_none());
    }

    #[test]
    fn keeps_keys_without_values() {
        let header = parse_header("# codingpad: readOnly").unwrap();
        assert_eq!(header.settings.get("readOnly").unwrap(), "");
    }

    #[test]
    fn round_trips_a_script_without_touching_its_header() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("example.py");
        let source = "# codingpad: connection=northwind\nprint('hi')\n";
        let written = write_script(&path, source).unwrap();
        assert_eq!(
            written.header.as_ref().unwrap().line,
            "# codingpad: connection=northwind"
        );
        let read = read_script(&path).unwrap();
        assert_eq!(read.source, source);
        assert_eq!(read.header, written.header);
        let names: Vec<_> = list_folder(directory.path())
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["example.py"]);
    }

    #[test]
    fn settings_default_when_missing_and_round_trip() {
        let directory = tempdir().unwrap();
        let config = directory.path().join("settings.json");
        let default_library = directory.path().join("CodingPad");
        let defaults = load_settings(&config, &default_library);
        assert_eq!(defaults.library_path, default_library.to_string_lossy());

        let saved = LibrarySettings {
            library_path: "/scripts".into(),
            pinned_folders: vec!["/one".into(), "/one".into(), "".into(), "/two".into()],
        };
        save_settings(&config, &saved).unwrap();
        let loaded = load_settings(&config, &default_library);
        assert_eq!(loaded.library_path, "/scripts");
        assert_eq!(loaded.pinned_folders, vec!["/one", "/two"]);
    }

    #[test]
    fn removes_pins_that_point_at_the_library() {
        let directory = tempdir().unwrap();
        let config = directory.path().join("settings.json");
        let saved = LibrarySettings {
            library_path: "/scripts".into(),
            pinned_folders: vec!["/scripts".into(), "/other".into()],
        };
        save_settings(&config, &saved).unwrap();
        let loaded = load_settings(&config, &directory.path().join("CodingPad"));
        assert_eq!(loaded.pinned_folders, vec!["/other"]);
    }

    #[test]
    fn backs_up_a_corrupt_settings_file_before_replacing_it() {
        let directory = tempdir().unwrap();
        let config = directory.path().join("settings.json");
        fs::write(&config, "{ not json").unwrap();
        save_settings(&config, &LibrarySettings::new("/scripts")).unwrap();
        let backup = format!("{}.bak", config.display());
        assert_eq!(fs::read_to_string(backup).unwrap(), "{ not json");
        assert_eq!(
            load_settings(&config, &directory.path().join("CodingPad")).library_path,
            "/scripts"
        );
    }

    #[test]
    fn lists_directories_before_files_and_skips_hidden_entries() {
        let directory = tempdir().unwrap();
        fs::create_dir(directory.path().join("zeta")).unwrap();
        fs::create_dir(directory.path().join("Alpha")).unwrap();
        fs::write(directory.path().join("b.py"), "").unwrap();
        fs::write(directory.path().join("A.py"), "").unwrap();
        fs::write(directory.path().join(".hidden"), "").unwrap();

        let entries = list_folder(directory.path()).unwrap();
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "zeta", "A.py", "b.py"]);
        assert!(entries[0].is_dir);
        assert!(!entries[2].is_dir);
    }

    #[test]
    fn empty_library_paths_are_rejected() {
        let directory = tempdir().unwrap();
        let settings = LibrarySettings::new("  ");
        assert!(save_settings(&directory.path().join("settings.json"), &settings).is_err());
    }
}
