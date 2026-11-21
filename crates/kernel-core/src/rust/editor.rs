use super::{build_cache, cargo, manifest, rust_tool, BuildCache, Manifest, Source, SCRIPT_FILE};
use crate::shadow::ShadowProject;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tokio::process::Command;

pub(crate) const MISSING_ANALYZER: &str =
    "rust-analyzer was not found. Install it with `rustup component add rust-analyzer rust-src` or set CODINGPAD_RUST_LSP.";

pub(crate) fn analyzer_command() -> Result<Command, String> {
    let executable = std::env::var_os("CODINGPAD_RUST_LSP")
        .map(PathBuf::from)
        .unwrap_or_else(|| rust_tool("rust-analyzer"));
    let executable = std::path::absolute(executable).map_err(|error| error.to_string())?;
    if !executable.is_file() {
        return Err(MISSING_ANALYZER.into());
    }
    let mut command = Command::new(executable);
    let cargo = cargo();
    if let Some(bin) = cargo.parent().filter(|path| path.is_absolute()) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(bin.to_owned()).chain(std::env::split_paths(&path)),
        )
        .map_err(|error| format!("Could not add Cargo to the rust-analyzer PATH: {error}"))?;
        command.env("PATH", path);
    }
    Ok(command)
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

impl Source {
    pub(crate) fn editor_to_shadow(&self, value: &Value) -> Option<Value> {
        let offset = position_offset(&self.original, value)?;
        if self.frontmatter.contains(&offset) {
            return None;
        }
        let generated = self
            .segments
            .iter()
            .rev()
            .find_map(|&(start, end, original)| {
                let delta = offset.checked_sub(original)?;
                (delta <= end - start).then_some(start + delta)
            })?;
        byte_position(&self.generated, generated)
    }

    fn shadow_to_editor(&self, value: &Value, insertion: bool) -> Option<Value> {
        let offset = position_offset(&self.generated, value)?;
        let mut original = self.original_offset(offset)?;
        if self.frontmatter.contains(&original) {
            if !insertion {
                return None;
            }
            original = self.frontmatter.end;
        }
        byte_position(&self.original, original)
    }

    fn map_range(&self, range: &mut Value, insertion: bool) -> bool {
        let insertion = insertion && range["start"] == range["end"];
        let Some(start) = self.shadow_to_editor(&range["start"], insertion) else {
            return false;
        };
        let Some(end) = self.shadow_to_editor(&range["end"], insertion) else {
            return false;
        };
        *range = json!({ "start": start, "end": end });
        true
    }

    pub(crate) fn map_diagnostics(&self, params: &mut Value, uri: &str) {
        let Some(diagnostics) = params["diagnostics"].as_array_mut() else {
            return;
        };
        diagnostics.retain_mut(|diagnostic| {
            if !self.map_range(&mut diagnostic["range"], false) {
                return false;
            }
            if let Some(related) = diagnostic["relatedInformation"].as_array_mut() {
                related.retain_mut(|info| {
                    let location = &mut info["location"];
                    location["uri"] != uri || self.map_range(&mut location["range"], false)
                });
            }
            true
        });
    }

    pub(crate) fn map_response(&self, method: &str, response: &mut Value) {
        match method {
            "textDocument/completion" => {
                let items = if response.is_array() {
                    response.as_array_mut()
                } else {
                    response.get_mut("items").and_then(Value::as_array_mut)
                };
                if let Some(items) = items {
                    items.retain_mut(|item| {
                        let valid = match item.get_mut("textEdit") {
                            Some(edit) if edit.get("range").is_some() => {
                                self.map_range(&mut edit["range"], false)
                            }
                            Some(edit) => {
                                self.map_range(&mut edit["insert"], false)
                                    && self.map_range(&mut edit["replace"], false)
                            }
                            None => true,
                        };
                        self.map_additional_edits(item);
                        valid
                    });
                }
            }
            "completionItem/resolve" => self.map_additional_edits(response),
            "textDocument/hover" => {
                if let Some(range) = response.get_mut("range") {
                    if !self.map_range(range, false) {
                        response.as_object_mut().unwrap().remove("range");
                    }
                }
            }
            _ => {}
        }
    }

    fn map_additional_edits(&self, item: &mut Value) {
        if let Some(edits) = item
            .get_mut("additionalTextEdits")
            .and_then(Value::as_array_mut)
        {
            edits.retain_mut(|edit| self.map_range(&mut edit["range"], true));
        }
    }
}

fn position_offset(text: &str, position: &Value) -> Option<usize> {
    let line = usize::try_from(position["line"].as_u64()?).ok()?;
    let character = usize::try_from(position["character"].as_u64()?).ok()?;
    let mut lines = text.split('\n');
    let offset: usize = lines.by_ref().take(line).map(|line| line.len() + 1).sum();
    let line = lines.next()?.trim_end_matches('\r');
    let mut utf16 = 0;
    for (index, ch) in line.char_indices() {
        if utf16 == character {
            return Some(offset + index);
        }
        utf16 += ch.len_utf16();
    }
    (utf16 == character).then_some(offset + line.len())
}

fn byte_position(text: &str, offset: usize) -> Option<Value> {
    let prefix = text.get(..offset)?;
    Some(json!({
        "line": prefix.bytes().filter(|&byte| byte == b'\n').count(),
        "character": prefix.rsplit('\n').next()?.encode_utf16().count(),
    }))
}
