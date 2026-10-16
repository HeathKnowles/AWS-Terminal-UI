use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Default wall-clock budget for one AWS CLI invocation.
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Default retry budget for throttled/transient failures.
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Classified failure kind, surfaced distinctly in status and output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Throttled,
    Credentials,
    NotFound,
    Validation,
    Cancelled,
    TimedOut,
    Failed,
}

/// Timeout override via `AWS_TUI_TIMEOUT_SECS` (positive integers only).
pub fn run_timeout() -> Duration {
    std::env::var("AWS_TUI_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_RUN_TIMEOUT)
}

/// Retry override via `AWS_TUI_MAX_RETRIES` (0 disables retries).
pub fn max_retries() -> u32 {
    std::env::var("AWS_TUI_MAX_RETRIES")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_RETRIES)
}

/// Classify a completed or failed invocation from its outputs.
pub fn classify_result(out: &RunOutput) -> ErrorKind {
    classify_text(&out.stdout, &out.stderr, out.exit_code)
}

pub fn classify_error_text(text: &str) -> ErrorKind {
    classify_text("", text, -1)
}

fn classify_text(stdout: &str, stderr: &str, exit_code: i32) -> ErrorKind {
    if exit_code == 0 {
        return ErrorKind::Failed;
    }
    let hay = format!("{stdout}\n{stderr}");
    let has = |needles: &[&str]| needles.iter().any(|n| hay.contains(n));
    if has(&[
        "ExpiredToken",
        "InvalidClientTokenId",
        "UnauthorizedOperation",
        "Unauthorized",
        "AccessDenied",
        "InvalidIdentityToken",
        "SSO",
        "sso",
        "TokenRefresh",
        "NoCredentialsError",
        "NoRegionError",
        "Unable to locate credentials",
        "AuthFailure",
    ]) {
        return ErrorKind::Credentials;
    }
    if has(&[
        "Throttl",
        "TooManyRequests",
        "RequestLimitExceeded",
        "SlowDown",
        "PriorRequestNotComplete",
        "RequestTimeout",
        "ServiceUnavailable",
        "503",
        "Connection reset",
        "Connection timed out",
        "EndpointConnectionError",
        "Read timeout",
    ]) {
        return ErrorKind::Throttled;
    }
    if has(&["NotFound", "NoSuch", "DoesNotExist", "404"]) {
        return ErrorKind::NotFound;
    }
    if has(&[
        "Unknown options",
        "Invalid",
        "ValidationError",
        "ValidationException",
        "MissingParameter",
        "Parameter validation failed",
    ]) {
        return ErrorKind::Validation;
    }
    ErrorKind::Failed
}

/// Human hint for a classified failure.
pub fn error_hint(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Throttled => "throttled — backed off and retried",
        ErrorKind::Credentials => "check credentials/region (aws sso login, AWS_PROFILE)",
        ErrorKind::NotFound => "resource not found in this account/region",
        ErrorKind::Validation => "check parameters against the Details pane",
        ErrorKind::Cancelled => "cancelled by user",
        ErrorKind::TimedOut => "timed out waiting for aws",
        ErrorKind::Failed => "command failed",
    }
}

pub fn run_command(argv: &[String], stdin: Option<&str>) -> Result<RunOutput, String> {
    run_command_cancelable(argv, stdin, &AtomicBool::new(false), run_timeout())
}

