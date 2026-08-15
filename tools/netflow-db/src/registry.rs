//! Dataset registry loading and canonical nfcapd tree discovery.

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("unable to read dataset registry: {0}")]
    Io(#[from] std::io::Error),
    #[error("unable to parse dataset registry: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid dataset registry: {0}")]
    Invalid(String),
    #[error("unknown dataset {requested:?}; available datasets: {available}")]
    UnknownDataset {
        requested: String,
        available: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetSource {
    pub source_id: String,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dataset {
    pub dataset_id: String,
    #[serde(default)]
    pub label: String,
    pub root_path: PathBuf,
    pub db_path: PathBuf,
    #[serde(default)]
    pub default_start_date: String,
    #[serde(default = "default_source_mode")]
    pub source_mode: String,
    #[serde(default = "default_discovery_mode")]
    pub discovery_mode: String,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub sources: Vec<DatasetSource>,
}

impl Dataset {
    pub fn validate(&mut self, repository_root: &Path) -> Result<(), RegistryError> {
        self.dataset_id = self.dataset_id.trim().to_owned();
        if self.dataset_id.is_empty() {
            return Err(RegistryError::Invalid("dataset_id cannot be empty".into()));
        }
        if self.label.trim().is_empty() {
            self.label = title(&self.dataset_id);
        }
        self.root_path = expand_path(&self.root_path, repository_root)?;
        self.db_path = expand_path(&self.db_path, repository_root)?;
        if !matches!(self.source_mode.as_str(), "subdirs" | "static") {
            return Err(RegistryError::Invalid(format!(
                "unsupported source_mode {:?} for {:?}",
                self.source_mode, self.dataset_id
            )));
        }
        if !self.source_ids.is_empty() && !self.sources.is_empty() {
            return Err(RegistryError::Invalid(format!(
                "dataset {:?} cannot define both source_ids and sources",
                self.dataset_id
            )));
        }
        unique_nonempty(&self.source_ids, "source_ids")?;
        validate_path_components(&self.source_ids, "source_ids")?;
        let source_names = self
            .sources
            .iter()
            .map(|source| source.source_id.clone())
            .collect::<Vec<_>>();
        unique_nonempty(&source_names, "sources")?;
        validate_path_components(&source_names, "sources")?;
        for source in &self.sources {
            if source.members.is_empty() {
                return Err(RegistryError::Invalid(format!(
                    "source {:?} must define members",
                    source.source_id
                )));
            }
            unique_nonempty(&source.members, "source members")?;
            validate_path_components(&source.members, "source members")?;
        }
        Ok(())
    }

    pub fn logical_sources(&self) -> Result<Vec<DatasetSource>, RegistryError> {
        if !self.sources.is_empty() {
            return Ok(self.sources.clone());
        }
        if !self.source_ids.is_empty() {
            return Ok(self
                .source_ids
                .iter()
                .map(|source| DatasetSource {
                    source_id: source.clone(),
                    members: vec![source.clone()],
                })
                .collect());
        }
        if self.source_mode != "subdirs" || !self.root_path.exists() {
            return Ok(Vec::new());
        }
        let mut sources = Vec::new();
        for entry in fs::read_dir(&self.root_path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let source = entry.file_name().to_string_lossy().into_owned();
                if !is_safe_path_component(&source) {
                    return Err(RegistryError::Invalid(format!(
                        "discovered source {source:?} is not a safe path component"
                    )));
                }
                sources.push(DatasetSource {
                    source_id: source.clone(),
                    members: vec![source],
                });
            }
        }
        sources.sort_unstable_by(|left, right| left.source_id.cmp(&right.source_id));
        Ok(sources)
    }
}

#[derive(Clone, Debug)]
pub struct DatasetRegistry {
    pub datasets: Vec<Dataset>,
}

impl DatasetRegistry {
    pub fn load(path: impl AsRef<Path>, repository_root: &Path) -> Result<Self, RegistryError> {
        let bytes = fs::read(path)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        let entries = value.get("datasets").cloned().unwrap_or(value);
        let mut datasets: Vec<Dataset> = serde_json::from_value(entries)?;
        if datasets.is_empty() {
            return Err(RegistryError::Invalid(
                "dataset registry cannot be empty".into(),
            ));
        }
        for dataset in &mut datasets {
            dataset.validate(repository_root)?;
        }
        let ids = datasets
            .iter()
            .map(|dataset| dataset.dataset_id.clone())
            .collect::<Vec<_>>();
        unique_nonempty(&ids, "dataset IDs")?;
        Ok(Self { datasets })
    }

    pub fn load_default(repository_root: &Path) -> Result<Self, RegistryError> {
        let configured = env::var_os("DATASETS_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| repository_root.join("datasets.json"));
        Self::load(configured, repository_root)
    }

    pub fn get(&self, dataset_id: &str) -> Result<&Dataset, RegistryError> {
        self.datasets
            .iter()
            .find(|dataset| dataset.dataset_id == dataset_id)
            .ok_or_else(|| RegistryError::UnknownDataset {
                requested: dataset_id.to_owned(),
                available: self
                    .datasets
                    .iter()
                    .map(|dataset| dataset.dataset_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            })
    }
}

fn expand_path(path: &Path, repository_root: &Path) -> Result<PathBuf, RegistryError> {
    let path = if path.starts_with("~") {
        let home = env::var_os("HOME")
            .ok_or_else(|| RegistryError::Invalid("cannot expand ~ without HOME".into()))?;
        PathBuf::from(home).join(path.strip_prefix("~").expect("prefix checked"))
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        repository_root.join(path)
    };
    Ok(path)
}

fn unique_nonempty(values: &[String], name: &str) -> Result<(), RegistryError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            return Err(RegistryError::Invalid(format!(
                "{name} cannot contain an empty value"
            )));
        }
        if !seen.insert(value) {
            return Err(RegistryError::Invalid(format!(
                "{name} contains duplicate {value:?}"
            )));
        }
    }
    Ok(())
}

fn validate_path_components(values: &[String], name: &str) -> Result<(), RegistryError> {
    if let Some(value) = values.iter().find(|value| !is_safe_path_component(value)) {
        return Err(RegistryError::Invalid(format!(
            "{name} value {value:?} must be exactly one normal path component"
        )));
    }
    Ok(())
}

#[must_use]
pub fn is_safe_path_component(value: &str) -> bool {
    !value.trim().is_empty()
        && !value.contains(['/', '\\'])
        && matches!(
            Path::new(value).components().collect::<Vec<_>>().as_slice(),
            [std::path::Component::Normal(_)]
        )
}

fn title(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters
                .next()
                .map(|first| first.to_uppercase().chain(characters).collect())
                .unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn default_source_mode() -> String {
    "subdirs".into()
}

fn default_discovery_mode() -> String {
    "static".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn registry_supports_list_and_wrapped_shapes_and_resolves_relative_db() {
        let root = tempdir().unwrap();
        let list = root.path().join("datasets.json");
        fs::write(
            &list,
            r#"[{"dataset_id":"sample_data","root_path":"/captures","db_path":"data/netflow.sqlite","source_ids":["r1"]}]"#,
        )
        .unwrap();

        let registry = DatasetRegistry::load(&list, root.path()).unwrap();

        assert_eq!(registry.get("sample_data").unwrap().label, "Sample Data");
        assert_eq!(
            registry.get("sample_data").unwrap().db_path,
            root.path().join("data/netflow.sqlite")
        );
        assert_eq!(
            registry
                .get("sample_data")
                .unwrap()
                .logical_sources()
                .unwrap(),
            [DatasetSource {
                source_id: "r1".into(),
                members: vec!["r1".into()]
            }]
        );
    }
}
