//! Toolchain release manifests and app-private installation lifecycle.
mod rust;
pub(crate) use rust::{RustToolchain, MISSING as MISSING_RUST};

use crate::{
    find_uv,
    installer::{self, Archive, Artifact},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    sync::Mutex,
};

pub const UV_VERSION: &str = "0.13.6";

/// Pinned with the Bun version CI installs (1.4.4).
pub const BUN_VERSION: &str = "1.4.4";

// SHA-256 digests from https://github.com/oven-sh/bun/releases/tag/bun-v1.4.4
// Update BUN_VERSION, all asset digests, the CI Bun pin and TECHNICAL_REFERENCE.md together.
// macOS assets mirror the uv manifest so a future macOS build (D62) has them;
// only the current platform's asset is downloaded.
const BUN_RELEASES: &[(&str, &str)] = &[
    (
        "bun-darwin-aarch64.zip",
        "c240d7f0ccb90c263a849fcdb09b7ee62630d921348ed6aafda86b9d577bc510",
    ),
    (
        "bun-darwin-x64-baseline.zip",
        "d4a155adfbfd09155eb3ba05c9e3a544f39297d7a68616d3c7759f67ab5c47dc",
    ),
    (
        "bun-darwin-x64.zip",
        "50a3491f73abc2fe81673171a85762d3bc2cc1680ccecadd983de99adf77977e",
    ),
    (
        "bun-linux-aarch64-musl.zip",
        "57f01b02ac7113c61c935f28f38c2eab849e3c970dab21f8b640a87b1ef65009",
    ),
    (
        "bun-linux-aarch64.zip",
        "baa6bb36f51d6e58561e02bbea825038a9df454f673d6d776e8bbf86f8c7850e",
    ),
    (
        "bun-linux-x64-baseline.zip",
        "e0446eb55995772eb9894bebae9a6be368d985a8b9271ee124486f9946eedd76",
    ),
    (
        "bun-linux-x64-musl-baseline.zip",
        "6e9c37098b293920aed28bfb51ab240e26d23bfdd9bbfecb3ad428ac166549bc",
    ),
    (
        "bun-linux-x64-musl.zip",
        "a5a8ccc5e74dcb883fe974f125a7bdb3b41e5195ed1b870e73f17effd3eb2b2d",
    ),
    (
        "bun-linux-x64.zip",
        "bf8ccbfb2b0b80159e0f9a6d15c55317add8077534dbc6130a3edb52fccb395c",
    ),
    (
        "bun-windows-aarch64.zip",
        "2d57a0eec0cb44d95c918c3f9734f60455a1306ae5085d76fc68f44e3ee30d09",
    ),
    (
        "bun-windows-x64-baseline.zip",
        "0f20ce6674d4f2dbcc8c5d59fdf99f7ba45f8c3b367e0f5781cbfa30a1c78d99",
    ),
    (
        "bun-windows-x64.zip",
        "cd2c0fccc78f3ede1f50ca827a15f063eda13d4d150a3d0674f4f6e323d864a5",
    ),
];

// Pinned official .NET install script from dotnet/install-scripts tag
// v2026.07.21. The tag is immutable, so the URLs and digests never change under
// us. Update the tag, both digests, and TECHNICAL_REFERENCE.md together.
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
const DOTNET_INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

// SHA-256 digests from https://github.com/astral-sh/uv/releases/tag/0.13.6
// Update UV_VERSION, all asset digests, and TECHNICAL_REFERENCE.md together.
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

/// The user's Bun first, then CodingPad's private copy.
pub fn resolve_bun(data_dir: &Path) -> PathBuf {
    resolve_bun_with(crate::typescript::find_bun(), data_dir)
}

fn resolve_bun_with(user: PathBuf, data_dir: &Path) -> PathBuf {
    prefer_user(user, private_bun(data_dir))
}

/// The one Bun every part of the app starts: an explicit pin, the user's copy,
/// then CodingPad's private install.
pub(crate) fn bun_for(pin: Option<&Path>, data_dir: Option<&Path>) -> PathBuf {
    if let Some(pin) = pin {
        return pin.to_path_buf();
    }
    data_dir
        .map(resolve_bun)
        .unwrap_or_else(crate::typescript::find_bun)
}

