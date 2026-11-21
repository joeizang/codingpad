use super::{installer, prepare_install, promote_directory, Artifact, Toolchains, MAX_TOOL_BYTES};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

const RUSTUP_VERSION: &str = "1.29.3";

// Published SHA-256 digests from the corresponding rustup archive directory.
const RELEASES: &[(&str, &str)] = &[
    (
        "x86_64-unknown-linux-gnu",
        "b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282",
    ),
    (
        "x86_64-unknown-linux-musl",
        "1552169aba2836777136ccff7a381e95c17233b5d8250cd5e10ae3cb971bfcfc",
    ),
    (
        "aarch64-unknown-linux-musl",
        "956deacdc27ab82ae7ac821d5d3e9e95e4795d476d17efe99743bb8531ca1031",
    ),
    (
        "x86_64-apple-darwin",
        "bc107f1fb0228de7fa32413c89c1b305cea9cd256e94b01150cd054128145429",
    ),
    (
        "aarch64-apple-darwin",
        "63752a1919a3a4793173ec70f8f212621a7ce38ba6a246095984482f4de20f9f",
    ),
    (
        "x86_64-pc-windows-msvc",
        "38027d51d69a5d05493627a0634dbea52d6cd793c8f072d2373d3f4163eec7db",
    ),
    (
        "aarch64-pc-windows-msvc",
        "407e0d72f81d26a01f9d032b8317908cbb2917ba22765474b682b41390792ad9",
    ),
];

pub(crate) const MISSING: &str = "Rust was not found. Install Rust from Results in CodingPad, or install a stable toolchain from https://rustup.rs/ and try again.";

#[derive(Default)]
pub(crate) struct RustToolchain {
    private: Option<PathBuf>,
}

impl RustToolchain {
    fn private(root: PathBuf) -> Self {
        Self {
            private: Some(root),
        }
    }

    pub(crate) async fn resolve(data: Option<&Path>) -> Option<Self> {
        let user = Self::default();
        if user.is_available().await {
            return Some(user);
        }
        let private = Self::private(std::path::absolute(private_dir(data?)).ok()?);
        private.is_available().await.then_some(private)
    }

