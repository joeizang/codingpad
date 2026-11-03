//! Toolchain release manifests and app-private installation lifecycle.
use crate::{
    find_uv,
    installer::{self, Archive, Artifact},
};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

pub const UV_VERSION: &str = "0.13.6";

// Pinned official .NET install script from dotnet/install-scripts tag
// v2026.07.21. The tag is immutable, so the URLs and digests never change under
// us. Update the tag, both digests, and README together.
#[cfg(test)]
const DOTNET_SCRIPTS_COMMIT: &str = "da3ce11ba63f3dbb0fb835d41bda2665d5c48e84";
const DOTNET_INSTALL_SH: (&str, &str) = (
    "https://raw.githubusercontent.com/dotnet/install-scripts/da3ce11ba63f3dbb0fb835d41bda2665d5c48e84/src/dotnet-install.sh",
    "082f7685e156738a1b2e2ed8381a621870d4ce8e8c59278034556f05c186eb2e",
);
const DOTNET_INSTALL_PS1: (&str, &str) = (
    "https://raw.githubusercontent.com/dotnet/install-scripts/da3ce11ba63f3dbb0fb835d41bda2665d5c48e84/src/dotnet-install.ps1",
    "3bb07bc8025211836c1e4f9d3f6a044e55b1fb6eec518a6c78851d04e210442b",
);
// D50/D58: install the current long-term-support release. The SDK includes the
// runtime the C# kernel needs.
const DOTNET_CHANNEL: &str = "LTS";
const DOTNET_SCRIPT_MAX_BYTES: u64 = 2 * 1024 * 1024;

// SHA-256 digests from https://github.com/astral-sh/uv/releases/tag/0.13.6
// Update UV_VERSION, all asset digests, and README together.
const RELEASES: &[(&str, &str)] = &[
    (
        "uv-aarch64-apple-darwin.tar.gz",
        "60fbd5c8a1ed681845eea28d9d622fcde03c51e91a43465b64a1e17db1ed3472",
    ),
    (
        "uv-aarch64-pc-windows-msvc.zip",
        "c2426c0c8e28b53d8c440e47a18d96ad4db51e159866c0e81435b196d7f0081c",
    ),
    (
        "uv-aarch64-unknown-linux-gnu.tar.gz",
        "229e194255c7958d86d56e8790d969a6f74736592133f0dfd0076f754c642c1d",
    ),
    (
        "uv-aarch64-unknown-linux-musl.tar.gz",
        "819692e404d23e5aae6bc44c0e260f1f5492fd1910ab4693b2c650f926dbe800",
    ),
    (
        "uv-x86_64-apple-darwin.tar.gz",
        "63ba17fdda55ad1a230d4b37fc6017ffa10c23688c83818bd3bfdf914c652a02",
    ),
    (
        "uv-x86_64-pc-windows-msvc.zip",
        "a2a8820d5eea32f08ef467ef3b7f66883a9b076d5737a2fc2fbb5f458d1bb50f",
    ),
    (
        "uv-x86_64-unknown-linux-gnu.tar.gz",
        "f29c3e94ffabd7e6f3ba521dba169a9b7ec12100bf3257087f0f24f6123f3645",
    ),
    (
        "uv-x86_64-unknown-linux-musl.tar.gz",
        "5d61f26b6c7213779eda90edc6971ea6b0b796bfb8d49a574c8ad69b59838548",
    ),
];

fn release(os: &str, arch: &str, musl: bool) -> Result<(&'static str, &'static str), String> {
    let platform = match os {
        "macos" => "apple-darwin",
        "linux" if musl => "unknown-linux-musl",
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        _ => return Err(format!("Private uv installation is not supported on {os}")),
    };
    let extension = if os == "windows" { "zip" } else { "tar.gz" };
    let name = format!("uv-{arch}-{platform}.{extension}");
    RELEASES.iter().copied().find(|(asset, _)| *asset == name)
        .ok_or_else(|| format!("Private uv installation is not supported on {os}/{arch}. Install uv manually and try Run again."))
}

pub fn private_uv(data_dir: &Path) -> PathBuf {
    data_dir
        .join("toolchains")
        .join("uv")
        .join(UV_VERSION)
        .join(if cfg!(windows) { "uv.exe" } else { "uv" })
}

pub fn resolve_uv(data_dir: &Path) -> PathBuf {
    let user = find_uv();
    prefer_user(user, private_uv(data_dir))
}

