use super::source::source_prefix_len;
use std::{ops::Range, path::Path};
use toml::{Table, Value};

const DEPENDENCY_SECTIONS: &[&str] = &["dependencies", "dev-dependencies", "dev_dependencies"];

pub(crate) struct Manifest {
    cargo: Table,
    pub(super) frontmatter: Range<usize>,
    pub(super) needs_packages: bool,
    pub(super) runtime_name: String,
    dependency_line: usize,
    dependency_lines: Vec<(String, usize)>,
}

#[derive(Debug)]
pub(crate) struct ManifestError {
    pub(super) line: usize,
    message: String,
}

impl ManifestError {
    fn at(source: &str, offset: usize, message: impl Into<String>) -> Self {
        Self {
            line: line_at(source, offset),
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
        let body = frontmatter.start
            + source[frontmatter.start..]
                .find('\n')
                .map_or(0, |end| end + 1);
        let error = |issue: ManifestIssue| {
            let offset = toml_edit::Document::parse(embedded)
                .ok()
                .and_then(|manifest| field_offset(manifest.as_table(), &issue.field))
                .map_or(frontmatter.start, |offset| body + offset);
            ManifestError::at(source, offset, issue.message)
        };
        let mut cargo = embedded.parse::<Table>().map_err(|failure| {
            let offset = body + failure.span().map_or(0, |span| span.start);
            ManifestError::at(source, offset, failure.message())
        })?;
        set_package_defaults(&mut cargo).map_err(error)?;
        resolve_dependencies(&mut cargo, path).map_err(error)?;

        let needs_packages = package_sections().any(|key| {
            cargo
                .get(key)
                .and_then(Value::as_table)
                .is_some_and(|section| !section.is_empty())
        });
        let dependencies = table_or_insert(&mut cargo, "dependencies").map_err(error)?;
        let mut runtime_keys = dependencies
            .iter()
            .filter(|(key, value)| {
                value.get("package").and_then(Value::as_str).unwrap_or(key) == "codingpad"
            })
            .map(|(key, _)| key.as_str());
        let runtime_name = runtime_keys.next().unwrap_or("codingpad").to_owned();
        if let Some(duplicate) = runtime_keys.next() {
            return Err(error(ManifestIssue::new(
                &["dependencies", duplicate],
                "Declare the codingpad runtime library once.",
            )));
        }
        if syn::parse_str::<syn::Ident>(&runtime_name.replace('-', "_")).is_err() {
            return Err(error(ManifestIssue::new(
                &["dependencies", &runtime_name],
                "Use a Rust identifier for the codingpad dependency name.",
            )));
        }
        cargo.insert("workspace".to_owned(), Value::Table(Table::new()));

        let mut dependency_line = line_at(source, frontmatter.start);
        let mut dependency_lines = Vec::new();
        if let Some(manifest) = needs_packages
            .then(|| toml_edit::Document::parse(embedded).ok())
            .flatten()
        {
            let table = manifest.as_table();
            if let Some(first) = package_sections()
                .filter_map(|section| field_offset(table, &[section.to_owned()]))
                .min()
            {
                dependency_line = line_at(source, body + first);
            }
            for &section in DEPENDENCY_SECTIONS {
                if let Some(dependencies) = cargo.get(section).and_then(Value::as_table) {
                    for (key, spec) in dependencies {
                        let package = spec.get("package").and_then(Value::as_str).unwrap_or(key);
                        if let Some(offset) =
                            field_offset(table, &[section.to_owned(), key.to_owned()])
                        {
                            dependency_lines
                                .push((package.to_owned(), line_at(source, body + offset)));
                        }
                    }
                }
            }
        }
        Ok(Self {
            cargo,
            frontmatter,
            needs_packages,
            runtime_name,
            dependency_line,
            dependency_lines,
        })
    }
    pub(super) fn dependency_error_line(&self, output: &str) -> usize {
        for line in output.lines() {
            for prefix in [
                "error: no matching package named `",
                "error: failed to get `",
                "error: failed to load manifest for dependency `",
                "error: failed to select a version for the requirement `",
            ] {
                if let Some(name) = line
                    .trim_start()
                    .strip_prefix(prefix)
                    .and_then(|text| text.split_once('`'))
                    .map(|(name, _)| name.split_whitespace().next().unwrap_or(name))
                {
                    if let Some((_, line)) = self
                        .dependency_lines
                        .iter()
                        .find(|(dependency, _)| dependency == name)
                    {
                        return *line;
                    }
                }
            }
        }
        self.dependency_line
    }
    pub(super) fn for_project(&self, runtime: &Path, binary: &str) -> Result<String, String> {
        let runtime = runtime
            .to_str()
            .ok_or("The Rust runtime path is not valid UTF-8.")?;
        let mut cargo = self.cargo.clone();
        table_or_insert(&mut cargo, "dependencies")
            .map_err(|error| error.to_string())?
            .insert(
                self.runtime_name.clone(),
                runtime_dependency(&self.runtime_name, runtime),
            );
        let package = table_or_insert(&mut cargo, "package").map_err(|error| error.to_string())?;
        package.insert("autobins".to_owned(), Value::Boolean(false));
        cargo.insert(
            "bin".to_owned(),
            Value::Array(vec![Value::Table(Table::from_iter([
                ("name".to_owned(), Value::from(binary)),
                ("path".to_owned(), Value::from(super::source::FILE)),
            ]))]),
        );
        Ok(cargo.to_string())
    }
}

fn line_at(source: &str, offset: usize) -> usize {
    source.as_bytes()[..offset]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + 1
}

fn package_sections() -> impl Iterator<Item = &'static str> {
    DEPENDENCY_SECTIONS
        .iter()
        .copied()
        .chain(["target", "patch", "replace"])
}

