use std::{
    fs,
    fs::OpenOptions,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use serde_json::json;

pub fn write(component: &str, message: &str) {
    let path = std::env::var("MONET_LOG").unwrap_or_else(|_| ".money-tui/monet.log".into());
    if let Some(parent) = Path::new(&path).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(
        file,
        "{}",
        json!({
            "timestamp_ms": timestamp_ms,
            "component": component,
            "message": message,
        })
    );
}

pub fn write_inference(request_id: &str, direction: &str, payload: &Value) {
    let directory = std::env::var("MONET_INFERENCE_LOG_DIR")
        .unwrap_or_else(|_| ".money-tui/logs/inference".into());
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let (filename, contents) = if direction == "request" {
        let system = payload.get("system").and_then(Value::as_str).unwrap_or("");
        let prompt = payload.get("prompt").and_then(Value::as_str).unwrap_or("");
        let prompt = serde_json::from_str::<Value>(prompt)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .unwrap_or_else(|| prompt.to_owned());
        (
            format!("{request_id}_req.txt"),
            format!("--- SYSTEM PROMPT ---\n{system}\n\n--- PROMPT ---\n{prompt}"),
        )
    } else {
        let Ok(contents) = serde_json::to_string_pretty(payload) else {
            return;
        };
        (format!("{request_id}_res.json"), contents)
    };
    let path = Path::new(&directory).join(filename);
    let _ = fs::write(path, format!("{contents}\n"));
}