fn prefer_user(user: PathBuf, private: PathBuf) -> PathBuf {
    if user.is_file() || !private.is_file() {
        user
    } else {
        private
    }
}

pub fn private_dotnet(data_dir: &Path) -> PathBuf {
    data_dir
        .join("toolchains")
        .join("dotnet")
        .join(DOTNET_CHANNEL.to_lowercase())
        .join(if cfg!(windows) {
            "dotnet.exe"
        } else {
            "dotnet"
        })
}

fn dotnet_name() -> &'static str {
    if cfg!(windows) {
        "dotnet.exe"
    } else {
        "dotnet"
    }
}

/// The user's installations first, then CodingPad's private copy. Scripts run on
/// the newest supported runtime, not merely the first executable on PATH.
pub fn dotnet_candidates(data_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<_> = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(dotnet_name()))
                .collect()
        })
        .unwrap_or_default();
    for variable in ["DOTNET_ROOT", "DOTNET_ROOT_X64", "DOTNET_ROOT_ARM64"] {
        if let Some(path) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(path).join(dotnet_name()));
        }
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".dotnet").join(dotnet_name()));
    }
    if cfg!(unix) {
        candidates.extend(
            [
                "/usr/local/share/dotnet",
                "/usr/share/dotnet",
                "/usr/lib/dotnet",
            ]
            .map(|directory| PathBuf::from(directory).join(dotnet_name())),
        );
    }
    #[cfg(windows)]
    if let Some(programs) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(programs).join("dotnet").join(dotnet_name()));
    }
    if let Some(data_dir) = data_dir {
        candidates.push(private_dotnet(data_dir));
    }
    candidates.push(PathBuf::from(dotnet_name()));
    candidates.dedup();
    candidates
}

/// A stable .NET 10+ version, preferring the newest installed release.
fn supported_version(version: &str) -> Option<(Vec<u32>, String)> {
    if version.contains('-') {
        return None;
    }
    let numbers: Vec<u32> = version
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    (numbers.len() == 3 && numbers[0] >= 10).then(|| (numbers, version.to_owned()))
}

fn newest_version(versions: impl Iterator<Item = String>) -> Option<String> {
    versions
        .filter_map(|version| supported_version(&version))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, version)| version)
}

/// The newest supported runtime from `dotnet --list-runtimes`.
pub(crate) fn newest_runtime_version(output: &str) -> Option<String> {
    newest_version(output.lines().filter_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != "Microsoft.NETCore.App" {
            return None;
        }
        Some(parts.next()?.to_owned())
    }))
}

/// The newest supported SDK from `dotnet --list-sdks`, whose lines start with
/// the version followed by the install root.
pub(crate) fn newest_sdk_version(output: &str) -> Option<String> {
    newest_version(
        output
            .lines()
            .filter_map(|line| line.split_whitespace().next().map(str::to_owned)),
    )
}

async fn list_dotnet(executable: &Path, argument: &str) -> Result<String, String> {
    let mut command = tokio::process::Command::new(executable);
    command.arg(argument).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = command.output().await.map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn has_supported_runtime(executable: &Path) -> bool {
    list_dotnet(executable, "--list-runtimes")
        .await
        .ok()
        .and_then(|output| newest_runtime_version(&output))
        .is_some()
}

/// Resolve the `dotnet` host to use. CodingPad prefers an installed .NET 10+
/// runtime and falls back to its private copy; a bare `dotnet` keeps the
/// existing actionable "not found" message when nothing is installed.
pub async fn resolve_dotnet(data_dir: Option<&Path>) -> PathBuf {
    let candidates = dotnet_candidates(data_dir);
    for candidate in &candidates {
        if candidate.is_file() && has_supported_runtime(candidate).await {
            return candidate.clone();
        }
    }
    if let Some(data_dir) = data_dir {
        let private = private_dotnet(data_dir);
        if private.is_file() {
            return private;
        }
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(dotnet_name()))
}

async fn find_installed_sdk(data_dir: &Path) -> Option<PathBuf> {
    for candidate in dotnet_candidates(Some(data_dir)) {
        if !candidate.is_file() {
            continue;
        }
        if list_dotnet(&candidate, "--list-sdks")
            .await
            .ok()
            .and_then(|output| newest_sdk_version(&output))
            .is_some()
        {
            return Some(candidate);
        }
    }
    None
}

/// One installer per application; simultaneous clicks share its completed result.
pub struct Toolchains {
    data_dir: PathBuf,
    install: Mutex<()>,
}

impl Toolchains {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            install: Mutex::new(()),
        }
    }

    pub async fn install_uv(&self, progress: impl FnMut(&str) + Send) -> Result<PathBuf, String> {
        let _guard = self.install.lock().await;
        let existing = resolve_uv(&self.data_dir);
        if existing.is_file() {
            return Ok(existing);
        }
        install_private_uv(&self.data_dir, progress).await
    }

    pub async fn install_dotnet(
        &self,
        progress: impl FnMut(&str) + Send,
    ) -> Result<PathBuf, String> {
        let _guard = self.install.lock().await;
        if let Some(installed) = find_installed_sdk(&self.data_dir).await {
            return Ok(installed);
        }
        install_private_dotnet(&self.data_dir, progress).await
    }
}