/// Run a command with cooperative cancellation and a wall-clock timeout.
///
/// Stdout/stderr are drained on reader threads so large outputs cannot
/// deadlock the wait loop. On cancel the child is killed and an error is
/// returned; the caller decides whether to keep any partial output.
pub fn run_command_cancelable(
    argv: &[String],
    stdin: Option<&str>,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<RunOutput, String> {
    if argv.is_empty() {
        return Err("empty argv".to_string());
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    if let Some(input) = stdin {
        use std::io::Write;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes());
        }
    }
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut out) = stdout.take() {
            let _ = out.read_to_end(&mut buf);
        }
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut err) = stderr.take() {
            let _ = err.read_to_end(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_handle.join();
            let _ = err_handle.join();
            return Err("cancelled by user".to_string());
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_handle.join();
            let _ = err_handle.join();
            return Err(format!("timed out after {}s", timeout.as_secs()));
        }
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                let stdout = out_handle.join().unwrap_or_default();
                let stderr = err_handle.join().unwrap_or_default();
                return Ok(RunOutput {
                    exit_code: status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&stdout).to_string(),
                    stderr: String::from_utf8_lossy(&stderr).to_string(),
                });
            }
            None => {
                // Sleep in small slices so cancel/timeout stay responsive.
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Run with bounded retries for throttled/transient failures.
///
/// Only completed invocations classified as [`ErrorKind::Throttled`] are
/// retried, with exponential backoff (500ms, 1s, 2s, ...). Cancel and
/// timeout apply across all attempts. Returns the last outcome.
pub fn run_with_retries(
    argv: &[String],
    cancel: &AtomicBool,
    timeout: Duration,
    retries: u32,
) -> Result<RunOutput, String> {
    let mut attempt = 0u32;
    loop {
        {
            let out = run_command_cancelable(argv, None, cancel, timeout)?;
            if out.exit_code != 0
                && classify_result(&out) == ErrorKind::Throttled
                && attempt < retries
            {
                attempt += 1;
                interruptible_sleep(
                    Duration::from_millis(500 * 2_u64.pow(attempt.saturating_sub(1).min(4))),
                    cancel,
                );
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled by user".to_string());
                }
                continue;
            }
            return Ok(out);
        }
    }
}

fn interruptible_sleep(d: Duration, cancel: &AtomicBool) {
    let start = Instant::now();
    while start.elapsed() < d {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Validate that a JMESPath query compiles.
pub fn validate_query(query: &str) -> Result<(), String> {
    jmespath::compile(query)
        .map(|_| ())
        .map_err(|e| format!("invalid JMESPath query: {e}"))
}

/// Apply a JMESPath query to already-fetched JSON (for "of this" follow-ups).
pub fn apply_query(json: &serde_json::Value, query: &str) -> Result<serde_json::Value, String> {
    let expr = jmespath::compile(query).map_err(|e| e.to_string())?;
    let var = jmespath::Variable::from_serializable(json).map_err(|e| e.to_string())?;
    let result = expr.search(var).map_err(|e| e.to_string())?;
    serde_json::to_value(&result).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn cancelable_run_reports_success() {
        let out = run_command_cancelable(
            &["echo".to_string(), "hi".to_string()],
            None,
            &AtomicBool::new(false),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout.trim(), "hi");
    }

    #[test]
    fn cancelable_run_times_out() {
        let err = run_command_cancelable(
            &["sleep".to_string(), "5".to_string()],
            None,
            &AtomicBool::new(false),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(err.contains("timed out"), "got: {err}");
    }

    #[test]
    fn cancelable_run_honors_cancel_flag() {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let handle = std::thread::spawn(move || {
            run_command_cancelable(
                &["sleep".to_string(), "5".to_string()],
                None,
                &worker_cancel,
                Duration::from_secs(30),
            )
        });
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::Relaxed);
        let err = handle.join().unwrap().unwrap_err();
        assert!(err.contains("cancelled"), "got: {err}");
    }

    #[test]
    fn error_classification_is_distinct() {
        let creds = RunOutput {
            exit_code: 254,
            stdout: String::new(),
            stderr: "An error occurred (ExpiredToken) when calling".to_string(),
        };
        assert_eq!(classify_result(&creds), ErrorKind::Credentials);
        let throttle = RunOutput {
            exit_code: 254,
            stdout: String::new(),
            stderr: "Rate exceeded (ThrottlingException)".to_string(),
        };
        assert_eq!(classify_result(&throttle), ErrorKind::Throttled);
        let ok = RunOutput {
            exit_code: 0,
            stdout: "{}".to_string(),
            stderr: String::new(),
        };
        assert_eq!(classify_result(&ok), ErrorKind::Failed);
    }

    #[test]
    fn retry_budget_defaults_and_parses() {
        assert_eq!(max_retries(), DEFAULT_MAX_RETRIES);
    }
}
