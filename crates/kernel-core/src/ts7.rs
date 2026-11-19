//! Locate the TypeScript 7 native language server (`tsgo`).
//!
//! The native preview ships as `@typescript/native-preview`. Its `tsgo` entry
//! point is a JavaScript shim with a `node` shebang, so a machine with Bun but
//! no Node cannot execute it. The shim resolves to a platform package such as
//! `@typescript/native-preview-linux-x64`; we prefer that native binary and
//! fall back to running the shim with Bun when the package layout is unknown.
//! On Windows the extensionless `tsgo` npm shim is a POSIX script, not a Win32
//! executable, so only `.exe`, `.cmd` and `.ps1` entries are candidates.
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use tokio::process::Command;

const EXECUTABLE: &str = "tsgo";

pub(crate) struct Ts7 {
    program: PathBuf,
    prefix: Vec<OsString>,
}

impl Ts7 {
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.prefix);
        command.args(["--lsp", "--stdio"]);
        command
    }
}

/// Resolve the language server, preferring a path the user chose explicitly.
pub(crate) fn find(data_dir: Option<&Path>) -> Result<Ts7, String> {
    find_with(data_dir, std::env::var_os("CODINGPAD_TS_LSP"))
}

fn find_with(data_dir: Option<&Path>, explicit: Option<OsString>) -> Result<Ts7, String> {
    let bun = crate::toolchains::bun_for(None, data_dir);
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.push(PathBuf::from(path));
    }
    if let Some(data_dir) = data_dir {
        candidates.extend(server_files(&data_dir.join("typescript")));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates
            .extend(std::env::split_paths(&path).flat_map(|directory| server_files(&directory)));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.extend(server_files(&home.join(".bun/bin")));
    }
    for candidate in candidates {
        if let Some(server) = resolve(&candidate, &bun) {
            return Ok(server);
        }
    }
    Err("The TypeScript 7 language server was not found. Install it with `bun add -g @typescript/native-preview`, or set CODINGPAD_TS_LSP to the tsgo executable.".to_owned())
}

fn server_files(directory: &Path) -> Vec<PathBuf> {
    server_files_for(directory, cfg!(windows))
}

fn server_files_for(directory: &Path, windows: bool) -> Vec<PathBuf> {
    if directory.is_file() {
        return vec![directory.to_path_buf()];
    }
    if windows {
        vec![
            directory.join(format!("{EXECUTABLE}.exe")),
            directory.join(format!("{EXECUTABLE}.cmd")),
            directory.join(format!("{EXECUTABLE}.ps1")),
        ]
    } else {
        vec![directory.join(EXECUTABLE)]
    }
}

fn resolve(candidate: &Path, bun: &Path) -> Option<Ts7> {
    if !candidate.is_file() {
        return None;
    }
    if is_windows_shim(candidate) {
        let shim = npm_shim_target(candidate)?;
        if let Some(native) = native_binary_for_js(&shim) {
            return Some(Ts7 {
                program: native,
                prefix: Vec::new(),
            });
        }
        return Some(Ts7 {
            program: bun.to_path_buf(),
            prefix: vec![shim.into_os_string()],
        });
    }
    if is_js_shim(candidate) {
        if let Some(native) = native_binary_for_js(candidate) {
            return Some(Ts7 {
                program: native,
                prefix: Vec::new(),
            });
        }
        return Some(Ts7 {
            program: bun.to_path_buf(),
            prefix: vec![candidate.as_os_str().to_owned()],
        });
    }
    Some(Ts7 {
        program: candidate.to_path_buf(),
        prefix: Vec::new(),
    })
}

fn is_windows_shim(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("ps1")
        })
}

fn is_js_shim(path: &Path) -> bool {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("js")
                || extension.eq_ignore_ascii_case("mjs")
                || extension.eq_ignore_ascii_case("cjs")
        })
    {
        return true;
    }
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut first = String::new();
    BufReader::new(file).read_line(&mut first).is_ok()
        && first.starts_with("#!")
        && first.contains("node")
}