/// Releases the installer lock explicitly on drop, so a duplicate or inherited
/// descriptor cannot keep it held after the install finishes.
struct InstallLock(std::fs::File);

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

// The OS releases this lock after a crash. Keep the lock file in place: deleting
// it would let different processes lock different inodes and sweep active files.
fn prepare_install(directory: &Path) -> Result<InstallLock, String> {
    std::fs::create_dir_all(directory)
        .map_err(|e| format!("Could not create the tool folder: {e}"))?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".install.lock"))
        .map_err(|e| format!("Could not open the tool installer lock: {e}"))?;
    lock.try_lock().map_err(|e| format!("Could not lock the tool installer. Another CodingPad instance may be installing a toolchain; retry when it finishes: {e}"))?;
    // Guard immediately so a failed cleanup below still unlocks explicitly.
    let lock = InstallLock(lock);
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Include the original tempfile prefix for interrupted older installs.
        if name.starts_with(".codingpad-install-") || name.starts_with(".tmp") {
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_file() || kind.is_symlink() {
                std::fs::remove_file(entry.path())
                    .map_err(|e| format!("Could not remove an interrupted tool download: {e}"))?;
            }
        }
    }
    Ok(lock)
}

async fn install_private_uv(
    data_dir: &Path,
    mut progress: impl FnMut(&str) + Send,
) -> Result<PathBuf, String> {
    let destination = private_uv(data_dir);
    let _lock = prepare_install(destination.parent().ok_or("Missing tool directory")?)?;
    let (asset, sha256) = release(
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_env = "musl"),
    )?;
    let windows = asset.ends_with(".zip");
    let member = if windows {
        "uv.exe".to_owned()
    } else {
        format!("{}/uv", asset.trim_end_matches(".tar.gz"))
    };
    let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{asset}");
    progress("Downloading and verifying uv…");
    let path = installer::install_binary(
        &Artifact {
            url: &url,
            sha256,
            max_bytes: 100 * 1024 * 1024,
        },
        if windows {
            Archive::Zip
        } else {
            Archive::TarGz
        },
        &member,
        &destination,
    )
    .await?;
    progress("uv installed. Starting script…");
    Ok(path)
}

async fn install_private_dotnet(
    data_dir: &Path,
    mut progress: impl FnMut(&str) + Send,
) -> Result<PathBuf, String> {
    let destination = private_dotnet(data_dir);
    let directory = destination.parent().ok_or("Missing tool directory")?;
    let _lock = prepare_install(directory)?;
    let (url, sha256, suffix) = if cfg!(windows) {
        (DOTNET_INSTALL_PS1.0, DOTNET_INSTALL_PS1.1, ".ps1")
    } else {
        (DOTNET_INSTALL_SH.0, DOTNET_INSTALL_SH.1, ".sh")
    };
    progress("Downloading and verifying the .NET install script…");
    let script = installer::download_verified_with_suffix(
        &Artifact {
            url,
            sha256,
            max_bytes: DOTNET_SCRIPT_MAX_BYTES,
        },
        directory,
        suffix,
    )
    .await?;
    progress("Installing the .NET long-term-support SDK…");
    let installed = run_dotnet_install_script(script.path(), directory).await?;
    if !installed {
        return Err(
            "The .NET install script did not finish. Check your connection and retry.".into(),
        );
    }
    let version = list_dotnet(&destination, "--list-sdks")
        .await
        .ok()
        .and_then(|output| newest_sdk_version(&output))
        .ok_or("The .NET SDK was installed but could not be started. Retry installation.")?;
    progress(&format!(".NET SDK {version} installed. Starting script…"));
    Ok(destination)
}

