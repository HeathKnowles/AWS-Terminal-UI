use std::path::Path;

use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::ops::incident::{Incident, validate_incident};
use crate::ops::plan::Plan;

const AUDIT_TABLE: TableDefinition<u64, &str> = TableDefinition::new("audit");
const INCIDENT_TABLE: TableDefinition<&str, &str> = TableDefinition::new("incidents");
const PLAN_TABLE: TableDefinition<&str, &str> = TableDefinition::new("plans");

/// Queryable mirror of the JSONL audit log.
///
/// JSONL in `audit.jsonl` stays the source of truth (human-readable,
/// appendable). This store keeps the same records keyed by `seq` so restarts
/// can resume the sequence and the TUI can fetch recent entries without
/// scanning the whole file.
pub struct HistoryStore {
    db: Database,
}

impl HistoryStore {
    pub fn open(path: &Path) -> Result<Self, redb::Error> {
        let db = Database::create(path)?;
        {
            let write_txn = db.begin_write()?;
            {
                let _ = write_txn.open_table(AUDIT_TABLE)?;
                let _ = write_txn.open_table(INCIDENT_TABLE)?;
                let _ = write_txn.open_table(PLAN_TABLE)?;
            }
            write_txn.commit()?;
        }
        Ok(HistoryStore { db })
    }

