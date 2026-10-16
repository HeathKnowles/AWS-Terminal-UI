use crate::aws::index::SearchIndex;
use crate::aws::loader::ServiceTree;

#[derive(Debug, Clone, Default)]
pub struct Intent {
    pub service: String,
    pub operation: String,
    /// (cli_name, value) pairs, in order.
    pub params: Vec<(String, String)>,
    /// JMESPath query, validated against output fields by the caller.
    pub query: Option<String>,
    /// --output format
    pub output: Option<String>,
    pub region: Option<String>,
    pub profile: Option<String>,
    /// CLI pagination controls (`--max-items`, `--page-size`).
    pub max_items: Option<String>,
    pub page_size: Option<String>,
}

impl Intent {
    pub fn argv(&self, aws_bin: &str) -> Vec<String> {
        let mut args = vec![
            aws_bin.to_string(),
            self.service.clone(),
            self.operation.clone(),
        ];
        if let Some(region) = &self.region {
            args.push("--region".to_string());
            args.push(region.clone());
        }
        if let Some(profile) = &self.profile {
            args.push("--profile".to_string());
            args.push(profile.clone());
        }
        for (k, v) in &self.params {
            args.push(format!("--{}", k));
            args.push(v.clone());
        }
        if let Some(q) = &self.query {
            args.push("--query".to_string());
            args.push(q.clone());
        }
        if let Some(o) = &self.output {
            args.push("--output".to_string());
            args.push(o.clone());
        }
        if let Some(m) = &self.max_items {
            args.push("--max-items".to_string());
            args.push(m.clone());
        }
        if let Some(p) = &self.page_size {
            args.push("--page-size".to_string());
            args.push(p.clone());
        }
        args.push("--no-cli-pager".to_string());
        args
    }

    pub fn command_line(&self, aws_bin: &str) -> String {
        Self::join_argv(&self.argv(aws_bin))
    }

    /// Command line with secret-looking param values replaced by `***`.
    /// Use for all on-screen display; the audit log keeps full values.
    pub fn masked_command_line(&self, aws_bin: &str) -> String {
        Self::join_argv(&self.masked_argv(aws_bin))
    }

    fn join_argv(argv: &[String]) -> String {
        let mut out = String::new();
        for (i, arg) in argv.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            if arg.contains(' ') {
                out.push('\'');
                out.push_str(arg);
                out.push('\'');
            } else {
                out.push_str(arg);
            }
        }
        out
    }

    fn masked_argv(&self, aws_bin: &str) -> Vec<String> {
        let mut args = vec![
            aws_bin.to_string(),
            self.service.clone(),
            self.operation.clone(),
        ];
        if let Some(region) = &self.region {
            args.push("--region".to_string());
            args.push(region.clone());
        }
        if let Some(profile) = &self.profile {
            args.push("--profile".to_string());
            args.push(profile.clone());
        }
        for (k, v) in &self.params {
            args.push(format!("--{}", k));
            args.push(if is_sensitive_param(k) {
                "***".to_string()
            } else {
                v.clone()
            });
        }
        if let Some(q) = &self.query {
            args.push("--query".to_string());
            args.push(q.clone());
        }
        if let Some(o) = &self.output {
            args.push("--output".to_string());
            args.push(o.clone());
        }
        if let Some(m) = &self.max_items {
            args.push("--max-items".to_string());
            args.push(m.clone());
        }
        if let Some(p) = &self.page_size {
            args.push("--page-size".to_string());
            args.push(p.clone());
        }
        args.push("--no-cli-pager".to_string());
        args
    }
}

/// Param names containing one of these substrings hold secrets. Values are
/// masked in on-screen command previews; the local audit log keeps them.
const SENSITIVE_SUBSTRINGS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api-key",
    "api_key",
    "apikey",
    "private-key",
    "privatekey",
    "passphrase",
    "access-key",
    "secret-key",
    "session-token",
];

