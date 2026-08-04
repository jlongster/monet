use std::{env, fs, path::PathBuf};

use thiserror::Error;

use crate::model::{InferenceConfig, MonetConfig};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid config JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("inference is not configured")]
    InferenceNotConfigured,
}

#[derive(Clone)]
pub struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    pub fn open() -> Result<Self, ConfigError> {
        let path = env::var_os("MONET_CONFIG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config/monet/config.json")
            });
        Self::at(path)
    }

    pub fn at(path: PathBuf) -> Result<Self, ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            fs::write(
                &path,
                serde_json::to_string_pretty(&MonetConfig::default())? + "\n",
            )?;
        }
        let config = Self { path };
        config.load()?;
        Ok(config)
    }

    pub fn load(&self) -> Result<MonetConfig, ConfigError> {
        Ok(serde_json::from_str(&fs::read_to_string(&self.path)?)?)
    }

    pub fn set_inference_model(&self, model: String) -> Result<InferenceConfig, ConfigError> {
        let mut value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&self.path)?)?;
        let inference = value
            .get_mut("inference")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or(ConfigError::InferenceNotConfigured)?;
        inference.insert("model".into(), serde_json::Value::String(model));

        let config: MonetConfig = serde_json::from_value(value.clone())?;
        fs::write(&self.path, serde_json::to_string_pretty(&value)? + "\n")?;
        config.inference.ok_or(ConfigError::InferenceNotConfigured)
    }

    pub fn path(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}
