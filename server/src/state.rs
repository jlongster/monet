use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::{
    config::ConfigFile,
    db::Database,
    plugin::{ImportManager, InferenceManager},
};

#[derive(Clone)]
pub(crate) struct MonetState {
    pub(crate) database: Arc<Mutex<Database>>,
    pub(crate) inference: Arc<InferenceManager>,
    pub(crate) importer: Arc<ImportManager>,
    pub(crate) config: Arc<ConfigFile>,
    pub(crate) import_log: Arc<Mutex<ImportLog>>,
}

impl MonetState {
    pub(crate) fn new(database: Database, config: ConfigFile) -> Self {
        Self {
            database: Arc::new(Mutex::new(database)),
            inference: Arc::new(InferenceManager::default()),
            importer: Arc::new(ImportManager::default()),
            config: Arc::new(config),
            import_log: Arc::new(Mutex::new(ImportLog::default())),
        }
    }
}

#[derive(Clone, Default, Serialize)]
pub(crate) struct ImportLog {
    pub(crate) running: bool,
    pub(crate) lines: Vec<String>,
}
