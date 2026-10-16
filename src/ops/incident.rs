use serde::{Deserialize, Serialize};

/// Schema version for [`Incident`]. Bump on breaking changes.
pub const INCIDENT_SCHEMA_VERSION: u32 = 1;

pub const INCIDENT_STATUSES: &[&str] = &[
    "open",
    "investigating",
    "planned",
    "approved",
    "executing",
    "verifying",
    "closed",
];

pub const INCIDENT_SEVERITIES: &[&str] = &["low", "medium", "high", "critical"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    pub version: u32,
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub signals: Vec<EvidenceEvent>,
    #[serde(default)]
    pub hypotheses: Vec<Hypothesis>,
    #[serde(default)]
    pub plan_path: Option<String>,
    #[serde(default)]
    pub approvals: Vec<Approval>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEvent {
    pub ts: String,
    pub source: String,
    pub summary: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hypothesis {
    pub id: String,
    pub statement: String,
    #[serde(default)]
    pub confidence: u8,
    #[serde(default)]
    pub supporting: Vec<String>,
    #[serde(default)]
    pub refuted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub step: String,
    pub approver: String,
    pub decision: String,
    #[serde(default)]
    pub note: String,
}

impl Incident {
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("invalid incident JSON: {e}"))
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// JSON Schema (draft 2020-12 subset) describing [`Incident`].
pub fn incident_json_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "aws-tui Incident",
        "type": "object",
        "required": ["version", "id", "title", "status"],
        "properties": {
            "version": {"type": "integer", "const": INCIDENT_SCHEMA_VERSION},
            "id": {"type": "string", "minLength": 1},
            "title": {"type": "string", "minLength": 1},
            "status": {"type": "string", "enum": INCIDENT_STATUSES},
            "severity": {"type": ["string", "null"], "enum": ["low", "medium", "high", "critical", null]},
            "account": {"type": "string"},
            "region": {"type": "string"},
            "signals": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["ts", "source", "summary"],
                    "properties": {
                        "ts": {"type": "string", "minLength": 1},
                        "source": {"type": "string", "minLength": 1},
                        "summary": {"type": "string", "minLength": 1},
                        "detail": {"type": "string"}
                    },
                    "additionalProperties": false
                }
            },
            "hypotheses": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "statement"],
                    "properties": {
                        "id": {"type": "string", "minLength": 1},
                        "statement": {"type": "string", "minLength": 1},
                        "confidence": {"type": "integer", "minimum": 0, "maximum": 100},
                        "supporting": {"type": "array", "items": {"type": "string"}},
                        "refuted": {"type": "boolean"}
                    },
                    "additionalProperties": false
                }
            },
            "plan_path": {"type": ["string", "null"]},
            "approvals": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["step", "approver", "decision"],
                    "properties": {
                        "step": {"type": "string", "minLength": 1},
                        "approver": {"type": "string", "minLength": 1},
                        "decision": {"type": "string", "enum": ["approved", "rejected"]},
                        "note": {"type": "string"}
                    },
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false
    })
}

