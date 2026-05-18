use eyre::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct Config {
    pub(crate) data_root: PathBuf,

    pub(crate) certificate: PathBuf,

    pub(crate) private_key: PathBuf,

    pub(crate) port: u16,
}

impl Config {
    pub(crate) fn new(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config = toml::from_str(&content)?;
        Ok(config)
    }

    pub(crate) fn user_dir(&self, token: &str) -> PathBuf {
        self.data_root.join(token)
    }
}