fn runtime_dependency(name: &str, path: &str) -> Value {
    let mut dependency = Table::from_iter([("path".to_owned(), Value::from(path))]);
    if name != "codingpad" {
        dependency.insert("package".to_owned(), Value::from("codingpad"));
    }
    Value::Table(dependency)
}

struct ManifestIssue {
    field: Vec<String>,
    message: String,
}

impl ManifestIssue {
    fn new(field: &[&str], message: impl Into<String>) -> Self {
        Self {
            field: field.iter().map(|key| (*key).to_owned()).collect(),
            message: message.into(),
        }
    }

    fn prepend(mut self, path: &[&str]) -> Self {
        self.field
            .splice(..0, path.iter().map(|key| (*key).to_owned()));
        self
    }
}

impl std::fmt::Display for ManifestIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn field_offset(mut table: &dyn toml_edit::TableLike, field: &[String]) -> Option<usize> {
    let mut path = field.iter().peekable();
    while let Some(part) = path.next() {
        let (key, item) = table.get_key_value(part)?;
        if path.peek().is_none() {
            return key.span().or_else(|| item.span()).map(|span| span.start);
        }
        table = item.as_table_like()?;
    }
    None
}

fn set_package_defaults(cargo: &mut Table) -> Result<(), ManifestIssue> {
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
    reject_fields(cargo, ROOT_FIELDS, &[])?;
    let package = table_or_insert(cargo, "package")?;
    reject_fields(package, PACKAGE_FIELDS, &["package"])?;
    for (key, value) in [
        ("name", "codingpad-script"),
        ("version", "0.0.0"),
        ("edition", "2024"),
    ] {
        package.entry(key).or_insert_with(|| Value::from(value));
    }
    Ok(())
}

fn reject_fields(table: &Table, fields: &[&str], path: &[&str]) -> Result<(), ManifestIssue> {
    if let Some(field) = fields.iter().find(|&&field| table.contains_key(field)) {
        let issue = ManifestIssue::new(&[field], "").prepend(path);
        return Err(ManifestIssue {
            message: format!(
                "{} is unavailable in single-file packages",
                issue.field.join(".")
            ),
            ..issue
        });
    }
    Ok(())
}

