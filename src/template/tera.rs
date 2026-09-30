// src/template/tera.rs

use crate::configuration::current_environment;
use crate::template::{TemplateError, TemplateRenderer};
use anyhow::Context;
use std::sync::RwLock;
use tera::{Context as TeraContext, Tera};

/// Where templates live, relative to the working directory. One constant so
/// the loader and the change-detector cannot disagree.
const TEMPLATE_ROOT: &str = "templates";
const TEMPLATE_GLOB: &str = "templates/**/*.html";
pub struct TeraRenderer {
    /// A lock rather than a plain `Tera` because development mode re-reads
    /// templates between renders, and `Tera::full_reload` needs `&mut self`.
    /// `render` is `&self` — it is shared across every request through
    /// `web::Data` — so the mutation has to be interior.
    engine: RwLock<Tera>,
    /// Site-wide settings every template sees (e.g. whether the Register
    /// nav link renders), injected here so no handler has to remember it.
    site: serde_json::Map<String, serde_json::Value>,
    /// Re-read templates whose mtime moved since the last render. Only ever
    /// true outside production; see `with_site`.
    reload: bool,
    /// mtime by path, so we can tell what actually changed.
    seen: RwLock<std::collections::HashMap<std::path::PathBuf, std::time::SystemTime>>,
    /// The root the templates were loaded from. Kept so the change check
    /// watches the same directory the engine reads, rather than a path
    /// repeated here that could drift away from it.
    root: std::path::PathBuf,
}

impl TeraRenderer {
    pub fn new() -> Result<Self, TemplateError> {
        Self::with_site(serde_json::Map::new())
    }

    /// Build a renderer that injects `site` into every render context.
    /// Handlers stay free of site-wide flags; templates read them as
    /// `{{ site.registration_open }}`.
    ///
    /// Outside production the engine re-reads any template whose mtime has
    /// moved, so editing a template takes effect on the next request. Without
    /// it, templates load exactly once at startup and an edit looks like it
    /// did nothing — which reads as a bug in whatever you just changed. It is
    /// off in production: it costs a stat per template per render, and
    /// nothing should be editing a deployed template.
    pub fn with_site(
        site: serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, TemplateError> {
        let mut engine = Tera::default();
        engine
            .load_from_glob(TEMPLATE_GLOB)
            .context("Unable to load the templates")?;
        // The glob is what makes `full_reload` possible later; keep it.
        let reload = current_environment() != "production";
        let root = std::path::PathBuf::from(TEMPLATE_ROOT);

        Ok(Self {
            engine: RwLock::new(engine),
            site,
            reload,
            seen: RwLock::new(std::collections::HashMap::new()),
            root,
        })
    }

    pub fn engine(&self) -> std::sync::RwLockReadGuard<'_, Tera> {
        self.engine.read().expect("template engine lock poisoned")
    }

    /// Re-read templates that have changed on disk. A no-op in production and
    /// whenever every mtime is unchanged, which is the overwhelmingly common
    /// case.
    fn reload_if_changed(&self) {
        if !self.reload {
            return;
        }

        let current = template_mtimes(&self.root);
        let mut seen = self.seen.write().expect("template mtime lock poisoned");
        if *seen == current {
            return; // nothing touched
        }
        *seen = current;

        if let Ok(mut engine) = self.engine.write() {
            // A failed reload must not take the site down: a half-written
            // template would otherwise turn every request into a 500 until
            // someone noticed. Keep the last good set and say so.
            if let Err(e) = engine.full_reload() {
                tracing::warn!(
                    error = %e,
                    "template reload failed; keeping the previous templates"
                );
            }
        }
    }
}

/// Every template's path paired with its mtime. Comparing two snapshots is how
/// we notice an edit, an addition, or a deletion.
fn template_mtimes(
    root: &std::path::Path,
) -> std::collections::HashMap<std::path::PathBuf, std::time::SystemTime> {
    fn walk(
        dir: &std::path::Path,
        out: &mut std::collections::HashMap<std::path::PathBuf, std::time::SystemTime>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
                continue;
            }
            if !path.extension().is_some_and(|e| e == "html") {
                continue;
            }
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                out.insert(path, modified);
            }
        }
    }

    let mut out = std::collections::HashMap::new();
    walk(root, &mut out);
    out
}

