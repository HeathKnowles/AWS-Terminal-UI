use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::aws::model::{Member, Operation, ServiceModel};
use crate::aws::xform::xform_name;

pub const DATA_DIR_ENV: &str = "AWS_TUI_DATA_DIR";

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Provenance {
    pub cli_version: String,
    pub model_path: String,
    pub model_sha256: String,
    pub api_version: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct InputParam {
    pub cli_name: String,
    pub api_name: String,
    pub required: bool,
    pub type_name: String,
    pub enum_values: Vec<String>,
    pub is_list: bool,
    pub is_map: bool,
    pub is_structure: bool,
    pub docs: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct OutputField {
    /// JMESPath path from the output root, e.g. "Subnets[].SubnetId"
    pub path: String,
    /// Last segment name, e.g. "SubnetId"
    pub name: String,
    /// Resolved leaf type: string/integer/boolean/timestamp/...
    pub kind: String,
    /// True if any ancestor in the path is a list
    pub is_list_leaf: bool,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct OpEntry {
    pub cli_name: String,
    pub api_name: String,
    pub docs: String,
    pub input: Vec<InputParam>,
    pub output_fields: Vec<OutputField>,
    pub output_shape_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ServiceEntry {
    /// CLI service name, e.g. "ec2"
    pub cli_name: String,
    pub full_name: String,
    pub service_id: String,
    pub ops: Vec<OpEntry>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct ServiceTree {
    pub services: Vec<ServiceEntry>,
}

pub fn default_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(DATA_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    if let Ok(cli_path) = std::env::var("AWS_CLI_DATA_DIR") {
        return Some(PathBuf::from(cli_path));
    }
    // Fall back to the sibling aws-cli checkout.
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd
            .join("aws-cli")
            .join("awscli")
            .join("botocore")
            .join("data");
        if candidate.is_dir() {
            return Some(candidate);
        }
        for ancestor in cwd.ancestors() {
            let candidate = ancestor
                .join("aws-cli")
                .join("awscli")
                .join("botocore")
                .join("data");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Discover all `<service>/<version>/service-2.json` files under the data dir.
pub fn discover_models(data_dir: &Path) -> Vec<(PathBuf, String, String)> {
    let mut out = Vec::new();
    if !data_dir.is_dir() {
        return out;
    }
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let service_path = entry.path();
        if !service_path.is_dir() {
            continue;
        }
        let Some(service_name) = service_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if service_name.starts_with('.') {
            continue;
        }
        // Pick the most recent api version directory.
        let mut versions: Vec<(PathBuf, String)> = std::fs::read_dir(&service_path)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                let v = p.file_name()?.to_str()?.to_string();
                if p.is_dir() && p.join("service-2.json").is_file() {
                    Some((p, v))
                } else {
                    None
                }
            })
            .collect();
        versions.sort();
        if let Some((version_dir, _v)) = versions.last() {
            let model_path = version_dir.join("service-2.json");
            out.push((model_path, service_name.to_string(), _v.to_string()));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Load and parse every service model in parallel, returning a ServiceTree.
///
/// Models that fail to read or parse are skipped with a diagnostic on
/// stderr; previously they silently produced empty entries.
pub fn load_services(data_dir: &Path, cli_version: &str) -> ServiceTree {
    let models = discover_models(data_dir);
    let mut services: Vec<ServiceEntry> = models
        .par_iter()
        .filter_map(|(path, service_name, api_version)| {
            load_service(path, service_name, api_version, cli_version)
        })
        .collect();
    services.sort_by(|a, b| a.cli_name.cmp(&b.cli_name));
    ServiceTree { services }
}

/// Cached variant of [`load_services`].
///
/// The parsed tree is stored as JSON under the platform cache dir, keyed by
/// a fingerprint of the CLI version plus every model path, size, and mtime.
/// Stale or unreadable caches fall back to a full parse; cache writes are
/// best-effort and never fail startup.
pub fn load_services_cached(data_dir: &Path, cli_version: &str) -> ServiceTree {
    let models = discover_models(data_dir);
    let fingerprint = cache_fingerprint(&models, cli_version);
    if let Some(dir) = cache_dir() {
        let path = dir.join(format!("models-{fingerprint}.json"));
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(tree) = serde_json::from_slice::<ServiceTree>(&bytes)
            && !tree.services.is_empty()
        {
            return tree;
        }
        let tree = load_services(data_dir, cli_version);
        if !tree.services.is_empty()
            && std::fs::create_dir_all(&dir).is_ok()
            && let Ok(bytes) = serde_json::to_vec(&tree)
        {
            let _ = std::fs::write(&path, bytes);
        }
        return tree;
    }
    load_services(data_dir, cli_version)
}

pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("aws-tui"))
}

fn cache_fingerprint(models: &[(PathBuf, String, String)], cli_version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cli_version.as_bytes());
    let mut sorted: Vec<(&PathBuf, &String, &String)> =
        models.iter().map(|(p, s, v)| (p, s, v)).collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (path, service, api_version) in sorted {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(service.as_bytes());
        hasher.update(api_version.as_bytes());
        if let Ok(meta) = std::fs::metadata(path) {
            hasher.update(meta.len().to_le_bytes());
            if let Ok(mtime) = meta.modified()
                && let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH)
            {
                hasher.update(d.as_secs().to_le_bytes());
            }
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn sha256_file(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

fn load_service(
    path: &Path,
    service_name: &str,
    api_version: &str,
    cli_version: &str,
) -> Option<ServiceEntry> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!(
                "skipping {}: failed to read {}: {e}",
                service_name,
                path.display()
            );
            return None;
        }
    };
    let model: ServiceModel = match serde_json::from_str(&raw) {
        Ok(model) => model,
        Err(e) => {
            eprintln!(
                "skipping {}: failed to parse {}: {e}",
                service_name,
                path.display()
            );
            return None;
        }
    };

    let mut ops: Vec<OpEntry> = model
        .operations
        .iter()
        .map(|(api_name, op)| build_op(api_name, op, &model))
        .collect();
    ops.sort_by(|a, b| a.cli_name.cmp(&b.cli_name));

    Some(ServiceEntry {
        cli_name: service_name.to_string(),
        full_name: model.metadata.service_full_name.clone(),
        service_id: model.metadata.service_id.clone(),
        ops,
        provenance: Provenance {
            cli_version: cli_version.to_string(),
            model_path: path.to_string_lossy().to_string(),
            model_sha256: sha256_file(path),
            api_version: api_version.to_string(),
        },
    })
}

fn build_op(api_name: &str, op: &Operation, model: &ServiceModel) -> OpEntry {
    let input = build_input_params(op, model);
    let (output_fields, output_shape_name) = build_output_fields(op, model);
    OpEntry {
        cli_name: xform_name(api_name, '-'),
        api_name: api_name.to_string(),
        docs: strip_html(&op.documentation),
        input,
        output_fields,
        output_shape_name,
    }
}

fn build_input_params(op: &Operation, model: &ServiceModel) -> Vec<InputParam> {
    let Some(input) = &op.input else {
        return Vec::new();
    };
    let Some(shape) = model.shapes.get(&input.shape) else {
        return Vec::new();
    };
    let required = shape.required.to_vec();
    let mut params = Vec::new();
    for (member_name, member) in &shape.members {
        let type_info = resolve_type(&member.shape, model);
        params.push(InputParam {
            cli_name: xform_name(member_name, '-'),
            api_name: member_name.clone(),
            required: required.contains(member_name) && !member.idempotency_token,
            type_name: type_info.type_name.clone(),
            enum_values: type_info.enum_values,
            is_list: type_info.is_list,
            is_map: type_info.is_map,
            is_structure: type_info.is_structure,
            docs: strip_html(&member.documentation),
        });
    }
    params.sort_by(|a, b| {
        b.required
            .cmp(&a.required)
            .then_with(|| a.cli_name.cmp(&b.cli_name))
    });
    params
}

#[derive(Default)]
struct TypeInfo {
    type_name: String,
    enum_values: Vec<String>,
    is_list: bool,
    is_map: bool,
    is_structure: bool,
}

/// Resolve a shape reference to a leaf type while detecting list/map/structure
/// wrappers. Recursion is bounded; shapes form a DAG so cycles don't occur.
fn resolve_type(shape_name: &str, model: &ServiceModel) -> TypeInfo {
    let mut info = TypeInfo::default();
    let mut current = shape_name.to_string();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 16 {
            info.type_name = "unknown".to_string();
            return info;
        }
        let Some(shape) = model.shapes.get(&current) else {
            info.type_name = "unknown".to_string();
            return info;
        };
        match shape.type_name.as_str() {
            "structure" => {
                info.is_structure = true;
                // Resolve to the first non-list/map leaf for display purposes,
                // but keep is_structure so the builder can offer a JSON form.
                if let Some(member) = shape.members.values().next() {
                    current = member.shape.clone();
                    info.type_name = "structure".to_string();
                    continue;
                }
                info.type_name = "structure".to_string();
                return info;
            }
            "list" => {
                info.is_list = true;
                let Some(member) = &shape.member else {
                    info.type_name = "list".to_string();
                    return info;
                };
                current = member.shape.clone();
            }
            "map" => {
                info.is_map = true;
                let Some(value) = &shape.value else {
                    info.type_name = "map".to_string();
                    return info;
                };
                current = value.shape.clone();
            }
            other => {
                info.type_name = other.to_string();
                info.enum_values = shape.enum_values.clone();
                return info;
            }
        }
    }
}

fn build_output_fields(op: &Operation, model: &ServiceModel) -> (Vec<OutputField>, Option<String>) {
    let Some(output) = &op.output else {
        return (Vec::new(), None);
    };
    let shape_name = output.shape.clone();
    let mut fields = Vec::new();
    walk_output(&shape_name, model, &mut Vec::new(), false, &mut fields);
    fields.sort_by(|a, b| a.path.cmp(&b.path));
    (fields, Some(shape_name))
}

fn walk_output(
    shape_name: &str,
    model: &ServiceModel,
    segments: &mut Vec<(String, bool)>,
    _parent_is_list: bool,
    fields: &mut Vec<OutputField>,
) {
    walk_output_inner(shape_name, model, segments, _parent_is_list, fields, 0);
}

fn walk_output_inner(
    shape_name: &str,
    model: &ServiceModel,
    segments: &mut Vec<(String, bool)>,
    _parent_is_list: bool,
    fields: &mut Vec<OutputField>,
    depth: usize,
) {
    if depth > 16 {
        return;
    }
    let Some(shape) = model.shapes.get(shape_name) else {
        return;
    };
    match shape.type_name.as_str() {
        "list" => {
            if let Some(member) = &shape.member {
                // List elements: JMESPath projection uses []
                let mut segs = segments.clone();
                if let Some(last) = segs.last_mut() {
                    last.1 = true; // mark the enclosing segment as a list
                }
                walk_output_inner(&member.shape, model, &mut segs, true, fields, depth + 1);
            }
        }
        "map" => {
            if let Some(value) = &shape.value {
                walk_output_inner(
                    &value.shape,
                    model,
                    segments,
                    _parent_is_list,
                    fields,
                    depth + 1,
                );
            }
        }
        "structure" => {
            for (member_name, member) in &shape.members {
                let mut segs = segments.clone();
                // JMESPath operates on raw JSON keys (camelCase), not CLI names.
                segs.push((member_name.clone(), false));
                push_member(member, &mut segs, model, fields, depth + 1);
            }
        }
        _ => {
            if let Some(last) = segments.last() {
                fields.push(OutputField {
                    path: build_jmespath(segments),
                    name: last.0.clone(),
                    kind: shape.type_name.clone(),
                    is_list_leaf: segments.iter().any(|(_, is_list)| *is_list),
                });
            }
        }
    }
}

fn push_member(
    member: &Member,
    segments: &mut Vec<(String, bool)>,
    model: &ServiceModel,
    fields: &mut Vec<OutputField>,
    depth: usize,
) {
    if depth > 16 {
        return;
    }
    // Determine if this member's shape is a list at the top.
    let is_list = model
        .shapes
        .get(&member.shape)
        .map(|s| s.type_name == "list")
        .unwrap_or(false);
    if let Some(last) = segments.last_mut()
        && is_list
    {
        last.1 = true;
    }
    walk_output_inner(&member.shape, model, segments, is_list, fields, depth + 1);
}

fn build_jmespath(segments: &[(String, bool)]) -> String {
    let mut out = String::new();
    for (i, (seg, is_list)) in segments.iter().enumerate() {
        out.push_str(seg);
        if i < segments.len() - 1 {
            if *is_list {
                out.push_str("[]");
            }
            out.push('.');
        }
    }
    out
}

/// Minimal HTML tag stripping (the models embed `<p>`, `<code>`, etc.).
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn find_repo() -> PathBuf {
        // Tests run from the crate dir; look upward for aws-cli.
        let cwd = std::env::current_dir().unwrap();
        for ancestor in cwd.ancestors() {
            let cand = ancestor
                .join("aws-cli")
                .join("awscli")
                .join("botocore")
                .join("data");
            if cand.is_dir() {
                return cand;
            }
        }
        panic!("aws-cli data dir not found")
    }

    #[test]
    fn parses_ec2() {
        let dir = find_repo();
        let tree = load_services(&dir, "2.36.18");
        let ec2 = tree
            .services
            .iter()
            .find(|s| s.cli_name == "ec2")
            .expect("ec2 present");
        let subnets = ec2
            .ops
            .iter()
            .find(|o| o.cli_name == "describe-subnets")
            .expect("describe-subnets present");
        assert_eq!(subnets.api_name, "DescribeSubnets");
        assert!(
            subnets
                .output_fields
                .iter()
                .any(|f| f.path == "Subnets[].SubnetId")
        );
        assert!(
            subnets
                .output_fields
                .iter()
                .any(|f| f.name == "SubnetId" && f.is_list_leaf)
        );
        assert!(!ec2.provenance.model_sha256.is_empty());
        assert_eq!(ec2.provenance.api_version, "2016-11-15");
    }

    #[test]
    fn parses_parameter_shapes() {
        let dir = find_repo();
        let tree = load_services(&dir, "2.36.18");
        let ec2 = tree.services.iter().find(|s| s.cli_name == "ec2").unwrap();
        let run = ec2
            .ops
            .iter()
            .find(|o| o.cli_name == "run-instances")
            .unwrap();
        let max_count = run
            .input
            .iter()
            .find(|p| p.cli_name == "max-count")
            .expect("max-count");
        assert!(max_count.required);
        assert_eq!(max_count.type_name, "integer");
        let image = run
            .input
            .iter()
            .find(|p| p.cli_name == "image-id")
            .expect("image-id");
        assert_eq!(image.type_name, "string");
    }

    #[test]
    fn service_count_sane() {
        let dir = find_repo();
        let tree = load_services(&dir, "2.36.18");
        assert!(
            tree.services.len() > 300,
            "expected many services, got {}",
            tree.services.len()
        );
    }

    #[test]
    fn cached_load_matches_fresh_parse() {
        let dir = find_repo();
        let fresh = load_services(&dir, "2.36.18");
        let cached = load_services_cached(&dir, "2.36.18");
        assert_eq!(cached.services.len(), fresh.services.len());
        // Second call must hit the cache file on disk.
        let cache_files: Vec<_> = cache_dir()
            .into_iter()
            .flat_map(|d| std::fs::read_dir(d).ok())
            .flatten()
            .flatten()
            .collect();
        assert!(!cache_files.is_empty());
    }
}
