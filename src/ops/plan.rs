use serde::{Deserialize, Serialize};

use crate::aws::index::SearchIndex;

/// Allowed `--output` values accepted by the AWS CLI.
pub const PLAN_ALLOWED_OUTPUTS: &[&str] = &["json", "text", "table", "yaml", "yaml-stream"];

/// Schema version for [`Plan`]. Bump on breaking changes.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub version: u32,
    pub title: String,
    pub target: String,
    #[serde(default)]
    pub steps: Vec<PlanStep>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub rollback: Vec<String>,
    #[serde(default)]
    pub expected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub service: String,
    pub operation: String,
    #[serde(default)]
    pub params: Vec<(String, String)>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub expected: Vec<String>,
}

impl Plan {
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("invalid plan JSON: {e}"))
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// JSON Schema (draft 2020-12 subset) describing [`Plan`].
///
/// Kept as data so editors and tooling can validate plan files without
/// depending on this crate. [`validate_plan`] is authoritative at runtime.
pub fn plan_json_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "aws-tui Plan",
        "type": "object",
        "required": ["version", "title", "target", "steps"],
        "properties": {
            "version": {"type": "integer", "const": PLAN_SCHEMA_VERSION},
            "title": {"type": "string", "minLength": 1},
            "target": {"type": "string", "minLength": 1},
            "steps": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "required": ["id", "title", "service", "operation"],
                    "properties": {
                        "id": {"type": "string", "minLength": 1},
                        "title": {"type": "string", "minLength": 1},
                        "service": {"type": "string", "minLength": 1},
                        "operation": {"type": "string", "minLength": 1},
                        "params": {
                            "type": "array",
                            "items": {
                                "type": "array",
                                "prefixItems": [{"type": "string"}, {"type": "string"}],
                                "minItems": 2,
                                "maxItems": 2
                            }
                        },
                        "query": {"type": ["string", "null"]},
                        "output": {"type": ["string", "null"]},
                        "requires_approval": {"type": "boolean"},
                        "preconditions": {"type": "array", "items": {"type": "string"}},
                        "expected": {"type": "array", "items": {"type": "string"}}
                    },
                    "additionalProperties": false
                }
            },
            "risks": {"type": "array", "items": {"type": "string"}},
            "preconditions": {"type": "array", "items": {"type": "string"}},
            "rollback": {"type": "array", "items": {"type": "string"}},
            "expected": {"type": "array", "items": {"type": "string"}}
        },
        "additionalProperties": false
    })
}