pub fn private_bun(data_dir: &Path) -> PathBuf {
    data_dir
        .join("toolchains")
        .join("bun")
        .join(BUN_VERSION)
        .join(if cfg!(windows) { "bun.exe" } else { "bun" })
}

/// The official Bun build for this machine. Bun's generic x64 build requires
/// AVX2, so older CPUs get the baseline build.
fn bun_release(
    os: &str,
    arch: &str,
    musl: bool,
    avx2: bool,
) -> Result<(&'static str, &'static str), String> {
    let platform_arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        _ => return Err(unsupported_bun(os, arch)),
    };
    let mut platform = match os {
        "macos" => format!("darwin-{platform_arch}"),
        "windows" => format!("windows-{platform_arch}"),
        "linux" if musl => format!("linux-{platform_arch}-musl"),
        "linux" => format!("linux-{platform_arch}"),
        _ => return Err(unsupported_bun(os, arch)),
    };
    if platform_arch == "x64" && !avx2 {
        platform.push_str("-baseline");
    }
    let name = format!("bun-{platform}.zip");
    BUN_RELEASES
        .iter()
        .copied()
        .find(|(asset, _)| *asset == name)
        .ok_or_else(|| unsupported_bun(os, arch))
}

fn has_avx2() -> bool {
    #[cfg(any(target_arch = "x86_64", target_arch = "x86"))]
    {
        is_x86_feature_detected!("avx2")
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
    {
        true
    }
}

fn unsupported_bun(os: &str, arch: &str) -> String {
    format!(
        "Private Bun installation is not supported on {os}/{arch}. Install Bun from https://bun.com/docs/installation and try Run again."
    )
}

fn prefer_user(user: PathBuf, private: PathBuf) -> PathBuf {
    if user.is_file() || !private.is_file() {
        user
    } else {
        private
    }
}

fn private_dotnet_dir(data_dir: &Path) -> PathBuf {
    data_dir
        .join("toolchains")
        .join("dotnet")
        .join(DOTNET_CHANNEL.to_lowercase())
}

pub fn private_dotnet(data_dir: &Path) -> PathBuf {
    private_dotnet_dir(data_dir).join(dotnet_name())
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
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
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

async fn installed_sdk_version(executable: &Path) -> Option<String> {
    if !executable.is_file() {
        return None;
    }
    list_dotnet(executable, "--list-sdks")
        .await
        .ok()
        .and_then(|output| newest_sdk_version(&output))
}

async fn find_installed_sdk(data_dir: &Path) -> Option<PathBuf> {
    let candidates = dotnet_candidates(Some(data_dir));
    for candidate in candidates {
        if installed_sdk_version(&candidate).await.is_some() {
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

    pub async fn install_bun(&self, progress: impl FnMut(&str) + Send) -> Result<PathBuf, String> {
        let _guard = self.install.lock().await;
        let existing = resolve_bun(&self.data_dir);
        if existing.is_file() {
            let private = private_bun(&self.data_dir);
            // Trust the user's own Bun, but prove a private copy still runs.
            // A CPU without AVX2, a partial restore or a changed platform can
            // leave an executable that exists and cannot start.
            if existing != private || bun_runs(&existing).await {
                return Ok(existing);
            }
            std::fs::remove_file(&existing).map_err(|error| {
                format!("Could not replace the unusable Bun installation: {error}")
            })?;
        }
        install_private_bun(&self.data_dir, progress).await
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
            if name.starts_with(".codingpad-install-") && kind.is_dir() {
                // A staging directory left by a killed .NET install. The lock
                // proves no live install is using it.
                std::fs::remove_dir_all(entry.path()).map_err(|e| {
                    format!("Could not remove an interrupted tool installation: {e}")
                })?;
            } else if kind.is_file() || kind.is_symlink() {
                std::fs::remove_file(entry.path())
                    .map_err(|e| format!("Could not remove an interrupted tool download: {e}"))?;
            }
        }
    }
    Ok(lock)
}

const MAX_TOOL_BYTES: u64 = 100 * 1024 * 1024;

struct ReleaseInstall<'a> {
    destination: &'a Path,
    sha256: &'a str,
    url: String,
    member: String,
    archive: Archive,
    downloading: &'a str,
}

/// Take the installer lock, download and verify one release, then promote only
/// its binary. Shared by the uv and Bun installers.
async fn install_release(
    release: ReleaseInstall<'_>,
    progress: &mut (impl FnMut(&str) + Send),
) -> Result<PathBuf, String> {
    let _lock = prepare_install(
        release
            .destination
            .parent()
            .ok_or("Missing tool directory")?,
    )?;
    progress(release.downloading);
    installer::install_binary(
        &Artifact {
            url: &release.url,
            sha256: release.sha256,
            max_bytes: MAX_TOOL_BYTES,
        },
        release.archive,
        &release.member,
        release.destination,
    )
    .await
}

async fn install_private_uv(
    data_dir: &Path,
    mut progress: impl FnMut(&str) + Send,
) -> Result<PathBuf, String> {
    let destination = private_uv(data_dir);
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
    let path = install_release(
        ReleaseInstall {
            destination: &destination,
            sha256,
            url: format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{asset}"),
            member,
            archive: if windows {
                Archive::Zip
            } else {
                Archive::TarGz
            },
            downloading: "Downloading and verifying uv…",
        },
        &mut progress,
    )
    .await?;
    progress("uv installed. Starting script…");
    Ok(path)
}

async fn install_private_bun(
    data_dir: &Path,
    mut progress: impl FnMut(&str) + Send,
) -> Result<PathBuf, String> {
    let destination = private_bun(data_dir);
    let (asset, sha256) = bun_release(
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_env = "musl"),
        has_avx2(),
    )?;
    let member = format!(
        "{}/{}",
        asset.trim_end_matches(".zip"),
        if cfg!(windows) { "bun.exe" } else { "bun" }
    );
    let path = install_release(
        ReleaseInstall {
            destination: &destination,
            sha256,
            url: format!(
                "https://github.com/oven-sh/bun/releases/download/bun-v{BUN_VERSION}/{asset}"
            ),
            member,
            archive: Archive::Zip,
            downloading: "Downloading and verifying Bun…",
        },
        &mut progress,
    )
    .await?;
    // install_binary probes `--version` before promoting, but another CodingPad
    // instance can win the persist_noclobber race and supply the file that is
    // actually used. Run that copy and remove it if this machine cannot start
    // it, so a reinstall can retry.
    if !bun_runs(&path).await {
        let _ = std::fs::remove_file(&path);
        return Err(
            "The installed Bun could not run on this computer. Nothing was kept; retry installation."
                .into(),
        );
    }
    progress("Bun installed. Starting script…");
    Ok(path)
}

/// Strict health check for the install path: a probe that times out counts as
/// unusable so a fresh install can retry.
pub(crate) async fn bun_runs(path: &Path) -> bool {
    bun_version(path, Duration::from_secs(10)).await == Some(true)
}

/// Only a definite failure marks an installed copy broken. A probe that times
/// out may be a busy machine or an antivirus scan, so the recovery path keeps
/// the copy instead of downloading it again.
pub(crate) async fn bun_is_broken(path: &Path) -> bool {
    bun_version(path, Duration::from_secs(10)).await == Some(false)
}

/// `Some(true)` started successfully, `Some(false)` definitely failed, and
/// `None` means the probe timed out.
async fn bun_version(path: &Path, deadline: Duration) -> Option<bool> {
    if !path.is_file() {
        return Some(false);
    }
    let mut command = tokio::process::Command::new(path);
    command.arg("--version").kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    match tokio::time::timeout(deadline, command.output()).await {
        Ok(Ok(output)) => Some(output.status.success()),
        Ok(Err(_)) => Some(false),
        Err(_) => None,
    }
}

/// Keep learner-facing progress friendly: the script's diagnostic lines include
/// URLs, staging paths and CI advice that do not belong in Results.
fn friendly_dotnet_progress(line: &str) -> Option<&'static str> {
    let line = line.trim().to_ascii_lowercase();
    if line.contains("attempting to download")
        || line.contains("downloading")
        || line.contains("payload url")
        || line.contains("downloaded file size")
    {
        Some("Downloading the .NET SDK…")
    } else if line.contains("extracting") || line.contains("extraction") {
        Some("Unpacking the .NET SDK…")
    } else if line.contains("installed version")
        || line.contains("installation finished")
        || line.contains("checking installation")
    {
        Some("Finishing the .NET SDK installation…")
    } else {
        None
    }
}

