use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use serde::Serialize;

use crate::aws::builder::Intent;
use crate::aws::loader::Provenance;
use crate::ops::store::HistoryStore;

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
    #[serde(default)]
    pub max_items: Option<String>,
    #[serde(default)]
    pub page_size: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResultSnapshot {
    pub exit_code: i32,
    pub duration_ms: u64,
    pub output_sha256: String,
    pub error: Option<String>,
}

/// Outcome of a command execution, bundled so [`AuditLogger::executed`]
/// stays under the argument-count lint.
#[derive(Debug, Clone)]
pub struct ExecutionOutcome {
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
    aws_bin: String,
    store: Option<HistoryStore>,
}

impl AuditLogger {
    pub fn new(base_dir: &PathBuf, aws_bin: impl Into<String>) -> Result<Self, String> {
        Self::new_with_limits(base_dir, aws_bin, 10 * 1024 * 1024)
    }

    /// Create a logger, rotating `audit.jsonl` to `audit.1.jsonl` (single
    /// backup) when it exceeds `max_bytes`.
    pub fn new_with_limits(
        base_dir: &PathBuf,
        aws_bin: impl Into<String>,
        max_bytes: u64,
    ) -> Result<Self, String> {
        fs::create_dir_all(base_dir).map_err(|e| e.to_string())?;
        let path = base_dir.join("audit.jsonl");
        maybe_rotate(&path, max_bytes);
        let store = match HistoryStore::open(&base_dir.join("history.redb")) {
            Ok(store) => Some(store),
            Err(e) => {
                eprintln!("history store unavailable: {e}");
                None
            }
        };
        let seq = std::cmp::max(
            store
                .as_ref()
                .and_then(|s| s.max_seq().ok())
                .flatten()
                .unwrap_or(0),
            jsonl_len(&path),
        );
        let session_id = uuid_like().to_string();
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        let host = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());
        Ok(AuditLogger {
            path,
            seq,
            session_id,
            user,
            host,
            aws_bin: aws_bin.into(),
            store,
        })
    }

    /// Queryable history mirror (redb). Returns empty when unavailable.
    pub fn history_len(&self) -> u64 {
        self.store.as_ref().and_then(|s| s.len().ok()).unwrap_or(0)
    }

    pub fn history_get(&self, seq: u64) -> Option<String> {
        self.store.as_ref()?.get(seq).ok()?
    }

    pub fn history_recent(&self, n: usize) -> Vec<(u64, String)> {
        self.store
            .as_ref()
            .and_then(|s| s.recent(n).ok())
            .unwrap_or_default()
    }

    fn op_store(&self) -> Result<&HistoryStore, String> {
        self.store
            .as_ref()
            .ok_or_else(|| "history store unavailable".to_string())
    }

    pub fn put_incident(
        &mut self,
        incident: &crate::ops::incident::Incident,
    ) -> Result<(), String> {
        self.op_store()?.put_incident(incident)
    }

    pub fn get_incident(&self, id: &str) -> Result<Option<crate::ops::incident::Incident>, String> {
        self.op_store()?.get_incident(id)
    }

    pub fn list_incidents(&self) -> Result<Vec<crate::ops::incident::Incident>, String> {
        self.op_store()?.list_incidents()
    }

    pub fn transition_incident(
        &mut self,
        id: &str,
        to: &str,
    ) -> Result<crate::ops::incident::Incident, String> {
        self.op_store()?.transition_incident(id, to)
    }

    pub fn put_plan(&mut self, plan: &crate::ops::plan::Plan) -> Result<(), String> {
        self.op_store()?.put_plan(plan)
    }

    pub fn get_plan(&self, title: &str) -> Result<Option<crate::ops::plan::Plan>, String> {
        self.op_store()?.get_plan(title)
    }

    pub fn list_plans(&self) -> Result<Vec<String>, String> {
        self.op_store()?.list_plans()
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
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{line}");
        }
        if let Some(store) = self.store.as_ref()
            && let Err(e) = store.append(rec.seq, &line)
        {
            eprintln!("history store append failed: {e}");
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
                argv: intent.argv(&self.aws_bin),
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
        outcome: ExecutionOutcome,
        reference: Option<u64>,
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
                argv: intent.argv(&self.aws_bin),
                provenance: provenance.clone(),
                reference,
                result: Some(ResultSnapshot {
                    exit_code: outcome.exit_code,
                    duration_ms: outcome.duration_ms,
                    output_sha256: outcome.output_sha256,
                    error: outcome.error,
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
        max_items: intent.max_items.clone(),
        page_size: intent.page_size.clone(),
    }
}

fn jsonl_len(path: &PathBuf) -> u64 {
    std::fs::read_to_string(path)
        .map(|text| text.lines().filter(|l| !l.trim().is_empty()).count() as u64)
        .unwrap_or(0)
}

fn maybe_rotate(path: &PathBuf, max_bytes: u64) {
    if max_bytes == 0 {
        return;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= max_bytes {
        return;
    }
    let backup = path.with_extension("1.jsonl");
    let _ = std::fs::remove_file(&backup);
    if let Err(e) = std::fs::rename(path, &backup) {
        eprintln!("audit rotation failed: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_intent() -> Intent {
        Intent {
            service: "ec2".to_string(),
            operation: "describe-subnets".to_string(),
            ..Default::default()
        }
    }

    fn test_provenance() -> Provenance {
        Provenance {
            cli_version: "test".to_string(),
            model_path: "ec2/2016-11-15/service-2.json".to_string(),
            model_sha256: "abc".to_string(),
            api_version: "2016-11-15".to_string(),
        }
    }

    #[test]
    fn propose_links_to_executed_via_reference() {
        let dir = std::env::temp_dir().join(format!(
            "aws-tui-audit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let mut logger = AuditLogger::new(&dir, "my-aws").unwrap();
        let intent = test_intent();
        let prov = test_provenance();
        let seq = logger.propose(&intent, &prov, "test");
        logger.executed(
            &intent,
            &prov,
            "test",
            ExecutionOutcome {
                exit_code: 0,
                duration_ms: 1,
                output_sha256: "deadbeef".to_string(),
                error: None,
            },
            Some(seq),
        );
        let text = std::fs::read_to_string(dir.join("audit.jsonl")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let proposed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let executed: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(proposed["seq"], seq);
        assert_eq!(executed["reference"], seq);
        // Custom binary must be used, not hardcoded "aws".
        assert_eq!(proposed["argv"][0], "my-aws");
        assert_eq!(executed["argv"][0], "my-aws"); // redb mirror holds the same records and resumes seq on reopen.
        assert_eq!(logger.history_len(), 2);
        assert_eq!(
            logger.history_get(seq).unwrap(),
            text.lines().next().unwrap().to_string()
        );
        assert_eq!(logger.history_recent(1).len(), 1);
        drop(logger);
        let mut reopened = AuditLogger::new(&dir, "my-aws").unwrap();
        let next = reopened.propose(&intent, &prov, "test");
        assert_eq!(next, seq + 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_keeps_single_backup() {
        let dir = std::env::temp_dir().join(format!(
            "aws-tui-rotate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("audit.jsonl"), "x".repeat(100)).unwrap();
        let _ = AuditLogger::new_with_limits(&dir, "aws", 10).unwrap();
        assert!(dir.join("audit.1.jsonl").is_file());
        assert!(!dir.join("audit.jsonl").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
