//! The page templates behind the layouts: the ones built in, and any the owner keeps in a directory.
//!
//! Each `NAME.html` in the directory is a layout called `NAME`, written in the same Handlebars as the built-in ones
//! and able to use the same partials (`style`, `header`, `departures`, and the weather icons) and the bundled fonts.
//! A file named for a built-in layout replaces it, which is how the dashboard is changed; any other name adds a layout,
//! after the built-in ones in name order. The default stays the first built-in.
//!
//! The directory is read once, when the program starts, and every template in it is drawn with sample data then: a
//! mistake in one stops the program with the file's name and what is wrong, not at the first render, hours later, with
//! a display waiting on it. Editing a template takes a restart.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use chrono::Utc;

use handlebars::Handlebars;

use super::{LAYOUTS, registry, render_page, template_name};
use crate::domain::models::GlanceData;
use crate::domain::models::layout::{Layout, LayoutName, Layouts};

/// More than any template needs: a page is a few tens of kilobytes.
const LARGEST_TEMPLATE: u64 = 512 * 1024;
/// Every layout is drawn on every run, so each one costs a Chrome render and an encode of every format.
const MOST_LAYOUTS: usize = 16;
/// The longest description kept, so a stray `<title>` can't fill a table.
const LONGEST_DESCRIPTION: usize = 80;

#[derive(Clone)]
struct Entry {
    layout: Layout,
    source: Arc<str>,
}

/// The templates there are, by layout, the default first.
#[derive(Clone)]
pub struct Templates {
    entries: Vec<Entry>,
    /// Every template parsed once, so a render doesn't parse them all again.
    registry: Arc<Handlebars<'static>>,
}

impl Templates {
    /// The templates built in.
    pub fn built_in() -> Self {
        Self::of(
            LAYOUTS
                .iter()
                .map(|(name, description, source)| Entry {
                    layout: Layout::new(name, description),
                    source: Arc::from(*source),
                })
                .collect(),
        )
        .expect("the templates built in are valid")
    }

    /// These templates, parsed, or which one can't be.
    fn of(entries: Vec<Entry>) -> anyhow::Result<Self> {
        let registry = registry(
            entries
                .iter()
                .map(|entry| (entry.layout.name.as_str(), &*entry.source)),
        )?;
        Ok(Self {
            entries,
            registry: Arc::new(registry),
        })
    }

    /// These, with the templates in `directory` replacing the ones of the same name and adding the rest. An error says
    /// which file and what is wrong with it.
    pub fn with_directory(self, directory: &Path) -> anyhow::Result<Self> {
        let mut entries = self.entries;
        let names = std::fs::read_dir(directory).with_context(|| {
            format!(
                "Failed to read the layouts directory {}",
                directory.display()
            )
        })?;
        let mut found = Vec::new();
        for entry in names {
            let path = entry
                .with_context(|| format!("Failed to read {}", directory.display()))?
                .path();
            if path
                .extension()
                .is_some_and(|extension| extension == "html")
                && path.is_file()
            {
                found.push(path);
            }
        }
        found.sort();
        let mut added = Vec::new();
        for path in found {
            let file = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let stem = path
                .file_stem()
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
            let name = LayoutName::parse(&stem).map_err(|e| anyhow::anyhow!("{file}: {e}"))?;
            let size = std::fs::metadata(&path)
                .with_context(|| format!("{file}: failed to inspect it"))?
                .len();
            if size > LARGEST_TEMPLATE {
                bail!(
                    "{file}: it is {size} bytes, and a template can be at most {LARGEST_TEMPLATE}"
                );
            }
            let source = std::fs::read_to_string(&path)
                .with_context(|| format!("{file}: failed to read it (a template is UTF-8 text)"))?;
            check(&source).with_context(|| {
                format!("{file}: it can't be drawn (see the layouts section of deploy/README.md)")
            })?;
            let layout = Layout::new(
                name.as_str(),
                &description_of(&source).unwrap_or_else(|| "A layout of your own".to_owned()),
            );
            let entry = Entry {
                layout,
                source: Arc::from(source),
            };
            match entries
                .iter_mut()
                .find(|existing| existing.layout.name == name)
            {
                Some(existing) => {
                    tracing::info!("The layout {name} is replaced by {file}");
                    *existing = entry;
                }
                None => {
                    tracing::info!("Added the layout {name} from {file}");
                    added.push(entry);
                }
            }
        }
        entries.extend(added);
        if entries.len() > MOST_LAYOUTS {
            bail!(
                "There are {} layouts, and at most {MOST_LAYOUTS} are drawn: each costs a render on every run",
                entries.len()
            );
        }
        Self::of(entries)
    }

