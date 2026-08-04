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
    InferenceCapabilities, InferenceConfig, InferenceGenerateInput, InferenceGenerateOutput,
    InferenceModels, InferencePluginInfo, InferenceStatus,
};

const PROTOCOL_VERSION: u32 = 1;
const MAX_MESSAGE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("inference is not configured")]
    NotConfigured,
    #[error("inference command must contain an executable")]
    InvalidCommand,
    #[error("failed to launch inference plugin: {0}")]
    Launch(#[source] std::io::Error),
    #[error("inference plugin I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("inference plugin returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("inference plugin request timed out")]
    Timeout,
    #[error("inference plugin exited unexpectedly")]
    Exited,
    #[error("inference plugin message exceeded {MAX_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
    #[error("inference plugin protocol error: {0}")]
    Protocol(String),
    #[error("inference plugin error {code}: {message}")]
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
    plugin: InferencePluginInfo,
    capabilities: InferenceCapabilities,
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
    config: InferenceConfig,
    initialize: InitializeResult,
    next_id: u64,
}

impl PluginProcess {
    async fn start(config: InferenceConfig) -> Result<Self, InferenceError> {
        let (executable, arguments) = config
            .command
            .split_first()
            .ok_or(InferenceError::InvalidCommand)?;
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

        let mut child = command.spawn().map_err(InferenceError::Launch)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| InferenceError::Protocol("plugin stdin was not available".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| InferenceError::Protocol("plugin stdout was not available".into()))?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("inference plugin: {line}");
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
                plugin: InferencePluginInfo {
                    name: "unknown".into(),
                    version: "unknown".into(),
                },
                capabilities: InferenceCapabilities {
                    inference: false,
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
        .map_err(|_| InferenceError::Timeout)??;
        if initialize.protocol_version != PROTOCOL_VERSION {
            return Err(InferenceError::Protocol(format!(
                "unsupported negotiated version {}",
                initialize.protocol_version
            )));
        }
        if !initialize.capabilities.inference {
            return Err(InferenceError::Protocol(
                "plugin does not provide inference capability".into(),
            ));
        }
        process.initialize = initialize;
        Ok(process)
    }

    async fn request<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, InferenceError> {
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
            let line = self
                .stdout
                .next_line()
                .await?
                .ok_or(InferenceError::Exited)?;
            if line.len() > MAX_MESSAGE_BYTES {
                return Err(InferenceError::MessageTooLarge);
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
                return Err(InferenceError::Remote {
                    code: error.code,
                    message: error.message,
                    data: error.data,
                });
            }
            let result = response.result.ok_or_else(|| {
                InferenceError::Protocol("response contained neither result nor error".into())
            })?;
            return serde_json::from_value(result).map_err(InferenceError::from);
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

#[derive(Default)]
pub struct InferenceManager {
    process: Mutex<Option<PluginProcess>>,
}

impl InferenceManager {
    async fn with_process<T: DeserializeOwned>(
        &self,
        config: InferenceConfig,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<(T, InitializeResult), InferenceError> {
        let mut process = self.process.lock().await;
        let needs_restart = process.as_ref().is_some_and(|running| {
            let mut running_config = running.config.clone();
            running_config.model = config.model.clone();
            running_config != config
        });
        if needs_restart && let Some(mut running) = process.take() {
            running.stop().await;
        }
        if let Some(running) = process.as_mut() {
            running.config = config.clone();
        }
        if process.is_none() {
            *process = Some(PluginProcess::start(config).await?);
        }
        let running = process.as_mut().expect("plugin was initialized");
        let initialize = running.initialize.clone();
        let result = timeout(
            Duration::from_millis(timeout_ms),
            running.request(method, params),
        )
        .await
        .map_err(|_| InferenceError::Timeout)?;
        match result {
            Ok(value) => Ok((value, initialize)),
            Err(error) => {
                let _ = running.child.kill().await;
                *process = None;
                Err(error)
            }
        }
    }

    pub async fn status(&self, config: InferenceConfig) -> Result<InferenceStatus, InferenceError> {
        let timeout_ms = config.request_timeout_ms;
        let (mut status, initialize) = self
            .with_process::<InferenceStatus>(config, "inference/status", json!({}), timeout_ms)
            .await?;
        status.plugin = Some(initialize.plugin);
        status.capabilities = Some(initialize.capabilities);
        Ok(status)
    }

    pub async fn models(&self, config: InferenceConfig) -> Result<InferenceModels, InferenceError> {
        let timeout_ms = config.request_timeout_ms;
        self.with_process(config, "inference/models", json!({}), timeout_ms)
            .await
            .map(|(models, _)| models)
    }

    pub async fn generate(
        &self,
        config: InferenceConfig,
        mut input: InferenceGenerateInput,
    ) -> Result<InferenceGenerateOutput, InferenceError> {
        if input.prompt.trim().is_empty() {
            return Err(InferenceError::Protocol("prompt must not be empty".into()));
        }
        if input.model.is_none() {
            input.model = config.model.clone();
        }
        let timeout_ms = input.timeout_ms.unwrap_or(config.request_timeout_ms);
        let params = serde_json::to_value(input)?;
        self.with_process(config, "inference/generate", params, timeout_ms)
            .await
            .map(|(response, _)| response)
    }

    pub async fn restart(&self) {
        let mut process = self.process.lock().await;
        if let Some(mut running) = process.take() {
            running.stop().await;
        }
    }
}
