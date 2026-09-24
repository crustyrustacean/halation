// src/template/tera.rs

use crate::template::{TemplateError, TemplateRenderer};
use anyhow::Context;
use tera::{Context as TeraContext, Tera};
pub struct TeraRenderer {
    engine: Tera,
    /// Site-wide settings every template sees (e.g. whether the Register
    /// nav link renders), injected here so no handler has to remember it.
    site: serde_json::Map<String, serde_json::Value>,
}

impl TeraRenderer {
    pub fn new() -> Result<Self, TemplateError> {
        Self::with_site(serde_json::Map::new())
    }

    /// Build a renderer that injects `site` into every render context.
    /// Handlers stay free of site-wide flags; templates read them as
    /// `{{ site.registration_open }}`.
    pub fn with_site(
        site: serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, TemplateError> {
        let mut engine = Tera::default();
        engine
            .load_from_glob("templates/**/*.html")
            .context("Unable to load the templates")?;

        Ok(Self { engine, site })
    }

    pub fn engine(&self) -> &Tera {
        &self.engine
    }
}

impl TemplateRenderer for TeraRenderer {
    fn render(
        &self,
        template_name: &str,
        context: &serde_json::Value,
    ) -> Result<String, TemplateError> {
        if !self.engine.contains_template(template_name) {
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

        let output = self
            .engine
            .render(template_name, &template_context)
            .context("Tera engine failed to render template")?;

        Ok(output)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

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
}
