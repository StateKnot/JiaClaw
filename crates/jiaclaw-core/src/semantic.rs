// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Explicit operator configuration for one immutable semantic embedding space.

use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::JiaClawError;

/// Optional semantic retrieval. Enabling this permits billable embedding requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SemanticMemoryConfig {
    /// Explicit consent to use the configured gateway for memory embeddings.
    pub enabled: bool,
    /// Exact logical embedding model configured in Brokerrouter.
    pub model: String,
    /// Operator-maintained embedding-space revision, changed when semantics change.
    pub space_revision: String,
    /// Exact configured vector dimension, between 1 and 3072.
    pub dimensions: usize,
    /// Up to three workspace-relative text files; empty inherits `memory.path`.
    pub sources: Vec<String>,
    /// Private index database path, resolved relative to the workspace.
    pub index_path: PathBuf,
    /// Total deadline for each gateway request, between 1 and 120 seconds.
    pub timeout_secs: u64,
}

impl Default for SemanticMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: String::new(),
            space_revision: String::new(),
            dimensions: 0,
            sources: Vec::new(),
            index_path: PathBuf::from("../state/semantic/index.sqlite3"),
            timeout_secs: 30,
        }
    }
}

impl SemanticMemoryConfig {
    /// Validate enabled semantic-memory settings without reading files or credentials.
    ///
    /// # Errors
    /// Returns a configuration error for invalid names, limits, or source paths.
    pub fn validate(&self) -> Result<(), JiaClawError> {
        if !self.enabled {
            return Ok(());
        }
        for (name, value) in [
            ("model", &self.model),
            ("space_revision", &self.space_revision),
        ] {
            if value.is_empty()
                || value.len() > 200
                || value.trim() != value
                || value.chars().any(char::is_control)
            {
                return Err(invalid(format!("memory.semantic.{name} requires 1..=200 UTF-8 bytes without controls or boundary whitespace")));
            }
        }
        if !(1..=3072).contains(&self.dimensions) {
            return Err(invalid(
                "memory.semantic.dimensions must be between 1 and 3072",
            ));
        }
        if !(1..=120).contains(&self.timeout_secs) {
            return Err(invalid(
                "memory.semantic.timeout_secs must be between 1 and 120",
            ));
        }
        if self.sources.len() > 3 {
            return Err(invalid(
                "memory.semantic.sources accepts at most three files",
            ));
        }
        let mut paths = HashSet::new();
        for source in &self.sources {
            let normalized = validate_source(source)?;
            if !paths.insert(normalized) {
                return Err(invalid("memory.semantic.sources contains duplicate paths"));
            }
        }
        let Some(index_path) = self.index_path.to_str() else {
            return Err(invalid("memory.semantic.index_path must be a UTF-8 path"));
        };
        if index_path.is_empty()
            || index_path.len() > 4096
            || index_path.trim() != index_path
            || index_path.chars().any(char::is_control)
        {
            return Err(invalid("memory.semantic.index_path must be a nonempty bounded path without controls or boundary whitespace"));
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> JiaClawError {
    JiaClawError::Configuration(message.into())
}

fn validate_source(source: &str) -> Result<PathBuf, JiaClawError> {
    if source.is_empty()
        || source.len() > 1024
        || source.trim() != source
        || source.chars().any(char::is_control)
    {
        return Err(invalid(
            "memory.semantic.sources requires bounded workspace-relative file paths",
        ));
    }
    let mut normalized = PathBuf::new();
    let mut count = 0;
    for component in Path::new(source).components() {
        match component {
            Component::Normal(name) => {
                normalized.push(name);
                count += 1;
            }
            Component::CurDir => {}
            _ => {
                return Err(invalid(
                    "memory.semantic.sources must stay relative to the workspace",
                ))
            }
        }
    }
    if count == 0 || count > 64 {
        return Err(invalid(
            "memory.semantic.sources requires 1..=64 path components",
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryConfig;

    fn enabled() -> SemanticMemoryConfig {
        SemanticMemoryConfig {
            enabled: true,
            model: "embedding/model@v1".into(),
            space_revision: "operator-v1".into(),
            dimensions: 3,
            ..SemanticMemoryConfig::default()
        }
    }

    #[test]
    fn semantic_configuration_is_opt_in_and_strict() {
        let legacy: MemoryConfig = toml::from_str("path = 'notes.md'\n").unwrap();
        assert!(!legacy.semantic.enabled);
        assert_eq!(legacy.semantic, SemanticMemoryConfig::default());
        assert_eq!(
            legacy.semantic.index_path,
            PathBuf::from("../state/semantic/index.sqlite3")
        );
        assert!(legacy.semantic.validate().is_ok());
        assert!(serde_json::from_str::<SemanticMemoryConfig>(r#"{"enable":true}"#).is_err());
        assert!(enabled().validate().is_ok());
    }

    #[test]
    fn semantic_names_and_resource_limits_are_checked() {
        for name in ["", " model", "model ", "model\n", &"x".repeat(201)] {
            let mut config = enabled();
            config.model = name.into();
            assert!(config.validate().is_err());
            config.model = "valid".into();
            config.space_revision = name.into();
            assert!(config.validate().is_err());
        }
        for dimensions in [0, 3073, usize::MAX] {
            let mut config = enabled();
            config.dimensions = dimensions;
            assert!(config.validate().is_err());
        }
        for timeout in [0, 121, u64::MAX] {
            let mut config = enabled();
            config.timeout_secs = timeout;
            assert!(config.validate().is_err());
        }
        let mut config = enabled();
        config.dimensions = 3072;
        config.timeout_secs = 120;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn semantic_sources_are_bounded_relative_and_unique() {
        for source in [
            "",
            ".",
            "../outside",
            "/outside",
            "notes/../../outside",
            " notes",
            "note\0",
            &"x".repeat(1025),
            &vec!["a"; 65].join("/"),
        ] {
            let mut config = enabled();
            config.sources = vec![source.into()];
            assert!(config.validate().is_err(), "invalid source accepted");
        }
        let mut config = enabled();
        config.sources = vec!["notes.md".into(), "./notes.md".into()];
        assert!(config.validate().is_err());
        config.sources = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        assert!(config.validate().is_err());
        config.sources = vec!["memory/long.md".into(), "SOUL.md".into(), "USER.md".into()];
        assert!(config.validate().is_ok());
        config.index_path = PathBuf::new();
        assert!(config.validate().is_err());
    }
}
