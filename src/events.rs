use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use time::macros::format_description;
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRecord {
    pub timestamp: String,
    pub run_id: String,
    pub task_id: Option<String>,
    pub kind: String,
    pub message: String,
}

pub fn utc_timestamp() -> String {
    let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub async fn append_event(path: &Path, lock: &Mutex<()>, event: &EventRecord) -> Result<()> {
    let _guard = lock.lock().await;
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?
        .write_all(line.as_bytes())
        .await?;
    Ok(())
}
