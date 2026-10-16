use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// AWS regions known to be usable for the public AWS partition.
pub const KNOWN_REGIONS: &[&str] = &[
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
    "af-south-1",
    "ap-east-1",
    "ap-south-1",
    "ap-south-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ca-central-1",
    "ca-west-1",
    "eu-central-1",
    "eu-central-2",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "mx-central-1",
    "sa-east-1",
    "us-gov-east-1",
    "us-gov-west-1",
    "cn-north-1",
    "cn-northwest-1",
];

/// Resolved AWS environment for a session: profiles + regions available to
/// pick from, plus the active defaults detected from env and ~/.aws config.
#[derive(Debug, Clone, Default)]
pub struct AwsEnvironment {
    pub profiles: Vec<String>,
    pub regions: Vec<String>,
    pub active_profile: String,
    pub active_region: String,
    pub region_source: String,
}

pub fn aws_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".aws")
}

/// Read `~/.aws/config` and `~/.aws/credentials` (if present) and return the
/// profile names found in either, preserving order.
pub fn discover_profiles() -> Vec<String> {
    discover_profiles_in(&aws_config_dir())
}

/// Testable variant that reads from an explicit directory.
///
/// This exists so tests don't have to mutate the global `HOME` environment
/// variable (which is process-wide and racy under parallel test execution).
pub fn discover_profiles_in(dir: &Path) -> Vec<String> {
    let mut profiles: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for file in ["config", "credentials"] {
        let path = dir.join(file);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for name in parse_profile_names(&text) {
            if seen.insert(name.clone()) {
                profiles.push(name);
            }
        }
    }
    if profiles.is_empty() {
        profiles.push("default".to_string());
    }
    profiles
}

/// Extract `[profile x]` / `[default]` section names from an INI-style file.
fn parse_profile_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('[') || !line.ends_with(']') {
            continue;
        }
        let section = &line[1..line.len() - 1];
        if let Some(rest) = section.strip_prefix("profile ") {
            let name = rest.trim().to_string();
            if !name.is_empty() {
                names.push(name);
            }
        } else if section == "default" {
            names.push("default".to_string());
        }
    }
    names
}

/// Testable variant of profile-region lookup with an explicit config dir.
pub fn profile_region_in(dir: &Path, profile: &str) -> Option<String> {
    for file in ["config", "credentials"] {
        let path = dir.join(file);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(r) = parse_region_for_profile(&text, profile) {
            return Some(r);
        }
    }
    None
}

/// Parse `region`/`sso_region` out of a specific profile section.
fn parse_region_for_profile(text: &str, target: &str) -> Option<String> {
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            let section = &line[1..line.len() - 1];
            current = if section == "default" {
                Some("default".to_string())
            } else if let Some(rest) = section.strip_prefix("profile ") {
                Some(rest.trim().to_string())
            } else {
                // credentials files use bare profile names
                Some(section.to_string())
            };
            continue;
        }
        if current.as_deref() != Some(target) {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if key == "region" || key == "sso_region" {
                return Some(value);
            }
        }
    }
    None
}

/// Resolve the active profile from AWS_PROFILE or the default.
pub fn active_profile() -> String {
    active_profile_from(&current_env_map())
}

