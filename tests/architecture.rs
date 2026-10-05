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

/// The source with each `#[cfg(test)]` item blanked out, keeping line numbers. Test code may wire
/// in an adapter, but what follows it in the file still has to obey the layering, so only the
/// gated item (a `mod tests { .. }`, a `mod testing;`, an impl) is dropped, not the rest of the file.
fn without_test_code(source: &str) -> String {
    let mut kept = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "#[cfg(test)]" {
            kept.push(line);
            continue;
        }
        kept.push("");
        // The item that follows runs to its closing brace, or to its `;` if it has no body.
        let mut depth = 0i32;
        let mut opened = false;
        for item in lines.by_ref() {
            kept.push("");
            depth += item.matches('{').count() as i32 - item.matches('}').count() as i32;
            opened |= item.contains('{');
            if (opened && depth <= 0) || (!opened && item.trim_end().ends_with(';')) {
                break;
            }
        }
    }
    kept.join("\n")
}

/// The module after `crate::` in each import, ignoring test code and comments.
fn imports(source: &str) -> Vec<(usize, String)> {
    let production = without_test_code(source);
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

#[test]
fn code_after_an_early_test_item_is_still_checked() {
    // A `mod testing;` above the imports, a gated impl in the middle, and a test module at the end.
    let source = "#[cfg(test)]\nmod testing;\n\nuse crate::config::A;\n\n#[cfg(test)]\nimpl X {\n    fn f() { crate::adapters::B; }\n}\n\nuse crate::bootstrap::C;\n\n#[cfg(test)]\nmod tests {\n    use crate::cli::D;\n}\n";
    assert_eq!(imports(source), [(4, "config".to_owned()), (11, "bootstrap".to_owned())]);
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
            let production = without_test_code(&source);
            production.contains("schemars") || production.contains("JsonSchema") || production.contains("Deserialize")
        })
        .map(|file| file.strip_prefix(root).unwrap().display().to_string())
        .collect();
    assert!(offenders.is_empty(), "config derives in the domain: {offenders:?}");
}
