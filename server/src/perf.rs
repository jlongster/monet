use std::{
    fs::{self, OpenOptions},
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

pub fn log(event: &str, fields: Value) {
    let path =
        std::env::var("MONET_PERF_LOG").unwrap_or_else(|_| ".money-tui/server-perf.jsonl".into());
    if let Some(parent) = std::path::Path::new(&path).parent() {
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
            "event": event,
            "fields": fields,
        })
    );
}
