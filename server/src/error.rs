use crate::{config::ConfigError, db::DatabaseError, plugin::PluginError};

#[derive(Debug)]
pub(crate) enum MonetError {
    Database(DatabaseError),
    Plugin(PluginError),
    Config(ConfigError),
}

impl From<DatabaseError> for MonetError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<PluginError> for MonetError {
    fn from(error: PluginError) -> Self {
        Self::Plugin(error)
    }
}

impl From<ConfigError> for MonetError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}