async fn run_dotnet_install_script(script: &Path, directory: &Path) -> Result<bool, String> {
    // The downloaded script is the only executable content: run it through the
    // platform shell so no archive extraction or PATH changes are involved.
    #[cfg(windows)]
    let mut command = {
        let mut command = tokio::process::Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(script)
            .arg("-Channel")
            .arg(DOTNET_CHANNEL)
            .arg("-InstallDir")
            .arg(directory)
            .arg("-NoPath");
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .arg(script)
            .arg("--channel")
            .arg(DOTNET_CHANNEL)
            .arg("--install-dir")
            .arg(directory)
            .arg("--no-path");
        command
    };
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = command
        .output()
        .await
        .map_err(|e| format!("Could not start the .NET install script: {e}"))?;
    if output.status.success() {
        return Ok(true);
    }
    let message = String::from_utf8_lossy(&output.stderr);
    let message = message.trim();
    Err(format!(
        "The .NET install script failed. {}",
        if message.is_empty() {
            "Check your connection and retry."
        } else {
            message
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_have_pinned_artifacts() {
        for os in ["linux", "macos", "windows"] {
            for arch in ["x86_64", "aarch64"] {
                let (asset, hash) = release(os, arch, false).unwrap();
                assert!(asset.contains(arch));
                assert_eq!(hash.len(), 64);
                assert!(hash.bytes().all(|c| c.is_ascii_hexdigit()));
            }
        }
        assert!(release("linux", "aarch64", true)
            .unwrap()
            .0
            .contains("musl"));
        assert!(release("windows", "mips", false).is_err());
        assert!(release("unknown", "x86_64", false).is_err());
    }

    #[test]
    fn user_tool_wins_and_private_install_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user-uv");
        let private = private_uv(directory.path());
        assert_eq!(prefer_user(user.clone(), private.clone()), user);
        std::fs::create_dir_all(private.parent().unwrap()).unwrap();
        std::fs::write(&private, "private").unwrap();
        assert_eq!(prefer_user(user.clone(), private.clone()), private);
        std::fs::write(&user, "user").unwrap();
        assert_eq!(prefer_user(user.clone(), private), user);
    }

    #[test]
    fn dotnet_scripts_are_pinned_official_https_assets() {
        for (url, hash) in [DOTNET_INSTALL_SH, DOTNET_INSTALL_PS1] {
            assert!(url.starts_with("https://raw.githubusercontent.com/dotnet/install-scripts/"));
            assert!(url.contains(DOTNET_SCRIPTS_COMMIT));
            assert_eq!(hash.len(), 64);
            assert!(hash.bytes().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn private_dotnet_lives_under_the_app_data_folder() {
        let directory = tempfile::tempdir().unwrap();
        let path = private_dotnet(directory.path());
        assert!(path.starts_with(directory.path().join("toolchains").join("dotnet")));
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            if cfg!(windows) {
                "dotnet.exe"
            } else {
                "dotnet"
            }
        );
    }

    #[test]
    fn selects_the_newest_supported_sdk_and_runtime() {
        let sdks = "10.0.100 [/usr/share/dotnet/sdk]\n10.0.402 [/usr/share/dotnet/sdk]\n9.0.201 [/usr/share/dotnet/sdk]\n12.0.100-preview.1 [/usr/share/dotnet/sdk]\n";
        assert_eq!(newest_sdk_version(sdks), Some("10.0.402".into()));
        let runtimes = "Microsoft.NETCore.App 9.0.9 [/a]\nMicrosoft.NETCore.App 10.0.9 [/b]\nMicrosoft.NETCore.App 10.0.14 [/b]\nMicrosoft.AspNetCore.App 12.0.0 [/b]\n";
        assert_eq!(newest_runtime_version(runtimes), Some("10.0.14".into()));
        assert_eq!(newest_sdk_version("Microsoft.NET.Sdk 9.0.201 [/a]"), None);
        assert_eq!(newest_runtime_version("not dotnet output"), None);
    }

    #[test]
    fn dotnet_candidates_prefer_user_locations_and_end_with_a_private_copy() {
        let directory = tempfile::tempdir().unwrap();
        let candidates = dotnet_candidates(Some(directory.path()));
        let private = private_dotnet(directory.path());
        assert!(candidates.contains(&private));
        assert_eq!(candidates.last(), Some(&PathBuf::from(dotnet_name())));
        assert!(
            candidates
                .iter()
                .position(|candidate| candidate == &private)
                < candidates
                    .iter()
                    .position(|candidate| candidate == &PathBuf::from(dotnet_name()))
        );
    }

    #[test]
    fn cleans_interrupted_installs_without_touching_active_or_unrelated_files() {
        let data = tempfile::tempdir().unwrap();
        let directory = data.path();
        for name in ["uv", ".tmp-old", ".codingpad-install-old", "notes"] {
            std::fs::write(directory.join(name), name).unwrap();
        }
        std::fs::create_dir(directory.join(".tmp-directory")).unwrap();
        let lock = prepare_install(directory).unwrap();
        assert!(!directory.join(".tmp-old").exists());
        assert!(!directory.join(".codingpad-install-old").exists());
        assert!(directory.join("uv").exists());
        assert!(directory.join("notes").exists());
        assert!(directory.join(".tmp-directory").is_dir());
        let active = directory.join(".codingpad-install-active");
        std::fs::write(&active, "active download").unwrap();
        assert!(prepare_install(directory).is_err());
        assert!(active.exists());
        // Model a descriptor inherited by a concurrently spawned subprocess.
        // The guard's Drop unlocks explicitly, so closing our handle is enough
        // even while the duplicate descriptor is still open.
        #[cfg(unix)]
        let inherited = lock.0.try_clone().unwrap();
        drop(lock);
        let _lock = prepare_install(directory).unwrap();
        #[cfg(unix)]
        drop(inherited);
        assert!(!active.exists());
    }

    /// Network acceptance test, separate from the offline default test suite.
    #[tokio::test]
    #[ignore = "downloads the official uv release; run explicitly when validating the installer"]
    async fn official_release_installs_privately_and_runs_python() {
        let data = tempfile::tempdir().unwrap();
        // A rejected download leaves no executable or staging file, and a
        // subsequent valid install can retry in the same app data directory.
        let (asset, _) = release(
            std::env::consts::OS,
            std::env::consts::ARCH,
            cfg!(target_env = "musl"),
        )
        .unwrap();
        let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{asset}");
        let rejected = installer::download_verified(
            &Artifact {
                url: &url,
                sha256: &"0".repeat(64),
                max_bytes: 100 * 1024 * 1024,
            },
            data.path(),
        )
        .await
        .unwrap_err();
        assert!(rejected.contains("SHA-256 verification"));
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
        // Bypass user-tool discovery so the private installer is exercised even
        // on a developer machine with uv already installed.
        let installed = install_private_uv(data.path(), |_| {}).await.unwrap();
        assert_eq!(installed, private_uv(data.path()));
        let supervisor = crate::Supervisor::with_uv(installed.clone());
        let mut events = Vec::new();
        assert!(
            supervisor
                .run("installed", "print('private uv works')\n6 * 7", |event| {
                    events.push(event)
                })
                .await
                .unwrap()
                .ok
        );
        assert!(events
            .iter()
            .any(|event| event.params.to_string().contains("private uv works")));
        supervisor.shutdown().await;
        assert_eq!(
            std::fs::read_dir(installed.parent().unwrap())
                .unwrap()
                .count(),
            2 // Installed executable plus the persistent process lock.
        );
    }

    /// Network acceptance test for the one-click .NET SDK install.
    #[tokio::test]
    #[ignore = "downloads the official .NET LTS SDK; run explicitly when validating the installer"]
    async fn official_dotnet_script_installs_lts_sdk_privately() {
        let data = tempfile::tempdir().unwrap();
        let installed = install_private_dotnet(data.path(), |_| {}).await.unwrap();
        assert_eq!(installed, private_dotnet(data.path()));
        let sdks = list_dotnet(&installed, "--list-sdks").await.unwrap();
        let version = newest_sdk_version(&sdks).expect("a supported SDK is installed");
        assert!(
            version.starts_with("10.")
                || version.split('.').next().unwrap().parse::<u32>().unwrap() >= 10
        );
        let runtimes = list_dotnet(&installed, "--list-runtimes").await.unwrap();
        assert!(newest_runtime_version(&runtimes).is_some());
        // The private host runs the bundled C# kernel without the user's SDK.
        let supervisor = crate::Supervisor::with_dotnet(installed.clone());
        let mut events = Vec::new();
        let result = supervisor
            .run_language(
                "dotnet",
                "Console.WriteLine(6 * 7);",
                crate::Language::Csharp,
                |event| events.push(event),
            )
            .await
            .unwrap();
        assert!(result.ok, "{events:?}");
        assert!(events
            .iter()
            .any(|event| event.params.to_string().contains("42")));
        supervisor.shutdown().await;
    }
}
