use super::source::source_prefix_len;
use std::{ops::Range, path::Path};
use toml::{Table, Value};

pub(crate) struct Manifest {
    pub(crate) cargo: String,
    pub(super) frontmatter: Range<usize>,
    pub(super) needs_packages: bool,
}

#[derive(Debug)]
pub(crate) struct ManifestError {
    pub(super) line: usize,
    message: String,
}

impl ManifestError {
    fn at(source: &str, offset: usize, message: impl Into<String>) -> Self {
        Self {
            line: source.as_bytes()[..offset]
                .iter()
                .filter(|&&byte| byte == b'\n')
                .count()
                + 1,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cargo-script frontmatter on line {}: {}",
            self.line, self.message
        )
    }
}

impl Manifest {
    pub(crate) fn parse(source: &str, path: Option<&Path>) -> Result<Self, ManifestError> {
        let (embedded, frontmatter) = frontmatter(source)?;
        let error = |message| ManifestError::at(source, frontmatter.start, message);
        let body = frontmatter.start
            + source[frontmatter.start..]
                .find('\n')
                .map_or(0, |end| end + 1);
        let mut cargo = embedded.parse::<Table>().map_err(|failure| {
            let offset = body + failure.span().map_or(0, |span| span.start);
            ManifestError::at(source, offset, failure.message())
        })?;
        set_package_defaults(&mut cargo).map_err(error)?;
        resolve_dependencies(&mut cargo, path).map_err(error)?;

        let needs_packages = [
            "dependencies",
            "dev-dependencies",
            "target",
            "patch",
            "replace",
        ]
        .into_iter()
        .any(|key| {
            cargo
                .get(key)
                .and_then(Value::as_table)
                .is_some_and(|section| !section.is_empty())
        });
        table_or_insert(&mut cargo, "dependencies")
            .map_err(error)?
            .insert(
                "codingpad".to_owned(),
                Value::Table(Table::from_iter([(
                    "path".to_owned(),
                    Value::from("codingpad"),
                )])),
            );
        cargo.insert("workspace".to_owned(), Value::Table(Table::new()));
        Ok(Self {
            cargo: cargo.to_string(),
            frontmatter,
            needs_packages,
        })
    }
}

fn set_package_defaults(cargo: &mut Table) -> Result<(), String> {
    const ROOT_FIELDS: &[&str] = &[
        "workspace",
        "lib",
        "bin",
        "example",
        "test",
        "bench",
        "build-dependencies",
        "build_dependencies",
    ];
    const PACKAGE_FIELDS: &[&str] = &[
        "workspace",
        "build",
        "default-run",
        "links",
        "autolib",
        "autobins",
        "autoexamples",
        "autotests",
        "autobenches",
    ];
    reject_fields(cargo, ROOT_FIELDS, "")?;
    let package = table_or_insert(cargo, "package")?;
    reject_fields(package, PACKAGE_FIELDS, "package.")?;
    for (key, value) in [
        ("name", "codingpad-script"),
        ("version", "0.0.0"),
        ("edition", "2024"),
    ] {
        package.entry(key).or_insert_with(|| Value::from(value));
    }
    Ok(())
}

fn reject_fields(table: &Table, fields: &[&str], prefix: &str) -> Result<(), String> {
    if let Some(field) = fields.iter().find(|&&field| table.contains_key(field)) {
        return Err(format!(
            "{prefix}{field} is unavailable in single-file packages"
        ));
    }
    Ok(())
}

fn resolve_dependencies(cargo: &mut Table, path: Option<&Path>) -> Result<(), String> {
    let script = path
        .map(std::path::absolute)
        .transpose()
        .map_err(|error| format!("Could not resolve the script path: {error}"))?;
    let directory = script.as_deref().and_then(Path::parent);
    dependency_paths(cargo, directory)?;
    if let Some(targets) = cargo.get_mut("target").and_then(Value::as_table_mut) {
        for target in targets
            .iter_mut()
            .filter_map(|(_, value)| value.as_table_mut())
        {
            reject_fields(
                target,
                &["build-dependencies", "build_dependencies"],
                "target.",
            )?;
            dependency_paths(target, directory)?;
        }
    }
    if let Some(patch) = cargo.get_mut("patch").and_then(Value::as_table_mut) {
        for registry in patch
            .iter_mut()
            .filter_map(|(_, value)| value.as_table_mut())
        {
            make_paths_absolute(registry, directory)?;
        }
    }
    if let Some(replace) = cargo.get_mut("replace").and_then(Value::as_table_mut) {
        make_paths_absolute(replace, directory)?;
    }
    Ok(())
}