fn resolve_dependencies(cargo: &mut Table, path: Option<&Path>) -> Result<(), ManifestIssue> {
    let script = path.map(std::path::absolute).transpose().map_err(|error| {
        ManifestIssue::new(&[], format!("Could not resolve the script path: {error}"))
    })?;
    let directory = script.as_deref().and_then(Path::parent);
    dependency_paths(cargo, directory)?;
    if let Some(targets) = cargo.get_mut("target").and_then(Value::as_table_mut) {
        for (name, target) in targets
            .iter_mut()
            .filter_map(|(key, value)| value.as_table_mut().map(|table| (key, table)))
        {
            reject_fields(
                target,
                &["build-dependencies", "build_dependencies"],
                &["target", name],
            )?;
            dependency_paths(target, directory)
                .map_err(|error| error.prepend(&["target", name]))?;
        }
    }
    if let Some(patch) = cargo.get_mut("patch").and_then(Value::as_table_mut) {
        for (name, registry) in patch
            .iter_mut()
            .filter_map(|(key, value)| value.as_table_mut().map(|table| (key, table)))
        {
            make_paths_absolute(registry, directory)
                .map_err(|error| error.prepend(&["patch", name]))?;
        }
    }
    if let Some(replace) = cargo.get_mut("replace").and_then(Value::as_table_mut) {
        make_paths_absolute(replace, directory).map_err(|error| error.prepend(&["replace"]))?;
    }
    Ok(())
}

fn table_or_insert<'a>(root: &'a mut Table, key: &str) -> Result<&'a mut Table, ManifestIssue> {
    root.entry(key)
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| ManifestIssue::new(&[key], format!("{key} must be a table")))
}

fn dependency_paths(root: &mut Table, directory: Option<&Path>) -> Result<(), ManifestIssue> {
    for &key in DEPENDENCY_SECTIONS {
        if let Some(dependencies) = root.get_mut(key).and_then(Value::as_table_mut) {
            make_paths_absolute(dependencies, directory).map_err(|error| error.prepend(&[key]))?;
        }
    }
    Ok(())
}

fn make_paths_absolute(
    dependencies: &mut Table,
    directory: Option<&Path>,
) -> Result<(), ManifestIssue> {
    for (name, dependency) in dependencies {
        let Some(value) = dependency.get_mut("path") else {
            continue;
        };
        let Some(path) = value.as_str().map(Path::new) else {
            return Err(ManifestIssue::new(
                &[name, "path"],
                format!("{name}.path must be a string"),
            ));
        };
        if path.is_relative() {
            let directory = directory.ok_or_else(|| {
                ManifestIssue::new(
                    &[name, "path"],
                    format!(
                        "Save the Rust script before using a relative path for dependency {name}."
                    ),
                )
            })?;
            *value = Value::from(directory.join(path).to_str().ok_or_else(|| {
                ManifestIssue::new(
                    &[name, "path"],
                    format!("The path for Rust dependency {name} is not valid UTF-8."),
                )
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
    fn points_to_the_rejected_key_and_invalid_path() {
        let error = Manifest::parse("---cargo\n[package]\n'default-run' = 'demo'\n---", None)
            .err()
            .unwrap();
        assert_eq!(error.line, 3);
        let error = Manifest::parse(
            "---cargo\n[dependencies]\nthing = { path = 'relative' }\n---",
            None,
        )
        .err()
        .unwrap();
        assert_eq!(error.line, 3);
    }

    #[test]
    fn underscored_dev_dependencies_require_package_resolution() {
        let manifest =
            Manifest::parse("---cargo\n[dev_dependencies]\nserde = \"1\"\n---", None).unwrap();
        assert!(manifest.needs_packages);
    }

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