pub fn is_sensitive_param(name: &str) -> bool {
    let lower = name.to_lowercase();
    SENSITIVE_SUBSTRINGS.iter().any(|s| lower.contains(s))
}

/// Resolve a natural-language intent string into a candidate Intent.
///
/// This is the heuristic Layer-1 resolver. It extracts:
///   - "across ec2" / "ec2"        -> service hint
///   - "subnet ids" / "regions"     -> output field hint
///   - "current region/profile"     -> flags to fill from environment
pub fn resolve_natural(index: &SearchIndex, text: &str, region: &str, profile: &str) -> Intent {
    let lower = text.to_lowercase();
    let mut intent = Intent {
        region: Some(region.to_string()),
        profile: Some(profile.to_string()),
        ..Default::default()
    };

    // Service hint: any whitespace-delimited token that matches a known service name.
    let tokens: Vec<String> = lower
        .split_whitespace()
        .map(|t| {
            t.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .filter(|t| !t.is_empty())
        .collect();
    let service_hint = index
        .by_service
        .keys()
        .find(|s| tokens.contains(s))
        .cloned();
    intent.service = service_hint.clone().unwrap_or_default();

    // Field tokens: noun-like words that hint at output field names.
    // Exclude the service token itself (e.g. "ec2" once matched as service).
    let field_tokens: Vec<String> = extract_field_tokens(&tokens)
        .into_iter()
        .filter(|t| {
            service_hint
                .as_deref()
                .map(|s| !s.starts_with(t.as_str()))
                .unwrap_or(true)
        })
        .collect();

    let candidates = find_by_fields(index, service_hint.as_deref(), &field_tokens);
    if let Some((service, op)) = candidates.first() {
        intent.service = service.clone();
        intent.operation = op.clone();
        intent.query = default_query_for(index, service, op, &field_tokens);
    }

    // Region/profile: if the user said "current", we inject resolved values.
    if lower.contains("current") {
        intent.region = Some(region.to_string());
        intent.profile = Some(profile.to_string());
    }

    intent
}

/// Check if the entity portion of an op name (after its verb) equals the
/// stemmed token, e.g. "describe-subnets" + "subnet" -> entity "subnets"
/// (ignoring trailing 's') -> true. "get-subnet-cidr-reservations" -> entity
/// "subnet-cidr-reservations" -> false.
fn entity_is_exact(op_name: &str, token: &str) -> bool {
    let parts: Vec<&str> = op_name.split('-').collect();
    if parts.len() < 2 {
        return false;
    }
    let entity = parts[1..].join("-");
    let entity_stemmed = stem(&entity);
    entity_stemmed == token
}

/// Word-boundary token match against an op name: a hyphen-separated segment
/// must stem to the token or start with it. This prevents "id" from matching
/// "cidr" inside "get-subnet-cidr-reservations".
fn op_name_has_token(op_name: &str, token: &str) -> bool {
    op_name
        .split('-')
        .any(|seg| stem(seg) == token || seg.starts_with(token))
}

/// Word-boundary token match against a field name: the (lowercased) field name
/// must equal the token, start with it, or end with it. "SubnetId" matches
/// "subnet" (prefix) and "id" (suffix); "Cidr" matches neither.
fn field_name_has_token(field_name: &str, token: &str) -> bool {
    let n = field_name.to_ascii_lowercase();
    n == token || n.starts_with(token) || n.ends_with(token)
}

/// Check if the entity portion of an op name (after its verb) starts with the
/// stemmed field token. "describe-subnets" + "subnet" -> entity "subnets"
/// starts_with "subnet" -> true. "describe-secondary-subnets" -> entity
/// "secondary-subnets" -> false.
fn entity_starts_with(op_name: &str, token: &str) -> bool {
    let parts: Vec<&str> = op_name.split('-').collect();
    if parts.is_empty() {
        return false;
    }
    let verb = parts[0];
    if !matches!(
        verb,
        "describe" | "list" | "get" | "delete" | "create" | "update"
    ) {
        return false;
    }
    let entity: String = parts[1..].join("-");
    entity.starts_with(token)
}

const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "me", "my", "give", "get", "show", "list", "current", "aws", "please", "all",
    "of", "this", "that", "across", "for", "in", "and", "or", "with", "to", "from", "you", "your",
    "i", "want", "need", "me", "can", "would", "could", "help",
];

/// Stem a noun token for field matching: "ids"->"id", "subnets"->"subnet",
/// "regions"->"region". Only strips a trailing 's' when preceded by a non-'s'
/// char, so "across" is left intact. Keeps "zones", "instances"->"instance".
fn stem(token: &str) -> String {
    let bytes = token.as_bytes();
    if bytes.len() >= 2 && bytes[bytes.len() - 1] == b's' && bytes[bytes.len() - 2] != b's' {
        token[..token.len() - 1].to_string()
    } else {
        token.to_string()
    }
}

/// Extract noun-like field hints from the query tokens.
fn extract_field_tokens(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        // Filter stop words on the raw token BEFORE stemming, so words like
        // "across" -> "acro" can't sneak through.
        .filter(|t| !STOP_WORDS.contains(&t.as_str()))
        .map(|t| stem(t))
        .collect()
}

/// Score each operation by how many field tokens match its output field names.
/// Returns (service, operation) candidates best first.
pub fn find_by_fields(
    index: &SearchIndex,
    service_hint: Option<&str>,
    field_tokens: &[String],
) -> Vec<(String, String)> {
    let mut scored: Vec<((String, String), i32)> = Vec::new();
    for row in &index.rows {
        if let crate::aws::index::SearchKind::Operation = row.kind {
            if service_hint.is_some_and(|s| row.service != s) {
                continue;
            }
            let mut score: i32 = 0;
            for ft in field_tokens {
                // Strong signal: the operation name itself contains the token,
                // e.g. "describe-subnets" for "subnet".
                if op_name_has_token(&row.operation, ft) {
                    score += 10;
                }
                // Strongest: the entity right after the verb matches the token,
                // e.g. "describe-subnets" (entity "subnets" starts with "subnet")
                // beats "describe-secondary-subnets" (entity "secondary-...").
                if entity_starts_with(&row.operation, ft) {
                    score += 8;
                }
                // Exact entity match ("subnets" ~ "subnet") ranks above prefix
                // matches ("subnet-cidr-reservations").
                if entity_is_exact(&row.operation, ft) {
                    score += 6;
                }
                // Weaker signal: the token appears in some output field name.
                if row
                    .output_fields
                    .iter()
                    .any(|f| field_name_has_token(&f.name, ft))
                {
                    score += 1;
                }
            }
            // Read-verb preference for "give me X" style queries.
            for (verb, weight) in [("describe", 4), ("list", 3), ("get", 2)] {
                if row.operation.starts_with(verb) {
                    score += weight;
                    break;
                }
            }
            if score > 0 {
                scored.push(((row.service.clone(), row.operation.clone()), score));
            }
        }
    }
    scored.sort_by_key(|b| std::cmp::Reverse(b.1));
    scored.into_iter().map(|(k, _)| k).collect()
}

/// Pick a default query from the first output field matching the field tokens.
fn default_query_for(
    index: &SearchIndex,
    service: &str,
    operation: &str,
    field_tokens: &[String],
) -> Option<String> {
    let row = index.get_operation(service, operation)?;
    // Score fields by how many tokens match; prefer the most specific match.
    let mut matching: Vec<(&crate::aws::loader::OutputField, usize)> = row
        .output_fields
        .iter()
        .map(|f| {
            let score = field_tokens
                .iter()
                .filter(|t| field_name_has_token(&f.name, t))
                .count();
            (f, score)
        })
        .filter(|(_, score)| *score > 0)
        .collect();
    matching.sort_by_key(|b| std::cmp::Reverse(b.1));
    matching.first().map(|(f, _)| f.path.clone())
}

/// Score a single op against field tokens — public for debugging/tests.
pub fn score_op(op_name: &str, field_tokens: &[String], has_field: &dyn Fn(&str) -> bool) -> i32 {
    let mut score: i32 = 0;
    for ft in field_tokens {
        if op_name_has_token(op_name, ft) {
            score += 10;
        }
        if entity_starts_with(op_name, ft) {
            score += 8;
        }
        if entity_is_exact(op_name, ft) {
            score += 6;
        }
        if has_field(ft) {
            score += 1;
        }
    }
    for (verb, weight) in [("describe", 4), ("list", 3), ("get", 2)] {
        if op_name.starts_with(verb) {
            score += weight;
            break;
        }
    }
    score
}

/// Validate that an intent references a real operation with known params.
pub fn validate_intent(index: &SearchIndex, intent: &Intent) -> Result<(), String> {
    if intent.service.is_empty() {
        return Err("no service selected".to_string());
    }
    if intent.operation.is_empty() {
        return Err("no operation selected".to_string());
    }
    let row = index
        .get_operation(&intent.service, &intent.operation)
        .ok_or_else(|| format!("unknown operation {}.{}", intent.service, intent.operation))?;
    if let Some(q) = &intent.query {
        if q.trim().is_empty() {
            return Err("empty query".to_string());
        }
        crate::aws::runner::validate_query(q)?;
    }
    for (label, value) in [
        ("--max-items", &intent.max_items),
        ("--page-size", &intent.page_size),
    ] {
        if let Some(v) = value
            && v.trim().parse::<u64>().ok().filter(|n| *n > 0).is_none()
        {
            return Err(format!("{label} must be a positive integer"));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for (name, value) in &intent.params {
        if name.trim().is_empty() {
            return Err("empty parameter name".to_string());
        }
        if !seen.insert(name) {
            return Err(format!("duplicate parameter --{name}"));
        }
        if !row.input_params.iter().any(|p| p == name) {
            return Err(format!(
                "unknown parameter --{name} for {}.{}",
                intent.service, intent.operation
            ));
        }
        if value.trim().is_empty() {
            return Err(format!("empty value for --{name}"));
        }
    }
    Ok(())
}

pub fn service_op_pairs(tree: &ServiceTree) -> Vec<(String, String)> {
    tree.services
        .iter()
        .flat_map(|s| {
            s.ops
                .iter()
                .map(move |o| (s.cli_name.clone(), o.cli_name.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws::index::{SearchIndex, SearchKind};
    use nucleo::Utf32String;

    fn test_index() -> SearchIndex {
        let rows = vec![
            SearchRowForTest::service("ec2"),
            SearchRowForTest::operation("ec2", "describe-subnets", &["subnet-ids"], &[]),
        ];
        SearchIndex {
            rows: rows
                .into_iter()
                .map(SearchRowForTest::into_row)
                .collect::<Vec<_>>(),
            by_service: [("ec2".to_string(), 0)].into_iter().collect(),
            by_service_op: [(("ec2".to_string(), "describe-subnets".to_string()), 1)]
                .into_iter()
                .collect(),
        }
    }

    struct SearchRowForTest {
        kind: SearchKind,
        service: String,
        operation: String,
        input_params: Vec<String>,
    }

    impl SearchRowForTest {
        fn service(service: &str) -> Self {
            Self {
                kind: SearchKind::Service,
                service: service.to_string(),
                operation: String::new(),
                input_params: Vec::new(),
            }
        }

        fn operation(service: &str, operation: &str, inputs: &[&str], _outputs: &[&str]) -> Self {
            Self {
                kind: SearchKind::Operation,
                service: service.to_string(),
                operation: operation.to_string(),
                input_params: inputs.iter().map(|s| s.to_string()).collect(),
            }
        }

        fn into_row(self) -> crate::aws::index::SearchRow {
            crate::aws::index::SearchRow {
                kind: self.kind,
                service: self.service.clone(),
                operation: self.operation.clone(),
                haystack: Utf32String::from(format!("{} {}", self.service, self.operation)),
                output_fields: Vec::new(),
                input_params: self.input_params,
            }
        }
    }

    #[test]
    fn rejects_invalid_jmespath() {
        let index = test_index();
        let intent = Intent {
            service: "ec2".to_string(),
            operation: "describe-subnets".to_string(),
            query: Some("Subnets[]..".to_string()),
            ..Default::default()
        };
        assert!(validate_intent(&index, &intent).is_err());
    }

    #[test]
    fn rejects_unknown_param() {
        let index = test_index();
        let intent = Intent {
            service: "ec2".to_string(),
            operation: "describe-subnets".to_string(),
            params: vec![("bogus-param".to_string(), "x".to_string())],
            ..Default::default()
        };
        let err = validate_intent(&index, &intent).unwrap_err();
        assert!(err.contains("unknown parameter"), "got: {err}");
    }

    #[test]
    fn rejects_duplicate_and_empty_params() {
        let index = test_index();
        let dup = Intent {
            service: "ec2".to_string(),
            operation: "describe-subnets".to_string(),
            params: vec![
                ("subnet-ids".to_string(), "a".to_string()),
                ("subnet-ids".to_string(), "b".to_string()),
            ],
            ..Default::default()
        };
        assert!(validate_intent(&index, &dup).is_err());
        let empty = Intent {
            service: "ec2".to_string(),
            operation: "describe-subnets".to_string(),
            params: vec![("subnet-ids".to_string(), "  ".to_string())],
            ..Default::default()
        };
        assert!(validate_intent(&index, &empty).is_err());
    }

    #[test]
    fn accepts_known_param_and_valid_query() {
        let index = test_index();
        let intent = Intent {
            service: "ec2".to_string(),
            operation: "describe-subnets".to_string(),
            params: vec![("subnet-ids".to_string(), "subnet-1".to_string())],
            query: Some("Subnets[].SubnetId".to_string()),
            ..Default::default()
        };
        assert!(validate_intent(&index, &intent).is_ok());
    }

    #[test]
    fn paging_argv_and_validation() {
        let mut intent = Intent {
            service: "ec2".to_string(),
            operation: "describe-subnets".to_string(),
            ..Default::default()
        };
        assert!(validate_intent(&test_index(), &intent).is_ok());
        intent.max_items = Some("10".to_string());
        intent.page_size = Some("5".to_string());
        assert!(validate_intent(&test_index(), &intent).is_ok());
        let argv = intent.argv("aws");
        assert!(argv.contains(&"--max-items".to_string()));
        assert!(argv.contains(&"--page-size".to_string()));
        intent.max_items = Some("0".to_string());
        assert!(validate_intent(&test_index(), &intent).is_err());
        intent.max_items = Some("abc".to_string());
        assert!(validate_intent(&test_index(), &intent).is_err());
    }

    #[test]
    fn secrets_mask_in_display_only() {
        assert!(is_sensitive_param("db-password"));
        assert!(is_sensitive_param("Token"));
        assert!(!is_sensitive_param("subnet-ids"));
        let intent = Intent {
            service: "rds".to_string(),
            operation: "create-db-instance".to_string(),
            params: vec![
                ("db-instance-identifier".to_string(), "mydb".to_string()),
                ("master-user-password".to_string(), "s3cret".to_string()),
            ],
            ..Default::default()
        };
        let shown = intent.masked_command_line("aws");
        assert!(shown.contains("mydb"));
        assert!(!shown.contains("s3cret"));
        assert!(shown.contains("***"));
        // Full command line (used for execution, not display) is intact.
        assert!(intent.command_line("aws").contains("s3cret"));
    }
}
