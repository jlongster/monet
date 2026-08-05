use std::{env, process::Stdio, time::Duration};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
    time::timeout,
};

use crate::model::{
    ImportPluginConfig, ImportTransactionsInput, ImportedAccounts, ImportedTransactions,
    InferenceConfig, InferenceGenerateInput, InferenceGenerateOutput, InferenceModels,
    PluginCapabilities, PluginConfig, PluginInfo, PluginStatus,
};

const PROTOCOL_VERSION: u32 = 1;
const MAX_MESSAGE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin is not configured")]
    NotConfigured,
    #[error("plugin command must contain an executable")]
    InvalidCommand,
    #[error("failed to launch plugin: {0}")]
    Launch(#[source] std::io::Error),
    #[error("plugin I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plugin request timed out")]
    Timeout,
    #[error("plugin exited unexpectedly")]
    Exited,
    #[error("plugin message exceeded {MAX_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
    #[error("plugin protocol error: {0}")]
    Protocol(String),
    #[error("plugin error {code}: {message}")]
    Remote {
        code: i64,
        message: String,
        data: Option<Value>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    protocol_version: u32,
    plugin: PluginInfo,
    capabilities: PluginCapabilities,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    id: Value,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    id: String,
    method: &'a str,
    params: Value,
}

struct PluginProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    config: PluginConfig,
    initialize: InitializeResult,
    next_id: u64,
}

impl PluginProcess {
    async fn start(config: PluginConfig) -> Result<Self, PluginError> {
        let (executable, arguments) = config
            .command
            .split_first()
            .ok_or(PluginError::InvalidCommand)?;
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear();
        for name in ["PATH", "HOME", "TMPDIR", "TEMP", "TMP"] {
            if let Ok(value) = env::var(name) {
                command.env(name, value);
            }
        }
        for name in &config.env {
            if let Ok(value) = env::var(name) {
                command.env(name, value);
            }
        }
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }

        let mut child = command.spawn().map_err(PluginError::Launch)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PluginError::Protocol("plugin stdin was not available".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginError::Protocol("plugin stdout was not available".into()))?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    crate::app_log::write("plugin", &line);
                    eprintln!("plugin: {line}");
                }
            });
        }

        let mut process = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            config: config.clone(),
            initialize: InitializeResult {
                protocol_version: 0,
                plugin: PluginInfo {
                    name: "unknown".into(),
                    version: "unknown".into(),
                },
                capabilities: PluginCapabilities {
                    inference: false,
                    transaction_import: false,
                    structured_output: false,
                    cancellation: false,
                },
            },
            next_id: 1,
        };
        let initialize = timeout(
            Duration::from_millis(config.initialize_timeout_ms),
            process.request::<InitializeResult>(
                "initialize",
                json!({
                    "protocolVersions": [PROTOCOL_VERSION],
                    "host": { "name": "monet", "version": env!("CARGO_PKG_VERSION") },
                    "config": config.config,
                }),
            ),
        )
        .await
        .map_err(|_| PluginError::Timeout)??;
        if initialize.protocol_version != PROTOCOL_VERSION {
            return Err(PluginError::Protocol(format!(
                "unsupported negotiated version {}",
                initialize.protocol_version
            )));
        }
        process.initialize = initialize;
        Ok(process)
    }

    async fn request<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, PluginError> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let message = serde_json::to_string(&JsonRpcRequest {
            jsonrpc: "2.0",
            id: id.clone(),
            method,
            params,
        })?;
        self.stdin.write_all(message.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        loop {
            let line = self.stdout.next_line().await?.ok_or(PluginError::Exited)?;
            if line.len() > MAX_MESSAGE_BYTES {
                return Err(PluginError::MessageTooLarge);
            }
            let value: Value = serde_json::from_str(&line)?;
            if value.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                continue;
            }
            let response: JsonRpcResponse = serde_json::from_value(value)?;
            if response.id != Value::String(id.clone()) {
                continue;
            }
            if let Some(error) = response.error {
                return Err(PluginError::Remote {
                    code: error.code,
                    message: error.message,
                    data: error.data,
                });
            }
            let result = response.result.ok_or_else(|| {
                PluginError::Protocol("response contained neither result nor error".into())
            })?;
            return serde_json::from_value(result).map_err(PluginError::from);
        }
    }

    async fn stop(&mut self) {
        let _ = timeout(
            Duration::from_secs(1),
            self.request::<Value>("shutdown", json!({})),
        )
        .await;
        let _ = self.child.kill().await;
    }
}

#[derive(Clone, Copy)]
enum Capability {
    Inference,
    TransactionImport,
}

#[derive(Default)]
struct PluginManager {
    process: Mutex<Option<PluginProcess>>,
}

