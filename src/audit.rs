use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use serde::Serialize;

use crate::builder::Intent;
use crate::loader::Provenance;

#[derive(Debug, Clone, Serialize)]
pub struct AuditRecord {
    pub kind: &'static str,
    pub ts: String,
    pub session_id: String,
    pub seq: u64,
    pub user: String,
    pub host: String,
    pub intent: IntentSnapshot,
    pub argv: Vec<String>,
    pub provenance: Provenance,
    pub reference: Option<u64>,
    pub result: Option<ResultSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntentSnapshot {
    pub service: String,
    pub operation: String,
    pub params: Vec<(String, String)>,
    pub query: Option<String>,
    pub output: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResultSnapshot {
    pub exit_code: i32,
    pub duration_ms: u64,
    pub output_sha256: String,
    pub error: Option<String>,
}

pub struct AuditLogger {
    path: PathBuf,
    seq: u64,
    session_id: String,
    user: String,
    host: String,
}

impl AuditLogger {
    pub fn new(base_dir: &PathBuf) -> Result<Self, String> {
        fs::create_dir_all(base_dir).map_err(|e| e.to_string())?;
        let path = base_dir.join("audit.jsonl");
        let session_id = format!("{}", uuid_like());
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        let host = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());
        Ok(AuditLogger {
            path,
            seq: 0,
            session_id,
            user,
            host,
        })
    }

    fn record(&mut self, kind: &'static str, rec: AuditRecord) {
        self.seq += 1;
        let rec = AuditRecord {
            kind,
            seq: self.seq,
            ..rec
        };
        let line = match serde_json::to_string(&rec) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("audit serialization failed: {e}");
                return;
            }
        };
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn propose(&mut self, intent: &Intent, provenance: &Provenance, source: &str) -> u64 {
        let next = self.seq + 1;
        self.record(
            "command_proposed",
            AuditRecord {
                kind: "command_proposed",
                ts: Utc::now().to_rfc3339(),
                session_id: self.session_id.clone(),
                seq: 0,
                user: self.user.clone(),
                host: self.host.clone(),
                intent: intent_snapshot(intent, source),
                argv: intent.argv("aws"),
                provenance: provenance.clone(),
                reference: None,
                result: None,
            },
        );
        next
    }

    pub fn executed(
        &mut self,
        intent: &Intent,
        provenance: &Provenance,
        source: &str,
        exit_code: i32,
        duration_ms: u64,
        output_sha256: String,
        error: Option<String>,
    ) {
        self.record(
            "command_executed",
            AuditRecord {
                kind: "command_executed",
                ts: Utc::now().to_rfc3339(),
                session_id: self.session_id.clone(),
                seq: 0,
                user: self.user.clone(),
                host: self.host.clone(),
                intent: intent_snapshot(intent, source),
                argv: intent.argv("aws"),
                provenance: provenance.clone(),
                reference: None,
                result: Some(ResultSnapshot {
                    exit_code,
                    duration_ms,
                    output_sha256,
                    error,
                }),
            },
        );
    }
}

fn intent_snapshot(intent: &Intent, source: &str) -> IntentSnapshot {
    IntentSnapshot {
        service: intent.service.clone(),
        operation: intent.operation.clone(),
        params: intent.params.clone(),
        query: intent.query.clone(),
        output: intent.output.clone(),
        source: source.to_string(),
    }
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:08x}{:08x}", std::process::id(), nanos)
}
