//! uv release manifest and app-private toolchain lifecycle.
use crate::{
    find_uv,
    installer::{self, Archive, Artifact},
};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

pub const UV_VERSION: &str = "0.13.6";

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
}

// The OS releases this lock after a crash. Keep the lock file in place: deleting
// it would let different processes lock different inodes and sweep active files.
fn prepare_install(directory: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(directory)
        .map_err(|e| format!("Could not create the tool folder: {e}"))?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".install.lock"))
        .map_err(|e| format!("Could not open the tool installer lock: {e}"))?;
    lock.try_lock().map_err(|e| format!("Could not lock the tool installer. Another CodingPad instance may be installing uv; retry when it finishes: {e}"))?;
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
        drop(lock); // Also happens automatically if the process exits.
        let _lock = prepare_install(directory).unwrap();
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
}