/// Validate plan shape plus operation/param/query semantics against the index.
pub fn validate_plan(index: &SearchIndex, plan: &Plan) -> Result<(), String> {
    if plan.version != PLAN_SCHEMA_VERSION {
        return Err(format!(
            "unsupported plan version {} (expected {})",
            plan.version, PLAN_SCHEMA_VERSION
        ));
    }
    if plan.title.trim().is_empty() {
        return Err("plan title is required".to_string());
    }
    if plan.target.trim().is_empty() {
        return Err("plan target is required".to_string());
    }
    if plan.steps.is_empty() {
        return Err("plan has no steps".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for (n, step) in plan.steps.iter().enumerate() {
        let at = format!("step {} ({})", n + 1, step.id);
        if step.id.trim().is_empty() {
            return Err(format!("{at}: id is required"));
        }
        if !seen.insert(step.id.clone()) {
            return Err(format!("duplicate step id '{}'", step.id));
        }
        if step.title.trim().is_empty() {
            return Err(format!("{at}: title is required"));
        }
        let row = index
            .get_operation(&step.service, &step.operation)
            .ok_or_else(|| {
                format!(
                    "{at}: unknown operation {}.{}",
                    step.service, step.operation
                )
            })?;
        let mut seen_params = std::collections::HashSet::new();
        for (name, value) in &step.params {
            if name.trim().is_empty() {
                return Err(format!("{at}: empty parameter name"));
            }
            if !seen_params.insert(name) {
                return Err(format!("{at}: duplicate parameter --{name}"));
            }
            if !row.input_params.iter().any(|p| p == name) {
                return Err(format!(
                    "{at}: unknown parameter --{name} for {}.{}",
                    step.service, step.operation
                ));
            }
            if value.trim().is_empty() {
                return Err(format!("{at}: empty value for --{name}"));
            }
        }
        if let Some(q) = &step.query {
            if q.trim().is_empty() {
                return Err(format!("{at}: empty query"));
            }
            crate::aws::runner::validate_query(q).map_err(|e| format!("{at}: {e}"))?;
        }
        if let Some(o) = &step.output
            && !o.trim().is_empty()
            && !PLAN_ALLOWED_OUTPUTS.contains(&o.trim())
        {
            return Err(format!(
                "{at}: unsupported output '{}' (expected one of {})",
                o.trim(),
                PLAN_ALLOWED_OUTPUTS.join(", ")
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_index() -> SearchIndex {
        use crate::aws::index::{SearchKind, SearchRow};
        use nucleo::Utf32String;
        let rows = vec![
            SearchRow {
                kind: SearchKind::Service,
                service: "sts".to_string(),
                operation: String::new(),
                haystack: Utf32String::from("sts"),
                output_fields: Vec::new(),
                input_params: Vec::new(),
            },
            SearchRow {
                kind: SearchKind::Operation,
                service: "sts".to_string(),
                operation: "get-caller-identity".to_string(),
                haystack: Utf32String::from("sts get-caller-identity"),
                output_fields: Vec::new(),
                input_params: Vec::new(),
            },
            SearchRow {
                kind: SearchKind::Operation,
                service: "ec2".to_string(),
                operation: "describe-subnets".to_string(),
                haystack: Utf32String::from("ec2 describe-subnets"),
                output_fields: Vec::new(),
                input_params: vec!["subnet-ids".to_string()],
            },
        ];
        let mut by_service = std::collections::HashMap::new();
        by_service.insert("sts".to_string(), 0);
        let mut by_service_op = std::collections::HashMap::new();
        by_service_op.insert(("sts".to_string(), "get-caller-identity".to_string()), 1);
        by_service_op.insert(("ec2".to_string(), "describe-subnets".to_string()), 2);
        SearchIndex {
            rows,
            by_service,
            by_service_op,
        }
    }

    fn valid_plan() -> Plan {
        Plan {
            version: 1,
            title: "Verify identity then list subnets".to_string(),
            target: "us-east-1 / default".to_string(),
            steps: vec![
                PlanStep {
                    id: "whoami".to_string(),
                    title: "Confirm effective identity".to_string(),
                    service: "sts".to_string(),
                    operation: "get-caller-identity".to_string(),
                    params: Vec::new(),
                    query: None,
                    output: Some("json".to_string()),
                    requires_approval: false,
                    preconditions: Vec::new(),
                    expected: vec!["exit 0".to_string()],
                },
                PlanStep {
                    id: "subnets".to_string(),
                    title: "List subnets".to_string(),
                    service: "ec2".to_string(),
                    operation: "describe-subnets".to_string(),
                    params: vec![("subnet-ids".to_string(), "subnet-1".to_string())],
                    query: Some("Subnets[].SubnetId".to_string()),
                    output: None,
                    requires_approval: true,
                    preconditions: vec!["whoami succeeded".to_string()],
                    expected: vec!["returns subnet list".to_string()],
                },
            ],
            risks: vec!["read-only".to_string()],
            preconditions: vec!["valid AWS credentials".to_string()],
            rollback: vec!["no mutations".to_string()],
            expected: vec!["identity confirmed, subnets listed".to_string()],
        }
    }

    #[test]
    fn accepts_valid_plan_and_round_trips() {
        let index = test_index();
        let plan = valid_plan();
        assert!(validate_plan(&index, &plan).is_ok());
        let text = plan.to_json_pretty();
        let parsed = Plan::from_json(&text).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn rejects_missing_title_target_and_steps() {
        let index = test_index();
        let mut plan = valid_plan();
        plan.title = "  ".to_string();
        assert!(validate_plan(&index, &plan).is_err());
        let mut plan = valid_plan();
        plan.target = String::new();
        assert!(validate_plan(&index, &plan).is_err());
        let mut plan = valid_plan();
        plan.steps.clear();
        assert!(validate_plan(&index, &plan).is_err());
    }

    #[test]
    fn rejects_duplicate_step_ids_and_unknown_ops() {
        let index = test_index();
        let mut plan = valid_plan();
        plan.steps[1].id = "whoami".to_string();
        assert!(validate_plan(&index, &plan).is_err());
        let mut plan = valid_plan();
        plan.steps[1].operation = "nope".to_string();
        assert!(validate_plan(&index, &plan).is_err());
    }

    #[test]
    fn rejects_bad_params_query_and_output() {
        let index = test_index();
        let mut plan = valid_plan();
        plan.steps[1].params = vec![("bogus".to_string(), "x".to_string())];
        assert!(validate_plan(&index, &plan).is_err());
        let mut plan = valid_plan();
        plan.steps[1].query = Some("Subnets[]..".to_string());
        assert!(validate_plan(&index, &plan).is_err());
        let mut plan = valid_plan();
        plan.steps[1].output = Some("bogus".to_string());
        assert!(validate_plan(&index, &plan).is_err());
    }

    #[test]
    fn example_plan_validates_against_real_models() {
        let dir = match crate::aws::loader::default_data_dir() {
            Some(dir) => dir,
            None => return,
        };
        if !dir.is_dir() {
            return;
        }
        let tree = crate::aws::loader::load_services(&dir, "test");
        if tree.services.is_empty() {
            return;
        }
        let index = crate::aws::index::SearchIndex::build(&tree);
        let manifest =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plans/example.json");
        let text = std::fs::read_to_string(&manifest).expect("plans/example.json readable");
        let plan = Plan::from_json(&text).unwrap();
        validate_plan(&index, &plan).expect("example plan valid");
    }

    #[test]
    fn schema_describes_required_shape() {
        let schema = plan_json_schema();
        let required = schema["required"].as_array().unwrap();
        for key in ["version", "title", "target", "steps"] {
            assert!(required.iter().any(|v| v == key));
        }
    }
}