/// Testable variant of [`active_profile`] that reads from a map.
pub fn active_profile_from(env: &HashMap<String, String>) -> String {
    env.get("AWS_PROFILE")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

fn current_env_map() -> HashMap<String, String> {
    ["AWS_PROFILE", "AWS_REGION", "AWS_DEFAULT_REGION"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect()
}

/// Resolve the active region. Precedence:
/// 1. AWS_REGION env
/// 2. AWS_DEFAULT_REGION env
/// 3. profile region in ~/.aws config
/// 4. us-east-1 (AWS's own default)
///
/// Returns (region, source_description).
pub fn detect_region(profile: &str) -> (String, String) {
    detect_region_with(&aws_config_dir(), &current_env_map(), profile)
}

/// Testable variant of [`detect_region`] with explicit inputs.
pub fn detect_region_with(
    config_dir: &Path,
    env: &HashMap<String, String>,
    profile: &str,
) -> (String, String) {
    for (key, label) in [
        ("AWS_REGION", "env AWS_REGION"),
        ("AWS_DEFAULT_REGION", "env AWS_DEFAULT_REGION"),
    ] {
        if let Some(v) = env.get(key)
            && !v.trim().is_empty()
        {
            return (v.trim().to_string(), label.to_string());
        }
    }
    if let Some(r) = profile_region_in(config_dir, profile) {
        return (r, format!("profile '{profile}' region"));
    }
    ("us-east-1".to_string(), "default".to_string())
}

/// Build the full environment: available profiles, known regions merged with
/// any config regions, and the active profile/region defaults.
pub fn build_environment() -> AwsEnvironment {
    build_environment_with(&aws_config_dir(), &current_env_map())
}

/// Testable variant of [`build_environment`] that takes explicit inputs
/// instead of reading global process state.
pub fn build_environment_with(config_dir: &Path, env: &HashMap<String, String>) -> AwsEnvironment {
    let mut profiles = discover_profiles_in(config_dir);
    profiles.sort();
    let wanted = active_profile_from(env);
    let active_profile = if profiles.contains(&wanted) {
        wanted
    } else {
        profiles[0].clone()
    };
    let (active_region, region_source) = detect_region_with(config_dir, env, &active_profile);

    let mut regions: Vec<String> = Vec::new();
    for p in &profiles {
        if let Some(r) = profile_region_in(config_dir, p)
            && !regions.contains(&r)
        {
            regions.push(r);
        }
    }
    for r in KNOWN_REGIONS {
        if !regions.contains(&r.to_string()) {
            regions.push((*r).to_string());
        }
    }
    if !regions.contains(&active_region) {
        regions.push(active_region.clone());
    }

    AwsEnvironment {
        profiles,
        regions,
        active_profile,
        active_region,
        region_source,
    }
}

/// Merge a new "env override" map of region sources, kept for tests/debugging.
pub fn region_from_env_map(map: &HashMap<String, String>) -> Option<String> {
    for k in ["AWS_REGION", "AWS_DEFAULT_REGION"] {
        if let Some(v) = map.get(k)
            && !v.trim().is_empty()
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profile_names() {
        let text = "\n[default]\nregion = us-east-1\n\n[profile dev]\nregion = eu-west-1\nsso_region = eu-west-1\n\n[profile prod]\nregion = ap-south-1\n";
        let names = parse_profile_names(text);
        assert_eq!(names, vec!["default", "dev", "prod"]);
    }

    #[test]
    fn parses_region_for_profile() {
        let text = "\n[default]\nregion = us-east-1\n\n[profile dev]\nregion = eu-west-1\n\n[profile sso]\nsso_region = ap-south-1\n";
        assert_eq!(
            parse_region_for_profile(text, "dev"),
            Some("eu-west-1".to_string())
        );
        assert_eq!(
            parse_region_for_profile(text, "sso"),
            Some("ap-south-1".to_string())
        );
        assert_eq!(
            parse_region_for_profile(text, "default"),
            Some("us-east-1".to_string())
        );
        assert_eq!(parse_region_for_profile(text, "nope"), None);
    }

    #[test]
    fn credentials_bare_sections_work() {
        let text = "\n[default]\naws_access_key_id = x\n\n[dev]\nregion = eu-central-1\n";
        assert_eq!(
            parse_region_for_profile(text, "dev"),
            Some("eu-central-1".to_string())
        );
    }

    #[test]
    fn env_override_wins() {
        let mut map = HashMap::new();
        map.insert("AWS_REGION".to_string(), "ap-south-2".to_string());
        assert_eq!(region_from_env_map(&map), Some("ap-south-2".to_string()));
        map.insert("AWS_DEFAULT_REGION".to_string(), "us-west-2".to_string());
        assert_eq!(region_from_env_map(&map), Some("ap-south-2".to_string()));
    }

    #[test]
    fn build_environment_uses_fake_home() {
        // No global env mutation: use an isolated config dir + env map.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "aws-tui-test-{}-{}-{:?}",
            std::process::id(),
            nanos,
            std::thread::current().id()
        ));
        let aws = tmp.join(".aws");
        std::fs::create_dir_all(&aws).unwrap();
        std::fs::write(
            aws.join("config"),
            "[default]\nregion = us-east-1\n\n[profile dev]\nregion = eu-west-1\n",
        )
        .unwrap();
        let env = HashMap::new();
        let built = build_environment_with(&aws, &env);
        assert!(built.profiles.contains(&"dev".to_string()));
        assert!(built.regions.contains(&"eu-west-1".to_string()));
        // default profile region is us-east-1
        assert_eq!(built.active_region, "us-east-1");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn env_map_overrides_profile_region() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "aws-tui-test-env-{}-{}-{:?}",
            std::process::id(),
            nanos,
            std::thread::current().id()
        ));
        let aws = tmp.join(".aws");
        std::fs::create_dir_all(&aws).unwrap();
        std::fs::write(aws.join("config"), "[default]\nregion = us-east-1\n").unwrap();
        let mut env = HashMap::new();
        env.insert("AWS_REGION".to_string(), "eu-central-1".to_string());
        let built = build_environment_with(&aws, &env);
        assert_eq!(built.active_region, "eu-central-1");
        assert_eq!(built.region_source, "env AWS_REGION");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