    /// The layouts, which are what the rest of the program knows of the templates.
    pub fn layouts(&self) -> Layouts {
        Layouts::new(self.entries.iter().map(|e| e.layout.clone()).collect())
            .expect("the names are unique: an entry with a name already there replaces it")
    }

    /// `data` laid out as `layout`.
    pub fn render(&self, layout: &LayoutName, data: &GlanceData) -> anyhow::Result<String> {
        anyhow::ensure!(
            self.entries
                .iter()
                .any(|entry| &entry.layout.name == layout),
            "There is no layout called {layout}"
        );
        Ok(self
            .registry
            .render(&template_name(layout.as_str()), data)?)
    }

    /// The template of the layout called `name`.
    #[cfg(test)]
    pub(super) fn source(&self, name: &LayoutName) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| &entry.layout.name == name)
            .map(|entry| &*entry.source)
    }
}

/// Draws `source` with data of each kind it will meet, and says why it can't if it can't.
fn check(source: &str) -> anyhow::Result<()> {
    let now = Utc::now().with_timezone(&chrono_tz::UTC);
    let none = GlanceData::new(None, vec![], crate::domain::models::DateInfo::new(now));
    for (what, data) in [
        ("with everything there", GlanceData::sample(now)),
        ("with sources down", GlanceData::sample_degraded(now)),
        ("with nothing to show", none),
    ] {
        render_page(source, &data).with_context(|| format!("drawing it {what}"))?;
    }
    Ok(())
}

