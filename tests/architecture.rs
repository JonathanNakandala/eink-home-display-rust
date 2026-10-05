//! Keeps the layers pointing inward: domain <- application <- adapters, with config and wiring
//! (`bootstrap`, `main`, the binaries) at the outside. Looks at what each non-test source file
//! imports, so a stray `use` fails here instead of eroding the structure unnoticed.

use std::fs;
use std::path::{Path, PathBuf};

/// A layer's directory, and the `crate::` modules it may not use.
const RULES: &[(&str, &[&str])] = &[
    ("src/domain", &["adapters", "application", "bootstrap", "config", "scheduler", "cli"]),
    ("src/application", &["adapters", "bootstrap", "config", "cli"]),
    ("src/adapters", &["bootstrap", "config", "cli"]),
];

/// `src/application.rs` is the root of the application layer, the same as its directory.
const LAYER_FILES: &[(&str, &str)] = &[("src/application.rs", "src/application")];

fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// The module after `crate::` in each import, ignoring test code (which may wire in an adapter)
/// and comments.
fn imports(source: &str) -> Vec<(usize, String)> {
    let production = source.split("#[cfg(test)]").next().unwrap_or(source);
    let mut found = Vec::new();
    for (number, line) in production.lines().enumerate() {
        let line = line.trim_start();
        if line.starts_with("//") {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("crate::") {
            let after = &rest[at + "crate::".len()..];
            let module: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            // `use crate::{a, b}` has no module name straight after the path.
            if !module.is_empty() {
                found.push((number + 1, module));
            }
            rest = after;
        }
    }
    found
}

#[test]
fn layers_only_depend_inward() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();
    for (layer, forbidden) in RULES {
        let mut files = Vec::new();
        sources(&root.join(layer), &mut files);
        files.extend(LAYER_FILES.iter().filter(|(_, l)| l == layer).map(|(f, _)| root.join(f)));
        for file in files {
            let source = fs::read_to_string(&file).unwrap();
            for (line, module) in imports(&source) {
                if forbidden.contains(&module.as_str()) {
                    let relative = file.strip_prefix(root).unwrap().display();
                    violations.push(format!("{relative}:{line} uses crate::{module}, which {layer} must not"));
                }
            }
        }
    }
    assert!(violations.is_empty(), "layering broken:\n{}", violations.join("\n"));
}

#[test]
fn the_check_sees_a_violation_and_ignores_tests_and_comments() {
    let source = "use crate::domain::X;\n// use crate::config::Y;\nuse crate::config::Z;\n#[cfg(test)]\nmod t { use crate::adapters::W; }\n";
    assert_eq!(imports(source), [(1, "domain".to_owned()), (3, "config".to_owned())]);
}

/// The domain describes the problem, not how it is configured: no config schema derives in it.
#[test]
fn the_domain_carries_no_config_schema() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src/domain"), &mut files);
    let offenders: Vec<_> = files
        .iter()
        .filter(|file| {
            let source = fs::read_to_string(file).unwrap();
            let production = source.split("#[cfg(test)]").next().unwrap_or(&source);
            production.contains("schemars") || production.contains("JsonSchema") || production.contains("Deserialize")
        })
        .map(|file| file.strip_prefix(root).unwrap().display().to_string())
        .collect();
    assert!(offenders.is_empty(), "config derives in the domain: {offenders:?}");
}
