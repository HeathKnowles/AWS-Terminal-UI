use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct RunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_command(argv: &[String], stdin: Option<&str>) -> Result<RunOutput, String> {
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
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(RunOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
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