/// What the page's `<title>` says, tidied: its description for whoever is choosing.
fn description_of(source: &str) -> Option<String> {
    let start = source.find("<title>")? + "<title>".len();
    let end = start + source[start..].find("</title>")?;
    let text: String = source[start..end]
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() || text.contains('{') {
        return None;
    }
    Some(text.chars().take(LONGEST_DESCRIPTION).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(directory.path().join(name), text).unwrap();
        }
        directory
    }

    fn names(templates: &Templates) -> Vec<String> {
        templates
            .layouts()
            .all()
            .iter()
            .map(|layout| layout.name.to_string())
            .collect()
    }

    #[test]
    fn the_built_in_templates_pass_their_own_check() {
        for (name, _, source) in LAYOUTS {
            check(source).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        }
        assert_eq!(names(&Templates::built_in()), ["dashboard", "departures"]);
    }

    #[test]
    fn a_template_in_the_directory_adds_a_layout_after_the_built_in_ones() {
        let directory = directory_with(&[
            (
                "clock.html",
                "<html><title>Just the time</title><body>{{date.currentTime}}</body></html>",
            ),
            ("a-first.html", "<body>{{date.currentDay}}</body>"),
        ]);

        let templates = Templates::built_in()
            .with_directory(directory.path())
            .unwrap();

        // In name order after the built-in ones, and the default is still the first of those.
        assert_eq!(
            names(&templates),
            ["dashboard", "departures", "a-first", "clock"]
        );
        let layouts = templates.layouts();
        assert_eq!(layouts.default_layout().name.as_str(), "dashboard");
        assert_eq!(layouts.find("clock").unwrap().description, "Just the time");
        // With no title, a plain description.
        assert_eq!(
            layouts.find("a-first").unwrap().description,
            "A layout of your own"
        );
        assert!(
            templates
                .source(&LayoutName::parse("clock").unwrap())
                .unwrap()
                .contains("currentTime")
        );
    }

    #[test]
    fn a_template_named_for_a_built_in_layout_replaces_it_and_keeps_its_place() {
        let directory = directory_with(&[(
            "dashboard.html",
            "<title>My dashboard</title>{{> header}}{{> departures}}",
        )]);

        let templates = Templates::built_in()
            .with_directory(directory.path())
            .unwrap();

        assert_eq!(names(&templates), ["dashboard", "departures"]);
        let layouts = templates.layouts();
        assert_eq!(layouts.default_layout().description, "My dashboard");
        assert!(
            !templates
                .source(&LayoutName::parse("dashboard").unwrap())
                .unwrap()
                .contains("weather")
        );
    }

    #[test]
    fn files_that_are_not_templates_are_left_alone() {
        let directory = directory_with(&[
            ("notes.txt", "{{"),
            ("clock.html.bak", "{{"),
            ("ok.html", "<body>{{date.currentTime}}</body>"),
        ]);
        std::fs::create_dir(directory.path().join("sub.html")).unwrap();

        let templates = Templates::built_in()
            .with_directory(directory.path())
            .unwrap();

        assert_eq!(names(&templates), ["dashboard", "departures", "ok"]);
    }

    /// The error for a directory holding just this file.
    fn refused(file: &str, text: &str) -> String {
        let directory = directory_with(&[(file, text)]);
        let error = Templates::built_in()
            .with_directory(directory.path())
            .err()
            .unwrap_or_else(|| panic!("{file} should have been refused"));
        format!("{error:#}")
    }

    #[test]
    fn a_template_that_cannot_be_drawn_stops_the_start_and_names_the_file_and_the_fault() {
        // Not Handlebars.
        let broken = refused("broken.html", "<body>{{#if departures}}no end</body>");
        assert!(broken.contains("broken.html"), "{broken}");
        assert!(broken.contains("drawing it"), "{broken}");

        // A partial there is not.
        let missing = refused("missing.html", "<body>{{> nonesuch}}</body>");
        assert!(missing.contains("missing.html"), "{missing}");
        assert!(missing.contains("nonesuch"), "{missing}");
    }

    #[test]
    fn a_name_that_cannot_be_a_layouts_is_refused() {
        for file in ["Clock.html", "2fast.html", "a b.html", "x.y.html"] {
            let message = refused(file, "<body></body>");
            assert!(message.contains("layout name"), "{file}: {message}");
        }
    }

    #[test]
    fn a_template_too_large_to_be_one_is_refused_unread() {
        let big = "x".repeat(usize::try_from(LARGEST_TEMPLATE).unwrap() + 1);
        let message = refused("big.html", &big);
        assert!(
            message.contains("big.html") && message.contains("at most"),
            "{message}"
        );
    }

    #[test]
    fn a_template_that_is_not_text_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("bytes.html"), [0xff, 0xfe, 0x00]).unwrap();
        let error = Templates::built_in()
            .with_directory(directory.path())
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("bytes.html"), "{error:#}");
    }

    #[test]
    fn a_directory_that_is_not_there_is_an_error_naming_it() {
        let error = Templates::built_in()
            .with_directory(Path::new("/nonexistent/layouts"))
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("/nonexistent/layouts"));
    }

    #[test]
    fn too_many_layouts_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        for i in 0..MOST_LAYOUTS {
            std::fs::write(
                directory.path().join(format!("extra-{i:02}.html")),
                "<body></body>",
            )
            .unwrap();
        }
        let error = Templates::built_in()
            .with_directory(directory.path())
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("at most"), "{error:#}");
    }

    #[test]
    fn a_description_is_tidied_and_kept_short() {
        assert_eq!(
            description_of("<title>\n  Big   clock\t</title>").as_deref(),
            Some("Big clock")
        );
        assert_eq!(description_of("<title></title>"), None);
        assert_eq!(description_of("<title>{{x}}</title>"), None);
        assert_eq!(description_of("no title"), None);
        let long = format!("<title>{}</title>", "a".repeat(500));
        assert_eq!(description_of(&long).unwrap().len(), LONGEST_DESCRIPTION);
    }

    #[test]
    fn a_layout_may_be_called_what_a_partial_is_and_both_still_work() {
        // `departures` is a built-in layout and a partial; `header` and `style` are partials a layout of that name
        // would otherwise replace.
        let directory = directory_with(&[
            ("header.html", "<body>my header {{> header}}</body>"),
            ("style.html", "<body>my style {{> style}}</body>"),
        ]);

        let templates = Templates::built_in()
            .with_directory(directory.path())
            .unwrap();

        let data = GlanceData::sample(Utc::now().with_timezone(&chrono_tz::UTC));
        let page = |layout: &str| {
            templates
                .render(&LayoutName::parse(layout).unwrap(), &data)
                .unwrap()
        };
        // The built-in layouts still find their partials, including the one that shares a name with `departures`.
        for layout in ["dashboard", "departures"] {
            assert!(page(layout).contains("class=\"header\""), "{layout}");
            assert!(page(layout).contains("class=\"departures\""), "{layout}");
        }
        // And the layouts called after partials are their own pages, which use the real partials.
        assert!(page("header").contains("my header"));
        assert!(page("header").contains("class=\"header\""));
        assert!(page("style").contains("my style"));
        assert!(page("style").contains("<style>"));
        // Asking for one there isn't is an error, not an empty page.
        let none = templates.render(&LayoutName::parse("nonesuch").unwrap(), &data);
        assert!(none.is_err());
    }
}