    pub(crate) fn executable(&self, name: &str) -> PathBuf {
        let name = executable_name(name);
        if let Some(root) = &self.private {
            return root.join("cargo/bin").join(name);
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .chain(std::env::var_os("CARGO_HOME").map(|path| PathBuf::from(path).join("bin")))
            .chain(dirs::home_dir().map(|home| home.join(".cargo/bin")))
            .map(|dir| dir.join(&name))
            .find(|path| path.is_file())
            .and_then(|path| std::path::absolute(path).ok())
            .unwrap_or_else(|| PathBuf::from(name))
    }

    pub(crate) fn command(&self, name: &str) -> Command {
        let mut command = Command::new(self.executable(name));
        self.configure(&mut command);
        command
    }

    pub(crate) fn configure(&self, command: &mut Command) {
        if let Some(root) = &self.private {
            private_homes(command, root);
            for key in [
                "RUSTUP_TOOLCHAIN",
                "RUSTC",
                "RUSTDOC",
                "RUSTC_WRAPPER",
                "RUSTC_WORKSPACE_WRAPPER",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "CARGO_BUILD_TARGET",
                "CARGO_BUILD_RUSTC",
            ] {
                command.env_remove(key);
            }
        }
        let cargo = self.executable("cargo");
        if let Some(bin) = cargo.parent().filter(|path| path.is_absolute()) {
            let path = std::env::var_os("PATH").unwrap_or_default();
            if let Ok(paths) = std::env::join_paths(
                std::iter::once(bin.to_owned()).chain(std::env::split_paths(&path)),
            ) {
                command.env("PATH", paths);
            }
            if self.private.is_some() {
                command.env("RUSTC", self.executable("rustc"));
                command.env("RUSTDOC", self.executable("rustdoc"));
            }
        }
    }

    async fn is_available(&self) -> bool {
        let (cargo, rustc) = tokio::join!(
            version(self.command("cargo")),
            version(self.command("rustc"))
        );
        cargo && rustc
    }
}

async fn version(mut command: Command) -> bool {
    command.arg("--version");
    run(command, Duration::from_secs(10)).await.is_ok()
}

fn private_dir(data: &Path) -> PathBuf {
    data.join("toolchains/rust/stable")
}

fn private_homes(command: &mut Command, root: &Path) {
    command
        .env("CARGO_HOME", root.join("cargo"))
        .env("RUSTUP_HOME", root.join("rustup"));
}

fn executable_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

impl Toolchains {
    pub async fn install_rust(&self, progress: impl FnMut(&str) + Send) -> Result<PathBuf, String> {
        let _guard = self.install.lock().await;
        if let Some(toolchain) = RustToolchain::resolve(Some(&self.data_dir)).await {
            return Ok(toolchain.executable("cargo"));
        }
        install_private_rust(&self.data_dir, progress).await
    }
}

async fn install_private_rust(
    data_dir: &Path,
    mut progress: impl FnMut(&str) + Send,
) -> Result<PathBuf, String> {
    let destination = std::path::absolute(private_dir(data_dir)).map_err(|e| e.to_string())?;
    let root = destination.parent().ok_or("Missing Rust tool directory")?;
    let _lock = prepare_install(root)?;
    let private = RustToolchain::private(destination.clone());
    if private.is_available().await {
        return Ok(private.executable("cargo"));
    }
    if destination.exists() {
        std::fs::remove_dir_all(&destination)
            .map_err(|e| format!("Could not remove the unusable private Rust toolchain: {e}"))?;
    }

    let triple = match std::env::consts::OS {
        "linux" if cfg!(target_env = "musl") => "unknown-linux-musl",
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        os => return Err(format!("Private Rust installation is unavailable on {os}.")),
    };
    let triple = format!("{}-{triple}", std::env::consts::ARCH);
    let (_, sha256) = RELEASES
        .iter()
        .find(|(target, _)| *target == triple)
        .ok_or_else(|| format!("Private Rust installation is unavailable on {triple}."))?;
    // The x64 GNU toolchain ships its own linker and Windows import libs.
    let host = if triple == "x86_64-pc-windows-msvc" {
        "x86_64-pc-windows-gnu"
    } else {
        &triple
    };

    let stage = tempfile::Builder::new()
        .prefix(".codingpad-install-rust-")
        .tempdir_in(root)
        .map_err(|e| format!("Could not create the Rust staging directory: {e}"))?;
    let name = executable_name("rustup-init");
    progress("Downloading and verifying rustup…");
    let bootstrap = installer::download_verified(
        &Artifact {
            url: &format!(
                "https://static.rust-lang.org/rustup/archive/{RUSTUP_VERSION}/{triple}/{name}"
            ),
            sha256,
            max_bytes: MAX_TOOL_BYTES,
        },
        stage.path(),
    )
    .await?;
    // rustup chooses installer mode from the exact executable name.
    let bootstrap = bootstrap
        .persist(stage.path().join(&name))
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        bootstrap
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    drop(bootstrap);
    let mut command = Command::new(stage.path().join(&name));
    private_homes(&mut command, stage.path());
    command
        .current_dir(stage.path())
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTUP_UPDATE_ROOT")
        .env_remove("RUSTUP_DIST_SERVER")
        .env_remove("RUSTUP_OVERRIDE_HOST_TRIPLE")
        .env("RUSTUP_INIT_SKIP_PATH_CHECK", "yes")
        .args([
            "--yes",
            "--no-modify-path",
            "--default-host",
            host,
            "--default-toolchain",
            "stable",
            "--profile",
            "minimal",
            "--component",
            "rust-analyzer,rust-src",
        ]);

    progress("Installing stable Rust and rust-analyzer…");
    run(command, Duration::from_secs(30 * 60)).await?;
    progress("Checking that Rust can compile and run a script…");
    let staged = RustToolchain::private(stage.path().to_owned());
    verify_install(&staged, stage.path()).await?;
    std::fs::remove_file(stage.path().join(name))
        .map_err(|error| format!("Could not remove the rustup bootstrap: {error}"))?;
    promote_directory(stage.path(), &destination, "Rust").await?;
    progress("Rust installed. Starting script…");
    Ok(private.executable("cargo"))
}

async fn verify_install(toolchain: &RustToolchain, directory: &Path) -> Result<(), String> {
    let probe = tempfile::tempdir_in(directory).map_err(|e| e.to_string())?;
    let source = probe.path().join("main.rs");
    let executable = probe.path().join(executable_name("hello"));
    std::fs::write(&source, "fn main() {}").map_err(|e| e.to_string())?;
    let mut compile = toolchain.command("rustc");
    compile
        .current_dir(probe.path())
        .arg(source)
        .arg("-o")
        .arg(&executable);
    run(compile, Duration::from_secs(60)).await
        .map_err(|error| format!("Rust could not compile a test script. Check that your system linker is installed and retry. {error}"))?;
    run(Command::new(executable), Duration::from_secs(10)).await?;
    if !toolchain.is_available().await || !version(toolchain.command("rust-analyzer")).await {
        return Err(
            "The private Rust toolchain did not pass its startup check. Retry installation.".into(),
        );
    }
    Ok(())
}

async fn run(mut command: Command, timeout: Duration) -> Result<(), String> {
    command.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| "The Rust command timed out.")?
        .map_err(|e| format!("Could not start the Rust command: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Rust command failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "downloads stable Rust and its editor components"]
    async fn private_install_recovers_and_compiles_after_promotion() {
        let data = tempfile::tempdir().unwrap();
        let root = private_dir(data.path());
        let abandoned = root.with_file_name(".codingpad-install-rust-abandoned");
        std::fs::create_dir_all(&abandoned).unwrap();
        std::fs::write(abandoned.join("partial"), b"interrupted").unwrap();
        std::fs::create_dir_all(root.join("cargo/bin")).unwrap();

        let cargo = install_private_rust(data.path(), |message| eprintln!("{message}"))
            .await
            .unwrap();
        assert!(!abandoned.exists());
        let private = RustToolchain::private(root);
        assert_eq!(cargo, private.executable("cargo"));
        verify_install(&private, data.path()).await.unwrap();

        let probe = tempfile::tempdir().unwrap();
        std::fs::create_dir(probe.path().join("src")).unwrap();
        std::fs::write(
            probe.path().join("Cargo.toml"),
            r#"[package]
name = "private-install-probe"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();
        std::fs::write(
            probe.path().join("src/main.rs"),
            "fn main() { println!(\"private Rust\"); }",
        )
        .unwrap();
        let mut command = private.command("cargo");
        command
            .current_dir(probe.path())
            .args(["run", "--offline", "--quiet"]);
        run(command, Duration::from_secs(60)).await.unwrap();
    }
}