impl PluginManager {
    async fn with_process<T: DeserializeOwned>(
        &self,
        config: PluginConfig,
        capability: Capability,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<(T, InitializeResult), PluginError> {
        let mut process = self.process.lock().await;
        let needs_restart = process
            .as_ref()
            .is_some_and(|running| running.config != config);
        if needs_restart && let Some(mut running) = process.take() {
            running.stop().await;
        }
        if process.is_none() {
            *process = Some(PluginProcess::start(config).await?);
        }
        let running = process.as_mut().expect("plugin was initialized");
        let initialize = running.initialize.clone();
        let supported = match capability {
            Capability::Inference => initialize.capabilities.inference,
            Capability::TransactionImport => initialize.capabilities.transaction_import,
        };
        if !supported {
            return Err(PluginError::Protocol(format!(
                "plugin does not provide {} capability",
                match capability {
                    Capability::Inference => "inference",
                    Capability::TransactionImport => "transaction import",
                }
            )));
        }
        let result = timeout(
            Duration::from_millis(timeout_ms),
            running.request(method, params),
        )
        .await
        .map_err(|_| PluginError::Timeout)?;
        match result {
            Ok(value) => Ok((value, initialize)),
            Err(error) => {
                let _ = running.child.kill().await;
                *process = None;
                Err(error)
            }
        }
    }

    async fn status(
        &self,
        config: PluginConfig,
        capability: Capability,
        method: &str,
    ) -> Result<PluginStatus, PluginError> {
        let timeout_ms = config.request_timeout_ms;
        let (mut status, initialize) = self
            .with_process::<PluginStatus>(config, capability, method, json!({}), timeout_ms)
            .await?;
        status.plugin = Some(initialize.plugin);
        status.capabilities = Some(initialize.capabilities);
        Ok(status)
    }

    async fn restart(&self) {
        let mut process = self.process.lock().await;
        if let Some(mut running) = process.take() {
            running.stop().await;
        }
    }
}

#[derive(Default)]
pub struct InferenceManager {
    plugin: PluginManager,
}

impl InferenceManager {
    pub async fn status(&self, config: InferenceConfig) -> Result<PluginStatus, PluginError> {
        self.plugin
            .status(config.plugin, Capability::Inference, "inference/status")
            .await
    }

    pub async fn models(&self, config: InferenceConfig) -> Result<InferenceModels, PluginError> {
        let timeout_ms = config.plugin.request_timeout_ms;
        self.plugin
            .with_process(
                config.plugin,
                Capability::Inference,
                "inference/models",
                json!({}),
                timeout_ms,
            )
            .await
            .map(|(models, _)| models)
    }

    pub async fn generate(
        &self,
        config: InferenceConfig,
        mut input: InferenceGenerateInput,
    ) -> Result<InferenceGenerateOutput, PluginError> {
        if input.prompt.trim().is_empty() {
            return Err(PluginError::Protocol("prompt must not be empty".into()));
        }
        if input.model.is_none() {
            input.model = config.model.clone();
        }
        let request_id = uuid::Uuid::now_v7().to_string();
        crate::app_log::write_inference(&request_id, "request", &serde_json::to_value(&input)?);
        let timeout_ms = input.timeout_ms.unwrap_or(config.plugin.request_timeout_ms);
        let params = serde_json::to_value(input)?;
        let response = self
            .plugin
            .with_process(
                config.plugin,
                Capability::Inference,
                "inference/generate",
                params,
                timeout_ms,
            )
            .await
            .map(|(response, _)| response);
        match &response {
            Ok(output) => crate::app_log::write_inference(
                &request_id,
                "response",
                &serde_json::to_value(output)?,
            ),
            Err(error) => crate::app_log::write_inference(
                &request_id,
                "error",
                &json!({ "message": error.to_string() }),
            ),
        }
        response
    }

    pub async fn restart(&self) {
        self.plugin.restart().await;
    }
}

#[derive(Default)]
pub struct ImportManager {
    plugin: PluginManager,
}

impl ImportManager {
    pub async fn status(&self, config: ImportPluginConfig) -> Result<PluginStatus, PluginError> {
        self.plugin
            .status(
                config.plugin,
                Capability::TransactionImport,
                "import/status",
            )
            .await
    }

    pub async fn accounts(
        &self,
        config: ImportPluginConfig,
    ) -> Result<ImportedAccounts, PluginError> {
        let timeout_ms = config.plugin.request_timeout_ms;
        self.plugin
            .with_process(
                config.plugin,
                Capability::TransactionImport,
                "import/accounts",
                json!({}),
                timeout_ms,
            )
            .await
            .map(|(accounts, _)| accounts)
    }

    pub async fn transactions(
        &self,
        config: ImportPluginConfig,
        input: ImportTransactionsInput,
    ) -> Result<ImportedTransactions, PluginError> {
        let timeout_ms = config.plugin.request_timeout_ms;
        self.plugin
            .with_process(
                config.plugin,
                Capability::TransactionImport,
                "import/transactions",
                serde_json::to_value(input)?,
                timeout_ms,
            )
            .await
            .map(|(transactions, _)| transactions)
    }

    pub async fn restart(&self) {
        self.plugin.restart().await;
    }
}