    pub fn append(&self, seq: u64, json: &str) -> Result<(), redb::Error> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(AUDIT_TABLE)?;
            table.insert(seq, json)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get(&self, seq: u64) -> Result<Option<String>, redb::Error> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(AUDIT_TABLE)?;
        Ok(table.get(seq)?.map(|g| g.value().to_string()))
    }

    pub fn len(&self) -> Result<u64, redb::Error> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(AUDIT_TABLE)?;
        Ok(table.len()?)
    }

    pub fn is_empty(&self) -> Result<bool, redb::Error> {
        Ok(self.len()? == 0)
    }

    pub fn max_seq(&self) -> Result<Option<u64>, redb::Error> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(AUDIT_TABLE)?;
        Ok(table.last()?.map(|(k, _)| k.value()))
    }

    pub fn recent(&self, n: usize) -> Result<Vec<(u64, String)>, redb::Error> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(AUDIT_TABLE)?;
        let mut out = table
            .iter()?
            .rev()
            .filter_map(|r| r.ok())
            .take(n)
            .map(|(k, v)| (k.value(), v.value().to_string()))
            .collect::<Vec<_>>();
        out.reverse();
        Ok(out)
    }

    /// Store a validated incident, keyed by id. Overwrites the same id.
    pub fn put_incident(&self, incident: &Incident) -> Result<(), String> {
        validate_incident(incident).map_err(|e| format!("invalid incident: {e}"))?;
        let json =
            serde_json::to_string(incident).map_err(|e| format!("serialize incident: {e}"))?;
        let write_txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut table = write_txn
                .open_table(INCIDENT_TABLE)
                .map_err(|e| e.to_string())?;
            table
                .insert(incident.id.as_str(), json.as_str())
                .map_err(|e| e.to_string())?;
        }
        write_txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_incident(&self, id: &str) -> Result<Option<Incident>, String> {
        let read_txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = read_txn
            .open_table(INCIDENT_TABLE)
            .map_err(|e| e.to_string())?;
        let Some(value) = table.get(id).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        serde_json::from_str::<Incident>(value.value())
            .map(Some)
            .map_err(|e| format!("unreadable incident {id}: {e}"))
    }

    /// All incidents ordered for the task inbox: active statuses first
    /// (open → verifying), then severity (critical → low), then id.
    /// Corrupt entries are skipped.
    pub fn list_incidents(&self) -> Result<Vec<Incident>, String> {
        let read_txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = read_txn
            .open_table(INCIDENT_TABLE)
            .map_err(|e| e.to_string())?;
        let mut out: Vec<Incident> = table
            .iter()
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .filter_map(|(_, v)| serde_json::from_str::<Incident>(v.value()).ok())
            .collect();
        out.sort_by(|a, b| {
            (
                inbox_status_rank(&a.status),
                inbox_severity_rank(a.severity.as_deref()),
            )
                .cmp(&(
                    inbox_status_rank(&b.status),
                    inbox_severity_rank(b.severity.as_deref()),
                ))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// Move an incident to a new status following the allowed map.
    /// Read-modify-write happens in one transaction.
    pub fn transition_incident(&self, id: &str, to: &str) -> Result<Incident, String> {
        if !crate::ops::incident::INCIDENT_STATUSES.contains(&to) {
            return Err(format!("unknown status '{to}'"));
        }
        let write_txn = self.db.begin_write().map_err(|e| e.to_string())?;
        let updated = {
            let mut table = write_txn
                .open_table(INCIDENT_TABLE)
                .map_err(|e| e.to_string())?;
            let current: String = {
                let Some(value) = table.get(id).map_err(|e| e.to_string())? else {
                    return Err(format!("unknown incident '{id}'"));
                };
                value.value().to_string()
            };
            let mut incident: Incident = serde_json::from_str(&current)
                .map_err(|e| format!("unreadable incident {id}: {e}"))?;
            if !allowed_transition(&incident.status, to) {
                return Err(format!(
                    "cannot move {} from '{}' to '{to}'",
                    incident.id, incident.status
                ));
            }
            incident.status = to.to_string();
            let json =
                serde_json::to_string(&incident).map_err(|e| format!("serialize incident: {e}"))?;
            table
                .insert(incident.id.as_str(), json.as_str())
                .map_err(|e| e.to_string())?;
            incident
        };
        write_txn.commit().map_err(|e| e.to_string())?;
        Ok(updated)
    }

    /// Store a plan document keyed by title. Shape-checked as a [`Plan`];
    /// index validation happens at view time.
    pub fn put_plan(&self, plan: &Plan) -> Result<(), String> {
        if plan.title.trim().is_empty() {
            return Err("plan title is required".to_string());
        }
        let json = serde_json::to_string(plan).map_err(|e| format!("serialize plan: {e}"))?;
        let write_txn = self.db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut table = write_txn
                .open_table(PLAN_TABLE)
                .map_err(|e| e.to_string())?;
            table
                .insert(plan.title.as_str(), json.as_str())
                .map_err(|e| e.to_string())?;
        }
        write_txn.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_plan(&self, title: &str) -> Result<Option<Plan>, String> {
        let read_txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = read_txn.open_table(PLAN_TABLE).map_err(|e| e.to_string())?;
        let Some(guard) = table.get(title).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        serde_json::from_str::<Plan>(guard.value())
            .map(Some)
            .map_err(|e| format!("unreadable plan {title}: {e}"))
    }

    pub fn list_plans(&self) -> Result<Vec<String>, String> {
        let read_txn = self.db.begin_read().map_err(|e| e.to_string())?;
        let table = read_txn.open_table(PLAN_TABLE).map_err(|e| e.to_string())?;
        let mut out: Vec<String> = table
            .iter()
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .map(|(k, _)| k.value().to_string())
            .collect();
        out.sort();
        Ok(out)
    }
}

fn inbox_status_rank(status: &str) -> u8 {
    match status {
        "open" => 0,
        "investigating" => 1,
        "planned" => 2,
        "approved" => 3,
        "executing" => 4,
        "verifying" => 5,
        _ => 6,
    }
}

fn inbox_severity_rank(severity: Option<&str>) -> u8 {
    match severity {
        Some("critical") => 0,
        Some("high") => 1,
        Some("medium") => 2,
        Some("low") => 3,
        _ => 4,
    }
}

fn allowed_transition(from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    matches!(
        (from, to),
        ("open", "investigating")
            | ("open", "closed")
            | ("investigating", "planned")
            | ("investigating", "open")
            | ("investigating", "closed")
            | ("planned", "approved")
            | ("planned", "investigating")
            | ("planned", "closed")
            | ("approved", "executing")
            | ("approved", "planned")
            | ("approved", "closed")
            | ("executing", "verifying")
            | ("executing", "planned")
            | ("executing", "closed")
            | ("verifying", "closed")
            | ("verifying", "executing")
            | ("closed", "open")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_in_seq_order() {
        let dir = std::env::temp_dir().join(format!(
            "aws-tui-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = HistoryStore::open(&dir.join("history.redb")).unwrap();
        store.append(1, r#"{"seq":1}"#).unwrap();
        store.append(2, r#"{"seq":2}"#).unwrap();
        assert_eq!(store.len().unwrap(), 2);
        assert_eq!(store.max_seq().unwrap(), Some(2));
        assert_eq!(store.get(1).unwrap().as_deref(), Some(r#"{"seq":1}"#));
        let recent = store.recent(1).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].0, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_incident(id: &str, status: &str, severity: Option<&str>) -> Incident {
        Incident {
            version: 1,
            id: id.to_string(),
            title: format!("title {id}"),
            status: status.to_string(),
            severity: severity.map(|s| s.to_string()),
            account: String::new(),
            region: String::new(),
            signals: Vec::new(),
            hypotheses: Vec::new(),
            plan_path: None,
            approvals: Vec::new(),
        }
    }

    fn test_store(name: &str) -> (HistoryStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "aws-tui-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = HistoryStore::open(&dir.join("history.redb")).unwrap();
        (store, dir)
    }

    #[test]
    fn incidents_round_trip_and_reject_invalid() {
        let (store, dir) = test_store("incidents");
        store
            .put_incident(&test_incident("INC-1", "open", None))
            .unwrap();
        assert_eq!(
            store.get_incident("INC-1").unwrap().unwrap().title,
            "title INC-1"
        );
        assert!(store.get_incident("missing").unwrap().is_none());
        let mut bad = test_incident("INC-2", "open", None);
        bad.title = String::new();
        assert!(store.put_incident(&bad).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transitions_follow_the_map() {
        let (store, dir) = test_store("transitions");
        store
            .put_incident(&test_incident("INC-1", "open", None))
            .unwrap();
        assert!(store.transition_incident("INC-1", "approved").is_err());
        let updated = store.transition_incident("INC-1", "investigating").unwrap();
        assert_eq!(updated.status, "investigating");
        assert!(store.transition_incident("INC-1", "bogus").is_err());
        assert!(store.transition_incident("missing", "open").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inbox_orders_active_then_severe() {
        let (store, dir) = test_store("inbox");
        store
            .put_incident(&test_incident("C-closed", "closed", Some("critical")))
            .unwrap();
        store
            .put_incident(&test_incident("B-inv", "investigating", Some("low")))
            .unwrap();
        store
            .put_incident(&test_incident("A-open", "open", Some("high")))
            .unwrap();
        let ids: Vec<String> = store
            .list_incidents()
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, vec!["A-open", "B-inv", "C-closed"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plans_round_trip_by_title() {
        use crate::ops::plan::{Plan, PlanStep};
        let (store, dir) = test_store("plans");
        let plan = Plan {
            version: 1,
            title: "t".to_string(),
            target: "x".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                title: "s".to_string(),
                service: "sts".to_string(),
                operation: "get-caller-identity".to_string(),
                params: Vec::new(),
                query: None,
                output: None,
                requires_approval: false,
                preconditions: Vec::new(),
                expected: Vec::new(),
            }],
            risks: Vec::new(),
            preconditions: Vec::new(),
            rollback: Vec::new(),
            expected: Vec::new(),
        };
        store.put_plan(&plan).unwrap();
        assert_eq!(store.get_plan("t").unwrap().unwrap(), plan);
        assert_eq!(store.list_plans().unwrap(), vec!["t".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