fn table_or_insert<'a>(root: &'a mut Table, key: &str) -> Result<&'a mut Table, String> {
    root.entry(key)
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| format!("{key} must be a table"))
}

fn dependency_paths(root: &mut Table, directory: Option<&Path>) -> Result<(), String> {
    for key in ["dependencies", "dev-dependencies", "dev_dependencies"] {
        if let Some(dependencies) = root.get_mut(key).and_then(Value::as_table_mut) {
            make_paths_absolute(dependencies, directory)?;
        }
    }
    Ok(())
}

fn make_paths_absolute(dependencies: &mut Table, directory: Option<&Path>) -> Result<(), String> {
    for (name, dependency) in dependencies {
        let Some(value) = dependency.get_mut("path") else {
            continue;
        };
        let Some(path) = value.as_str().map(Path::new) else {
            return Err(format!("{name}.path must be a string"));
        };
        if path.is_relative() {
            let directory = directory.ok_or_else(|| {
                format!("Save the Rust script before using a relative path for dependency {name}.")
            })?;
            *value = Value::from(directory.join(path).to_str().ok_or_else(|| {
                format!("The path for Rust dependency {name} is not valid UTF-8.")
            })?);
        }
    }
    Ok(())
}

pub(crate) fn frontmatter(source: &str) -> Result<(&str, Range<usize>), ManifestError> {
    let mut offset = source_prefix_len(source);
    if source[offset..].starts_with('\n') {
        offset += 1; // The shebang terminator.
    }
    let mut lines = source[offset..].split_inclusive('\n');
    let mut opener = "";
    for line in lines.by_ref() {
        if !line.trim().is_empty() {
            opener = line;
            break;
        }
        offset += line.len();
    }
    let dashes = opener.bytes().take_while(|byte| *byte == b'-').count();
    if dashes < 3 {
        return Ok(("", 0..0));
    }
    let fence = &opener[..dashes];
    let info = opener[dashes..].trim();
    let invalid = |message| ManifestError::at(source, offset, message);
    if dashes > 255 || !matches!(info, "" | "cargo") {
        return Err(invalid(
            "Expected a fence of 3 to 255 dashes and an optional cargo identifier.",
        ));
    }
    let body = offset + opener.len();
    let mut end = body;
    for line in lines {
        if line.starts_with(fence) {
            if line.trim_end() != fence {
                return Err(invalid("The closing fence must match the opening fence."));
            }
            let after_fence = end + line.len();
            let remaining = &source[after_fence..];
            let next = remaining.trim_start();
            if next.starts_with("---") {
                return Err(ManifestError::at(
                    source,
                    source.len() - next.len(),
                    "Only one cargo-script frontmatter block is allowed.",
                ));
            }
            return Ok((&source[body..end], offset..after_fence));
        }
        end += line.len();
    }
    Err(invalid("Missing closing fence."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_invalid_frontmatter_and_leaves_negative_expressions_alone() {
        for (source, expected) in [
            ("---cargo\n[dependencies]\n", "Missing closing fence"),
            ("---cargo\n[dependencies\n---", "expected"),
            ("---other\n---", "optional cargo identifier"),
            (
                "---cargo\n---\n\n---cargo\n---",
                "Only one cargo-script frontmatter block",
            ),
            (
                concat!("---cargo\n[package]\n", r#"default-run = "demo""#, "\n---"),
                "package.default-run is unavailable",
            ),
            (
                "---cargo\n[workspace]\n---",
                "unavailable in single-file packages",
            ),
        ] {
            let message = Manifest::parse(source, None).err().unwrap().to_string();
            assert!(message.contains(expected), "{message}");
        }
        assert!(Manifest::parse("-42", None).unwrap().frontmatter.is_empty());
    }
}
