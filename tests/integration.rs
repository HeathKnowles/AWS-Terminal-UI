use std::path::PathBuf;

use aws_tui::builder::resolve_natural;
use aws_tui::index::SearchIndex;
use aws_tui::loader::{default_data_dir, load_services};
use aws_tui::runner::apply_query;

fn find_repo() -> PathBuf {
    if let Some(dir) = default_data_dir() {
        return dir;
    }
    panic!("data dir not found; set AWS_TUI_DATA_DIR")
}

#[test]
fn subnet_ids_query_builds_correct_argv() {
    // Simulates: "give me current subnet ids across ec2"
    let dir = find_repo();
    let tree = load_services(&dir, "2.36.18");
    let index = SearchIndex::build(&tree);
    let intent = resolve_natural(
        &index,
        "give me current subnet ids across ec2",
        "us-east-1",
        "prod",
    );
    assert_eq!(intent.service, "ec2");
    assert_eq!(intent.operation, "describe-subnets");
    assert_eq!(intent.query.as_deref(), Some("Subnets[].SubnetId"));
    assert_eq!(intent.region.as_deref(), Some("us-east-1"));
    assert_eq!(intent.profile.as_deref(), Some("prod"));
    assert_eq!(
        intent.argv("aws"),
        vec![
            "aws",
            "ec2",
            "describe-subnets",
            "--region",
            "us-east-1",
            "--profile",
            "prod",
            "--query",
            "Subnets[].SubnetId",
            "--no-cli-pager",
        ]
    );
}

#[test]
fn follow_up_requery_on_cached_result() {
    // "give me the regions of this" => apply secondary query to cached json.
    let cached = serde_json::json!({
        "Subnets": [
            {"SubnetId": "subnet-1", "AvailabilityZone": "us-east-1a"},
            {"SubnetId": "subnet-2", "AvailabilityZone": "us-east-1b"},
        ]
    });
    let result = apply_query(&cached, "Subnets[].AvailabilityZone").unwrap();
    let zones: Vec<String> = serde_json::from_value(result).unwrap();
    assert_eq!(zones, vec!["us-east-1a", "us-east-1b"]);
}

#[test]
fn validate_query_rejects_bad_jmespath() {
    assert!(aws_tui::runner::validate_query("Subnets[]..").is_err());
    assert!(aws_tui::runner::validate_query("Subnets[].SubnetId").is_ok());
}

#[test]
fn ops_without_query_are_valid() {
    // get-caller-identity has no query; validation must not reject it.
    let dir = find_repo();
    let tree = load_services(&dir, "2.36.18");
    let index = SearchIndex::build(&tree);
    let intent = aws_tui::builder::Intent {
        service: "sts".to_string(),
        operation: "get-caller-identity".to_string(),
        region: Some("us-east-1".to_string()),
        profile: Some("dev".to_string()),
        ..Default::default()
    };
    assert!(aws_tui::builder::validate_intent(&index, &intent).is_ok());
    // And the argv builds without a --query flag.
    let argv = intent.argv("aws");
    assert!(!argv.iter().any(|a| a == "--query"));
}
