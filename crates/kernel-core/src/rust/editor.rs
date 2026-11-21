use super::{build_cache, manifest, BuildCache, Manifest, Source, SCRIPT_FILE};
use crate::shadow::ShadowProject;
use std::{path::PathBuf, sync::Arc};
use tokio::process::Command;

pub(crate) struct AnalyzerCommand {
    pub(crate) command: Command,
    pub(crate) help: String,
}

pub(crate) async fn analyzer_command(builds: &BuildCache) -> Result<AnalyzerCommand, String> {
    let toolchain = builds.toolchain().await.unwrap_or_default();
    let help = toolchain.analyzer_help();
    let executable = match std::env::var_os("CODINGPAD_RUST_LSP") {
        Some(path) => PathBuf::from(path),
        None => toolchain.analyzer().await.ok_or_else(|| help.clone())?,
    };
    let executable = std::path::absolute(executable).map_err(|error| error.to_string())?;
    if !executable.is_file() {
        return Err(help);
    }
    let mut command = Command::new(executable);
    toolchain.configure(&mut command);
    Ok(AnalyzerCommand { command, help })
}

pub(crate) struct RustShadowProject {
    pub(crate) shadow: ShadowProject,
    runtime_name: String,
}

impl RustShadowProject {
    pub(crate) async fn create(
        cache: &BuildCache,
        manifest: &Manifest,
        source: &str,
    ) -> Result<Self, String> {
        let shadow = ShadowProject::temporary(SCRIPT_FILE)?;
        let runtime = cache.stage_runtime().await?;
        let source =
            Source::with_frontmatter(source, manifest.frontmatter.clone(), &manifest.runtime_name);
        build_cache::stage(
            &shadow,
            &source.generated,
            manifest,
            &runtime,
            "codingpad-editor",
        )?;
        Ok(Self {
            shadow,
            runtime_name: manifest.runtime_name.clone(),
        })
    }

    pub(crate) fn stage(&self, source: &str) -> Result<Arc<Source>, String> {
        let (_, frontmatter) = manifest::frontmatter(source).map_err(|error| error.to_string())?;
        let source = Source::with_frontmatter(source, frontmatter, &self.runtime_name);
        build_cache::write_if_changed(&self.shadow.root().join(SCRIPT_FILE), &source.generated)
            .map_err(|error| format!("Could not stage Rust editor source: {error}"))?;
        Ok(Arc::new(source))
    }
}