/// Find the JavaScript entry point behind a Windows `.cmd`/`.ps1` npm shim.
fn npm_shim_target(candidate: &Path) -> Option<PathBuf> {
    let directory = candidate.parent()?;
    let mut modules = Vec::new();
    if directory.ends_with(".bin") {
        modules.push(directory.parent()?.to_path_buf());
    } else {
        modules.push(directory.join("node_modules"));
        if let Some(parent) = directory.parent() {
            modules.push(parent.join("install/global/node_modules"));
            modules.push(parent.join("node_modules"));
        }
    }
    for modules in modules {
        for entry in [
            "@typescript/native-preview/bin/tsgo",
            "@typescript/native-preview/lib/tsgo.js",
        ] {
            let path = modules.join(entry);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Resolve `.../@typescript/native-preview/bin/tsgo` to the platform package's
/// native executable.
fn native_binary_for_js(shim: &Path) -> Option<PathBuf> {
    let canonical = fs::canonicalize(shim).ok()?;
    let package_root = canonical.parent()?.parent()?;
    let platform = platform_package()?;
    let executable = package_root
        .parent()?
        .join(platform)
        .join("lib")
        .join(if cfg!(windows) { "tsgo.exe" } else { "tsgo" });
    executable.is_file().then(|| without_verbatim(executable))
}

/// `fs::canonicalize` returns an extended-length `\\?\` path on Windows.
/// `CreateProcess` accepts it, but keeping the ordinary path makes log and test
/// output readable. Non-Windows paths pass through unchanged.
fn without_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

fn platform_package() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "native-preview-linux-x64",
        ("linux", "aarch64") => "native-preview-linux-arm64",
        ("macos", "x86_64") => "native-preview-darwin-x64",
        ("macos", "aarch64") => "native-preview-darwin-arm64",
        ("windows", "x86_64") => "native-preview-win32-x64",
        ("windows", "aarch64") => "native-preview-win32-arm64",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_name() -> &'static str {
        if cfg!(windows) {
            "tsgo.exe"
        } else {
            "tsgo"
        }
    }

    #[test]
    fn finds_a_server_in_the_private_data_folder() {
        let data = tempfile::tempdir().unwrap();
        let directory = data.path().join("typescript");
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join(native_name());
        fs::write(&executable, "").unwrap();
        let server = find_with(Some(data.path()), None).unwrap();
        assert_eq!(server.program, executable);
    }

    #[test]
    fn windows_candidates_skip_the_posix_shim() {
        let files = server_files_for(Path::new("bin"), true);
        assert_eq!(
            files,
            [
                PathBuf::from("bin/tsgo.exe"),
                PathBuf::from("bin/tsgo.cmd"),
                PathBuf::from("bin/tsgo.ps1"),
            ]
        );
        assert!(!files.contains(&PathBuf::from("bin/tsgo")));
    }

    #[test]
    fn resolves_a_javascript_shim_to_the_native_binary() {
        let Some(platform) = platform_package() else {
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let package = directory
            .path()
            .join("node_modules/@typescript/native-preview");
        let shim = package.join("bin/tsgo");
        fs::create_dir_all(shim.parent().unwrap()).unwrap();
        fs::write(&shim, "#!/usr/bin/env node\n").unwrap();
        let native = directory
            .path()
            .join("node_modules/@typescript")
            .join(platform)
            .join("lib")
            .join(native_name());
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        fs::write(&native, "").unwrap();

        let server = resolve(&shim, Path::new("bun")).unwrap();
        assert_eq!(
            fs::canonicalize(&server.program).unwrap(),
            fs::canonicalize(&native).unwrap()
        );
        assert!(server.prefix.is_empty());
    }

    #[test]
    fn a_javascript_shim_without_a_native_package_runs_on_the_resolved_bun() {
        let directory = tempfile::tempdir().unwrap();
        let shim = directory
            .path()
            .join("node_modules/@typescript/native-preview/bin/tsgo");
        fs::create_dir_all(shim.parent().unwrap()).unwrap();
        fs::write(&shim, "#!/usr/bin/env node\n").unwrap();
        let bun = Path::new("/app/toolchains/bun/1.4.2/bun");
        let server = resolve(&shim, bun).unwrap();
        assert_eq!(server.program, bun);
        assert_eq!(server.prefix, [shim.into_os_string()]);
    }

    #[test]
    fn commands_the_server_with_lsp_over_stdio() {
        let server = Ts7 {
            program: PathBuf::from("tsgo"),
            prefix: Vec::new(),
        };
        let command = server.command();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--lsp", "--stdio"]);
    }
}