#[cfg(unix)]
pub(crate) struct ProcessGroup(Option<u32>);

#[cfg(unix)]
impl ProcessGroup {
    pub(crate) fn new(pid: Option<u32>) -> Self {
        Self(pid)
    }

    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // Negative pid targets the group made by process_group(0). This
            // also stops the script's curl/wget child after a timeout or drop.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

async fn promote_directory(stage: &Path, destination: &Path, name: &str) -> Result<(), String> {
    let mut last = None;
    for attempt in 0..6 {
        match std::fs::rename(stage, destination) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = Some(error);
                if attempt < 5 {
                    // Windows antivirus can briefly hold the new executables.
                    tokio::time::sleep(Duration::from_millis(200 * (1 << attempt))).await;
                }
            }
        }
    }
    Err(format!(
        "Could not finish the {name} installation: {}",
        last.unwrap()
    ))
}

async fn install_private_dotnet(
    data_dir: &Path,
    mut progress: impl FnMut(&str) + Send,
) -> Result<PathBuf, String> {
    let root = private_dotnet_dir(data_dir)
        .parent()
        .ok_or("Missing tool directory")?
        .to_path_buf();
    let destination = private_dotnet_dir(data_dir);
    let executable = destination.join(dotnet_name());
    let _lock = prepare_install(&root)?;
    if installed_sdk_version(&executable).await.is_some() {
        return Ok(executable);
    }
    // A killed install can leave version folders without a working host. The
    // app never silently repairs that in place, so replace the whole folder.
    if destination.exists() {
        std::fs::remove_dir_all(&destination)
            .map_err(|e| format!("Could not remove an interrupted .NET installation: {e}"))?;
    }
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
        &root,
        suffix,
    )
    .await?
    // PowerShell must be able to reopen the script on Windows. The TempPath
    // retains automatic cleanup while closing the download's file handle.
    .into_temp_path();
    let stage = tempfile::Builder::new()
        .prefix(".codingpad-install-dotnet-")
        .tempdir_in(&root)
        .map_err(|e| format!("Could not create the .NET staging folder: {e}"))?;
    progress("Installing the .NET long-term-support SDK…");
    run_dotnet_install_script(&script, stage.path(), &mut progress).await?;
    let version = installed_sdk_version(&stage.path().join(dotnet_name()))
        .await
        .ok_or("The .NET SDK was installed but could not be started. Retry installation.")?;
    // Promote only a verified, complete SDK so an interrupted copy is never
    // mistaken for an installed one on the next attempt.
    promote_directory(stage.path(), &destination, ".NET").await?;
    progress(&format!(".NET SDK {version} installed. Starting script…"));
    Ok(executable)
}

