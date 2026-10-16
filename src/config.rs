use std::path::PathBuf;
use std::time::Duration;

/// File-backed defaults. Every field is optional; environment variables win
/// over the file, and built-ins win over nothing.
///
/// Path: `$AWS_TUI_CONFIG`, else `$XDG_CONFIG_HOME/aws-tui/config.toml`
/// (usually `~/.config/aws-tui/config.toml`). Missing or invalid files are
/// ignored with a stderr note.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Config {
    pub default_region: Option<String>,
    pub default_profile: Option<String>,
    pub timeout_secs: Option<u64>,
    pub max_retries: Option<u32>,
    pub read_only: Option<bool>,
    pub allow_prod_mutations: Option<bool>,
    pub prod_markers: Option<Vec<String>>,
    pub audit_max_mb: Option<u64>,
}

impl Config {
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("AWS_TUI_CONFIG") {
            return PathBuf::from(p);
        }
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aws-tui")
            .join("config.toml")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &std::path::Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match toml::from_str(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("ignoring config {}: {e}", path.display());
                Self::default()
            }
        }
    }

    pub fn timeout(&self) -> Duration {
        if let Ok(v) = std::env::var("AWS_TUI_TIMEOUT_SECS")
            && let Ok(s) = v.trim().parse::<u64>()
            && s > 0
        {
            return Duration::from_secs(s);
        }
        if let Some(s) = self.timeout_secs
            && s > 0
        {
            return Duration::from_secs(s);
        }
        crate::aws::runner::DEFAULT_RUN_TIMEOUT
    }

    pub fn retries(&self) -> u32 {
        if let Ok(v) = std::env::var("AWS_TUI_MAX_RETRIES")
            && let Ok(n) = v.trim().parse::<u32>()
        {
            return n;
        }
        self.max_retries
            .unwrap_or(crate::aws::runner::DEFAULT_MAX_RETRIES)
    }

    pub fn read_only(&self) -> bool {
        if let Ok(v) = std::env::var("AWS_TUI_READ_ONLY")
            && matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes")
        {
            return true;
        }
        self.read_only.unwrap_or(false)
    }

    pub fn allow_prod_mutations(&self) -> bool {
        if let Ok(v) = std::env::var("AWS_TUI_ALLOW_PROD_MUTATIONS")
            && matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes")
        {
            return true;
        }
        self.allow_prod_mutations.unwrap_or(false)
    }

    pub fn prod_markers(&self) -> Vec<String> {
        self.prod_markers
            .clone()
            .unwrap_or_else(|| vec!["prod".to_string(), "production".to_string()])
    }

    pub fn audit_max_bytes(&self) -> u64 {
        if let Ok(v) = std::env::var("AWS_TUI_AUDIT_MAX_MB")
            && let Ok(n) = v.trim().parse::<u64>()
            && n > 0
        {
            return n * 1024 * 1024;
        }
        self.audit_max_mb.unwrap_or(10) * 1024 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let cfg = Config::load_from(std::path::Path::new("/definitely/not/here.toml"));
        assert_eq!(cfg.timeout(), crate::aws::runner::DEFAULT_RUN_TIMEOUT);
        assert!(!cfg.read_only());
    }

    #[test]
    fn file_values_parse() {
        let dir = std::env::temp_dir().join(format!(
            "aws-tui-cfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "read_only = true\ntimeout_secs = 30\nprod_markers = [\"prod\", \"live\"]\n",
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert!(cfg.read_only());
        assert_eq!(cfg.timeout(), Duration::from_secs(30));
        assert_eq!(
            cfg.prod_markers(),
            vec!["prod".to_string(), "live".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