pub fn validate_incident(incident: &Incident) -> Result<(), String> {
    if incident.version != INCIDENT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported incident version {} (expected {})",
            incident.version, INCIDENT_SCHEMA_VERSION
        ));
    }
    if incident.id.trim().is_empty() {
        return Err("incident id is required".to_string());
    }
    if incident.title.trim().is_empty() {
        return Err("incident title is required".to_string());
    }
    if !INCIDENT_STATUSES.contains(&incident.status.as_str()) {
        return Err(format!(
            "unknown status '{}' (expected one of {})",
            incident.status,
            INCIDENT_STATUSES.join(", ")
        ));
    }
    if let Some(sev) = incident.severity.as_deref()
        && !sev.trim().is_empty()
        && !INCIDENT_SEVERITIES.contains(&sev)
    {
        return Err(format!(
            "unknown severity '{sev}' (expected one of {})",
            INCIDENT_SEVERITIES.join(", ")
        ));
    }
    for (n, sig) in incident.signals.iter().enumerate() {
        let at = format!("signal {}", n + 1);
        if sig.ts.trim().is_empty() {
            return Err(format!("{at}: ts is required"));
        }
        if sig.source.trim().is_empty() {
            return Err(format!("{at}: source is required"));
        }
        if sig.summary.trim().is_empty() {
            return Err(format!("{at}: summary is required"));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for h in &incident.hypotheses {
        if h.id.trim().is_empty() {
            return Err("hypothesis id is required".to_string());
        }
        if !seen.insert(h.id.clone()) {
            return Err(format!("duplicate hypothesis id '{}'", h.id));
        }
        if h.statement.trim().is_empty() {
            return Err(format!("hypothesis {}: statement is required", h.id));
        }
        if h.confidence > 100 {
            return Err(format!("hypothesis {}: confidence must be 0-100", h.id));
        }
    }
    for (n, a) in incident.approvals.iter().enumerate() {
        let at = format!("approval {}", n + 1);
        if a.step.trim().is_empty() {
            return Err(format!("{at}: step is required"));
        }
        if a.approver.trim().is_empty() {
            return Err(format!("{at}: approver is required"));
        }
        if !["approved", "rejected"].contains(&a.decision.as_str()) {
            return Err(format!(
                "{at}: unknown decision '{}' (expected approved/rejected)",
                a.decision
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_incident() -> Incident {
        Incident {
            version: 1,
            id: "INC-142".to_string(),
            title: "API error rate elevated".to_string(),
            status: "investigating".to_string(),
            severity: Some("high".to_string()),
            account: "dev".to_string(),
            region: "us-east-1".to_string(),
            signals: vec![EvidenceEvent {
                ts: "2026-10-01T00:00:00Z".to_string(),
                source: "cloudwatch-alarm".to_string(),
                summary: "5xx rate above threshold".to_string(),
                detail: String::new(),
            }],
            hypotheses: vec![Hypothesis {
                id: "h1".to_string(),
                statement: "Errors correlate with the latest deployment".to_string(),
                confidence: 60,
                supporting: vec!["deploy at 23:40Z".to_string()],
                refuted: false,
            }],
            plan_path: Some("plans/example.json".to_string()),
            approvals: vec![Approval {
                step: "subnets".to_string(),
                approver: "oncall".to_string(),
                decision: "approved".to_string(),
                note: String::new(),
            }],
        }
    }

    #[test]
    fn accepts_valid_incident_and_round_trips() {
        let incident = valid_incident();
        assert!(validate_incident(&incident).is_ok());
        let text = incident.to_json_pretty();
        assert_eq!(Incident::from_json(&text).unwrap(), incident);
    }

    #[test]
    fn rejects_bad_status_severity_and_ids() {
        let mut incident = valid_incident();
        incident.status = "nope".to_string();
        assert!(validate_incident(&incident).is_err());
        let mut incident = valid_incident();
        incident.severity = Some("nope".to_string());
        assert!(validate_incident(&incident).is_err());
        let mut incident = valid_incident();
        incident.id = "  ".to_string();
        assert!(validate_incident(&incident).is_err());
        let mut incident = valid_incident();
        incident.hypotheses.push(incident.hypotheses[0].clone());
        assert!(validate_incident(&incident).is_err());
    }

    #[test]
    fn rejects_bad_approvals_and_signals() {
        let mut incident = valid_incident();
        incident.approvals[0].decision = "maybe".to_string();
        assert!(validate_incident(&incident).is_err());
        let mut incident = valid_incident();
        incident.signals[0].summary = String::new();
        assert!(validate_incident(&incident).is_err());
    }

    #[test]
    fn example_incident_parses_and_validates() {
        let manifest =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("incidents/example.json");
        let text = std::fs::read_to_string(&manifest).expect("incidents/example.json readable");
        let incident = Incident::from_json(&text).unwrap();
        validate_incident(&incident).expect("example incident valid");
    }

    #[test]
    fn schema_describes_required_shape() {
        let schema = incident_json_schema();
        let required = schema["required"].as_array().unwrap();
        for key in ["version", "id", "title", "status"] {
            assert!(required.iter().any(|v| v == key));
        }
    }
}