impl TemplateRenderer for TeraRenderer {
    fn render(
        &self,
        template_name: &str,
        context: &serde_json::Value,
    ) -> Result<String, TemplateError> {
        // Development mode only; a stat per template, and a no-op in
        // production. Done before the existence check so a newly added
        // template is usable without a restart.
        self.reload_if_changed();

        let engine = self.engine.read().expect("template engine lock poisoned");
        if !engine.contains_template(template_name) {
            return Err(TemplateError::NotFound(template_name.to_string()));
        }

        let mut template_context =
            TeraContext::from_serialize(context).context("Unable to build the page context")?;
        // Site-wide settings ride along on every render; templates guard
        // with `| default(value=true)` so bare `TeraRenderer::new()` (tests,
        // tooling) keeps rendering pages that mention the flag.
        if !self.site.is_empty() {
            template_context.insert("site", &self.site);
        }

        let output = engine
            .render(template_name, &template_context)
            .context("Tera engine failed to render template")?;

        Ok(output)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    /// `APP_ENVIRONMENT` is process-global, so every test that touches it
    /// must share one lock. A `static` *inside* each test would be a
    /// different lock each time and would exclude nobody.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn set_environment(value: &str) -> Option<String> {
        let previous = std::env::var("APP_ENVIRONMENT").ok();
        // SAFETY: ENV serialises the tests that write this variable.
        unsafe { std::env::set_var("APP_ENVIRONMENT", value) };
        previous
    }

    fn restore_environment(previous: Option<String>) {
        match previous {
            Some(v) => unsafe { std::env::set_var("APP_ENVIRONMENT", &v) },
            None => unsafe { std::env::remove_var("APP_ENVIRONMENT") },
        }
    }

    #[test]
    fn render_returns_404_not_found_for_missing_template() {
        // Arrange
        let renderer = TeraRenderer::new().unwrap();
        let template_name = "test";
        let context = serde_json::json!({});

        // Act
        let result = renderer.render(template_name, &context);

        // Assert
        assert!(matches!(result, Err(TemplateError::NotFound(_))));
    }

    #[test]
    fn mtime_snapshot_sees_every_template() {
        // A snapshot that misses files would make reload silently inert, and
        // an inert reload looks exactly like "my edit did nothing".
        let snapshot = template_mtimes(std::path::Path::new(TEMPLATE_ROOT));
        assert!(
            !snapshot.is_empty(),
            "expected templates/ to contain at least one .html file"
        );
        assert!(
            snapshot.keys().any(|p| p.ends_with("base.html")),
            "base.html should be in the snapshot: {snapshot:?}"
        );
    }

    #[test]
    fn mtime_snapshot_is_stable_when_nothing_changes() {
        // Two snapshots in a row must be equal, or every render would look
        // like a change and reload forever.
        assert_eq!(
            template_mtimes(std::path::Path::new(TEMPLATE_ROOT)),
            template_mtimes(std::path::Path::new(TEMPLATE_ROOT))
        );
    }

    /// The environment decides reload at construction time, and
    /// `APP_ENVIRONMENT` is process-global — so these two tests must not run
    /// concurrently or each reads the other's value. One test, both cases.
    #[test]
    fn reload_follows_the_environment() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let previous = set_environment("production");
        let in_production = TeraRenderer::new().unwrap().reload;

        set_environment("local");
        let in_local = TeraRenderer::new().unwrap().reload;

        restore_environment(previous);

        assert!(
            !in_production,
            "production must not re-read templates on the render path"
        );
        assert!(
            in_local,
            "local development should pick up template edits without a restart"
        );
    }

    /// The whole point of the feature: a template edited on disk shows up on
    /// the next render, with no restart. Writes to a temp file rather than
    /// editing a real template, so the test cannot corrupt the app.
    #[test]
    fn an_edited_template_is_picked_up_without_a_restart() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let previous = set_environment("local");

        let dir = std::env::temp_dir().join(format!("tera-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("probe.html");
        std::fs::write(&file, "first").unwrap();

        let mut engine = Tera::default();
        // Tera requires a glob, and the dir is joined on Windows with
        // backslashes, so build the pattern rather than passing the path.
        let pattern = dir.join("*.html");
        engine.load_from_glob(pattern.to_str().unwrap()).unwrap();
        let renderer = TeraRenderer {
            engine: RwLock::new(engine),
            site: serde_json::Map::new(),
            reload: true,
            seen: RwLock::new(template_mtimes(&dir)),
            root: dir.clone(),
        };

        let context = serde_json::json!({});
        assert_eq!(renderer.render("probe.html", &context).unwrap(), "first");

        // Rewrite the file. The mtime granularity here is coarse on some
        // filesystems, so sleep past it rather than risk a false pass.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&file, "second").unwrap();

        assert_eq!(
            renderer.render("probe.html", &context).unwrap(),
            "second",
            "an edited template must be re-read on the next render"
        );

        restore_environment(previous);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