async fn run_dotnet_install_script(
    script: &Path,
    directory: &Path,
    mut progress: impl FnMut(&str) + Send,
) -> Result<(), String> {
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
        // dotnet-install.sh is bash, not POSIX sh; Debian and Ubuntu use dash
        // for /bin/sh, where it fails on `[[` and pipefail.
        let mut command = tokio::process::Command::new("bash");
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
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            if cfg!(windows) {
                "The .NET installer needs PowerShell, which was not found.".to_owned()
            } else {
                "The .NET installer needs bash, which was not found. Install bash and retry."
                    .to_owned()
            }
        } else {
            format!("Could not start the .NET install script: {error}")
        }
    })?;
    #[cfg(unix)]
    let mut process_group = ProcessGroup::new(child.id());
    let stdout = child
        .stdout
        .take()
        .ok_or("Could not read the .NET installer output")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Could not read the .NET installer errors")?;
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut text).await;
        text
    });
    let install = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| format!("Could not read the .NET installer output: {e}"))?
        {
            let line = line.trim();
            if let Some(message) = friendly_dotnet_progress(line) {
                progress(message);
            }
        }
        let status = child
            .wait()
            .await
            .map_err(|e| format!("Could not wait for the .NET installer: {e}"))?;
        #[cfg(unix)]
        process_group.disarm();
        let errors = errors.await.unwrap_or_default();
        Ok::<_, String>((status, errors))
    };
    let (status, errors) = tokio::time::timeout(DOTNET_INSTALL_TIMEOUT, install)
        .await
        .map_err(|_| {
            "The .NET installation timed out. Check your connection and retry.".to_owned()
        })??;
    if status.success() {
        return Ok(());
    }
    let message = errors.trim();
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
    fn bun_platforms_have_pinned_artifacts() {
        for (os, arch, musl) in [
            ("linux", "x86_64", false),
            ("linux", "x86_64", true),
            ("linux", "aarch64", false),
            ("linux", "aarch64", true),
            ("macos", "x86_64", false),
            ("macos", "aarch64", false),
            ("windows", "x86_64", false),
            ("windows", "aarch64", false),
        ] {
            for avx2 in [true, false] {
                let (asset, hash) = bun_release(os, arch, musl, avx2).unwrap();
                assert!(asset.starts_with("bun-"), "{asset}");
                assert!(asset.ends_with(".zip"), "{asset}");
                assert_eq!(hash.len(), 64);
                assert!(hash.bytes().all(|c| c.is_ascii_hexdigit()));
            }
        }
        assert_eq!(
            bun_release("linux", "x86_64", false, true).unwrap().0,
            "bun-linux-x64.zip"
        );
        assert_eq!(
            bun_release("linux", "x86_64", false, false).unwrap().0,
            "bun-linux-x64-baseline.zip"
        );
        assert_eq!(
            bun_release("linux", "x86_64", true, false).unwrap().0,
            "bun-linux-x64-musl-baseline.zip"
        );
        assert_eq!(
            bun_release("macos", "x86_64", false, false).unwrap().0,
            "bun-darwin-x64-baseline.zip"
        );
        assert_eq!(
            bun_release("windows", "x86_64", false, false).unwrap().0,
            "bun-windows-x64-baseline.zip"
        );
        // ARM builds have no baseline variant.
        assert_eq!(
            bun_release("linux", "aarch64", false, false).unwrap().0,
            "bun-linux-aarch64.zip"
        );
        assert!(bun_release("linux", "mips", false, true).is_err());
        assert!(bun_release("unknown", "x86_64", false, true).is_err());
    }

    #[test]
    fn resolve_bun_prefers_the_user_copy_then_the_private_one() {
        let data = tempfile::tempdir().unwrap();
        let user = data.path().join("user-bun");
        let private = private_bun(data.path());
        assert_eq!(resolve_bun_with(user.clone(), data.path()), user);
        std::fs::create_dir_all(private.parent().unwrap()).unwrap();
        std::fs::write(&private, "private").unwrap();
        assert_eq!(resolve_bun_with(user.clone(), data.path()), private);
        std::fs::write(&user, "user").unwrap();
        assert_eq!(resolve_bun_with(user.clone(), data.path()), user);
    }

    #[test]
    fn bun_for_uses_an_explicit_pin_before_the_user_or_private_copy() {
        let data = tempfile::tempdir().unwrap();
        let pin = data.path().join("pinned-bun");
        assert_eq!(
            bun_for(Some(&pin), Some(data.path())),
            pin,
            "an explicit pin wins even when the file does not exist"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bun_health_check_runs_the_binary() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = |name: &str, body: &str| {
            let path = directory.path().join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        assert!(
            bun_runs(&executable("good-bun", "#!/bin/sh\necho 1.4.2\n")).await,
            "a working binary passes the check"
        );
        assert!(
            !bun_runs(&executable("bad-bun", "#!/bin/sh\nexit 3\n")).await,
            "a binary that cannot start is rejected"
        );
        assert!(!bun_runs(&directory.path().join("missing-bun")).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_slow_version_probe_is_not_a_definite_failure() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("slow-bun");
        std::fs::write(&path, "#!/bin/sh\nsleep 60\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            bun_version(&path, Duration::from_millis(100)).await,
            None,
            "the recovery path keeps a copy when the probe does not finish"
        );
    }

    #[test]
    fn private_bun_lives_under_the_app_data_folder() {
        let directory = tempfile::tempdir().unwrap();
        let path = private_bun(directory.path());
        assert!(path.starts_with(directory.path().join("toolchains").join("bun")));
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            if cfg!(windows) { "bun.exe" } else { "bun" }
        );
        assert!(path.to_string_lossy().contains(BUN_VERSION));
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
    fn windows_installer_extraction_message_shows_unpacking() {
        assert_eq!(
            friendly_dotnet_progress("dotnet-install: Extracting the archive."),
            Some("Unpacking the .NET SDK…")
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn promotion_retries_while_a_file_is_locked() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = tempfile::tempdir().unwrap();
        let stage = directory.path().join("stage");
        let destination = directory.path().join("sdk");
        std::fs::create_dir(&stage).unwrap();
        let file = stage.join("dotnet.exe");
        std::fs::write(&file, "sdk").unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&file)
            .unwrap();
        assert!(std::fs::rename(&stage, &destination).is_err());
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1000)).await;
            drop(lock);
        });
        promote_directory(&stage, &destination, ".NET")
            .await
            .unwrap();
        release.await.unwrap();
        assert_eq!(
            std::fs::read(destination.join("dotnet.exe")).unwrap(),
            b"sdk"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dotnet_install_script_streams_progress_and_reports_failure() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("install.sh");
        std::fs::write(
            &script,
            "#!/usr/bin/env bash\necho 'dotnet-install: Attempting to download using aka.ms link'\necho 'dotnet-install: noise that learners should not see'\necho 'dotnet-install: Extracting archive from /tmp/sdk.tar.gz'\n",
        )
        .unwrap();
        let mut progress = Vec::new();
        super::run_dotnet_install_script(&script, directory.path(), |line| {
            progress.push(line.to_owned())
        })
        .await
        .unwrap();
        assert_eq!(
            progress,
            ["Downloading the .NET SDK…", "Unpacking the .NET SDK…"]
        );

        std::fs::write(
            &script,
            "#!/usr/bin/env bash\necho 'partial output'\necho 'connection failed' >&2\nexit 3\n",
        )
        .unwrap();
        let error = super::run_dotnet_install_script(&script, directory.path(), |_| {})
            .await
            .unwrap_err();
        assert!(error.contains("connection failed"), "{error}");
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
        std::fs::create_dir(directory.join(".codingpad-install-dotnet-stale")).unwrap();
        let lock = prepare_install(directory).unwrap();
        assert!(!directory.join(".tmp-old").exists());
        assert!(!directory.join(".codingpad-install-old").exists());
        assert!(!directory.join(".codingpad-install-dotnet-stale").exists());
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

    /// Network acceptance test for the one-click Bun install.
    #[tokio::test]
    #[ignore = "downloads the official Bun release; run explicitly when validating the installer"]
    async fn official_bun_release_installs_privately_and_runs_typescript() {
        let data = tempfile::tempdir().unwrap();
        let installed = install_private_bun(data.path(), |_| {}).await.unwrap();
        assert_eq!(installed, private_bun(data.path()));
        let output = tokio::process::Command::new(&installed)
            .arg("--version")
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout)
            .trim()
            .starts_with(BUN_VERSION));
        let supervisor = crate::Supervisor::with_bun(installed);
        let mut events = Vec::new();
        let result = supervisor
            .run_language(
                "bun",
                "console.log('private bun works');\nawait Promise.resolve(6 * 7)",
                crate::Language::Typescript,
                |event| events.push(event),
            )
            .await
            .unwrap();
        assert!(result.ok, "{events:?}");
        assert!(events
            .iter()
            .any(|event| event.params.to_string().contains("private bun works")));
        assert!(events
            .iter()
            .any(|event| event.params.to_string().contains("42")));
        supervisor.shutdown().await;
    }

    /// Network acceptance test for the one-click .NET SDK install.
    #[tokio::test]
    #[ignore = "downloads the official .NET LTS SDK; run explicitly when validating the installer"]
    async fn official_dotnet_script_installs_lts_sdk_privately() {
        let data = tempfile::tempdir().unwrap();
        let installed = install_private_dotnet(data.path(), |_| {}).await.unwrap();
        assert_eq!(installed, private_dotnet(data.path()));
        let sdks = list_dotnet(&installed, "--list-sdks").await.unwrap();
        newest_sdk_version(&sdks).expect("a supported SDK is installed");
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
