use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use crate::aws::awsconfig::AwsEnvironment;
use crate::aws::builder::{Intent, validate_intent};
use crate::aws::index::SearchIndex;
use crate::aws::loader::{Provenance, ServiceEntry, ServiceTree};
use crate::aws::runner::{
    apply_query, classify_error_text, classify_result, error_hint, run_with_retries, validate_query,
};
use crate::ops::audit::AuditLogger;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::CrosstermBackend;

use crate::tui::jsonview::{ViewRow, pretty_json, view_rows};
use crate::tui::ui;

pub const CLI_VERSION: &str = "2.36.18";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Search,
    Results,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picker {
    Region,
    Profile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputView {
    /// Tree of JSON blocks at the current drill level (default).
    Blocks,
    /// Full pretty-printed JSON of the current block.
    Json,
}

#[derive(Debug, Clone)]
pub struct DrillLevel {
    /// Label (key name / index) of the folder that was opened.
    pub label: String,
    /// The parent value we came from (for drilling back up).
    pub json: serde_json::Value,
    /// Tree-sitter style preview of the folder contents.
    pub preview: String,
}
/// One input field in the parameters popup.
pub struct ParamField {
    pub cli_name: String,
    pub type_label: String,
    pub type_name: String,
    pub enum_values: Vec<String>,
    pub is_list: bool,
    pub is_structure: bool,
    pub docs: String,
    pub value: String,
    pub required: bool,
}

pub struct ParamPopup {
    pub fields: Vec<ParamField>,
    pub active: usize,
    pub error: Option<String>,
    pub query: String,
    pub output: String,
    pub max_items: String,
    pub page_size: String,
    pub scroll: usize,
}

impl ParamPopup {
    pub fn total_rows(&self) -> usize {
        self.fields.len() + 4
    }
}

/// Typed confirmation for a production mutation.
pub struct DangerConfirm {
    pub prompt: String,
    pub expected: String,
    pub text: String,
    pub error: Option<String>,
}

/// Allowed `--output` values accepted by the AWS CLI.
pub const ALLOWED_OUTPUTS: &[&str] = &["json", "text", "table", "yaml", "yaml-stream"];

/// Result delivered by a background worker thread.
pub struct PendingResult {
    pub result: Result<crate::aws::runner::RunOutput, String>,
    pub duration_ms: u64,
}

/// A command currently executing off the UI thread.
pub struct PendingRun {
    pub command_line: String,
    pub started: Instant,
    pub handle: Option<std::thread::JoinHandle<PendingResult>>,
    pub cancel: Arc<AtomicBool>,
    pub intent: Intent,
    pub provenance: Provenance,
    pub reference: u64,
    pub source: String,
}

/// One row of the command-history overlay, parsed from stored audit JSON.
#[derive(Debug, Clone, Default)]
pub struct HistoryItem {
    pub seq: u64,
    pub service: String,
    pub operation: String,
    pub exit: Option<i32>,
    pub ts: String,
}

pub struct App {
    pub tree: ServiceTree,
    pub index: SearchIndex,
    pub audit: AuditLogger,
    pub aws_bin: String,
    pub env: AwsEnvironment,

    pub query: String,
    pub results: Vec<(usize, u32)>,
    pub selected: Option<usize>,

    pub intent: Option<Intent>,
    pub validation_error: Option<String>,
    pub output: String,
    pub provenance_line: String,
    pub last_exit: Option<i32>,
    pub output_scroll: u16,
    pub details_scroll: u16,
    pub view: OutputView,

    pub focus: Focus,
    pub picker: Option<Picker>,
    pub picker_idx: usize,
    pub status: String,

    pub last_json: Option<serde_json::Value>,
    pub last_source: String,

    pub table_selected: Option<usize>,
    pub drill_stack: Vec<DrillLevel>,

    pub param_popup: Option<ParamPopup>,

    pub plan: Option<crate::ops::plan::Plan>,
    pub plan_source: String,
    pub plan_error: Option<String>,
    pub show_plan: bool,
    pub plan_scroll: u16,

    pub incident: Option<crate::ops::incident::Incident>,
    pub incident_source: String,
    pub incident_error: Option<String>,
    pub show_incident: bool,
    pub incident_scroll: u16,
    pub linked_plan_title: Option<String>,
    pub linked_plan_steps: usize,
    pub linked_plan_error: Option<String>,

    pub pending: Option<PendingRun>,

    pub show_history: bool,
    pub history_items: Vec<HistoryItem>,
    pub history_idx: usize,
    pub history_scroll: usize,
    pub history_error: Option<String>,

    pub show_inbox: bool,
    pub inbox_items: Vec<crate::ops::incident::Incident>,
    pub inbox_idx: usize,
    pub inbox_scroll: usize,
    pub inbox_error: Option<String>,

    pub timeout: Duration,
    pub retries: u32,
    pub read_only: bool,
    pub allow_prod_mutations: bool,
    pub prod_markers: Vec<String>,
    pub danger: Option<DangerConfirm>,
    pub show_help: bool,
    pub help_scroll: u16,
}

impl App {
    pub fn new(
        tree: ServiceTree,
        index: SearchIndex,
        audit: AuditLogger,
        aws_bin: String,
        env: AwsEnvironment,
    ) -> Self {
        let mut app = App {
            tree,
            index,
            audit,
            aws_bin,
            env,
            query: String::new(),
            results: Vec::new(),
            selected: None,
            intent: None,
            validation_error: None,
            output: String::new(),
            provenance_line: String::new(),
            last_exit: None,
            output_scroll: 0,
            details_scroll: 0,
            view: OutputView::Blocks,
            focus: Focus::Search,
            picker: None,
            picker_idx: 0,
            status: String::new(),
            last_json: None,
            last_source: "typed-direct".to_string(),
            table_selected: None,
            drill_stack: Vec::new(),
            param_popup: None,
            plan: None,
            plan_source: String::new(),
            plan_error: None,
            show_plan: false,
            plan_scroll: 0,
            incident: None,
            incident_source: String::new(),
            incident_error: None,
            show_incident: false,
            incident_scroll: 0,
            linked_plan_title: None,
            linked_plan_steps: 0,
            linked_plan_error: None,
            pending: None,
            show_history: false,
            history_items: Vec::new(),
            history_idx: 0,
            history_scroll: 0,
            history_error: None,
            show_inbox: false,
            inbox_items: Vec::new(),
            inbox_idx: 0,
            inbox_scroll: 0,
            inbox_error: None,
            timeout: crate::aws::runner::DEFAULT_RUN_TIMEOUT,
            retries: crate::aws::runner::DEFAULT_MAX_RETRIES,
            read_only: false,
            allow_prod_mutations: false,
            prod_markers: vec!["prod".to_string(), "production".to_string()],
            danger: None,
            show_help: false,
            help_scroll: 0,
        };
        app.refresh();
        app.build_preview();
        app
    }

    pub fn refresh(&mut self) {
        self.results = self.index.search(&self.query, 200);
        if self.results.is_empty() {
            self.selected = None;
        } else if let Some(sel) = self.selected {
            if !self.results.iter().any(|(i, _)| *i == sel) {
                self.selected = Some(self.results[0].0);
            }
        } else {
            self.selected = Some(self.results[0].0);
        }
    }

    pub fn selected_row(&self) -> Option<&crate::aws::index::SearchRow> {
        self.selected.and_then(|i| self.index.rows.get(i))
    }

    pub fn current_service(&self) -> Option<&ServiceEntry> {
        let row = self.selected_row()?;
        self.tree
            .services
            .iter()
            .find(|s| s.cli_name == row.service)
    }

    pub fn build_preview(&mut self) {
        self.details_scroll = 0;
        let Some(row) = self.selected_row() else {
            self.intent = None;
            return;
        };
        let service = self
            .current_service()
            .map(|s| s.cli_name.clone())
            .unwrap_or_default();
        let op = row.operation.clone();
        if op.is_empty() {
            self.intent = None;
            self.provenance_line = String::new();
            return;
        }
        let intent = Intent {
            service,
            operation: op,
            region: Some(self.env.active_region.clone()),
            profile: Some(self.env.active_profile.clone()),
            output: Some("json".to_string()),
            ..Default::default()
        };
        self.validation_error = validate_intent(&self.index, &intent).err();
        self.intent = Some(intent);
        if let Some(svc) = self.current_service() {
            let p = &svc.provenance;
            let short_sha = p.model_sha256.get(..8).unwrap_or(&p.model_sha256);
            self.provenance_line = format!(
                "fetched from {} (sha256:{}..) aws-cli {} api {}",
                p.model_path, short_sha, p.cli_version, p.api_version
            );
        }
    }

    pub fn is_running(&self) -> bool {
        self.pending.is_some()
    }

    pub fn execute(&mut self) {
        if self.pending.is_some() {
            self.status = "already running (x cancels)".to_string();
            return;
        }
        let Some(intent) = self.intent.clone() else {
            return;
        };
        if self.read_only && Self::is_mutation_op(&intent.operation) {
            self.validation_error = Some("read-only mode: mutations disabled".to_string());
            self.status = "blocked: read-only mode".to_string();
            return;
        }
        if !self.allow_prod_mutations
            && Self::is_mutation_op(&intent.operation)
            && self.is_prod_context()
        {
            self.danger = Some(DangerConfirm {
                prompt: format!(
                    "Production mutation: {}.{} on {}/{}. Type the operation name to confirm.",
                    intent.service,
                    intent.operation,
                    self.env.active_profile,
                    self.env.active_region
                ),
                expected: intent.operation.clone(),
                text: String::new(),
                error: None,
            });
            self.status = "confirmation required".to_string();
            return;
        }
        if let Some(q) = &intent.query
            && let Err(e) = validate_query(q)
        {
            self.validation_error = Some(e);
            return;
        }
        let provenance = self
            .current_service()
            .map(|s| s.provenance.clone())
            .unwrap_or_else(|| Provenance {
                cli_version: CLI_VERSION.to_string(),
                model_path: String::new(),
                model_sha256: String::new(),
                api_version: String::new(),
            });
        let argv = intent.argv(&self.aws_bin);
        let reference = self.audit.propose(&intent, &provenance, &self.last_source);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let timeout = self.timeout;
        let retries = self.retries;
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            let result = run_with_retries(&argv, &worker_cancel, timeout, retries);
            PendingResult {
                result,
                duration_ms: start.elapsed().as_millis() as u64,
            }
        });
        self.status = format!(
            "running: {} (x cancels)",
            intent.masked_command_line(&self.aws_bin)
        );
        self.pending = Some(PendingRun {
            command_line: intent.masked_command_line(&self.aws_bin),
            started: Instant::now(),
            handle: Some(handle),
            cancel,
            intent,
            provenance,
            reference,
            source: self.last_source.clone(),
        });
    }

    pub fn cancel_pending(&mut self) {
        if let Some(pending) = self.pending.as_ref() {
            pending.cancel.store(true, Ordering::Relaxed);
            self.status = format!("cancelling: {}", pending.command_line);
        }
    }

    /// Read verbs are safe to run anywhere; everything else is a mutation.
    pub fn is_mutation_op(operation: &str) -> bool {
        let verb = operation.split('-').next().unwrap_or("");
        !matches!(verb, "describe" | "list" | "get")
    }

    pub fn is_prod_context(&self) -> bool {
        let profile = self.env.active_profile.to_lowercase();
        let region = self.env.active_region.to_lowercase();
        self.prod_markers.iter().any(|m| {
            let m = m.to_lowercase();
            !m.is_empty() && (profile.contains(&m) || region.contains(&m))
        })
    }

    /// Save the current output (pretty JSON when available) to a file.
    pub fn save_output(&mut self) {
        let body = match self.last_json.as_ref() {
            Some(v) => pretty_json(v),
            None => self.output.clone(),
        };
        if body.trim().is_empty() {
            self.status = "nothing to save".to_string();
            return;
        }
        let name = format!(
            "aws-tui-output-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );
        match std::fs::write(&name, &body) {
            Ok(()) => self.status = format!("saved {name}"),
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    /// Join a finished worker and apply its outcome. Returns true when a run
    /// completed on this call. Safe to call every tick.
    pub fn poll_pending(&mut self) -> bool {
        let finished = self
            .pending
            .as_ref()
            .and_then(|p| p.handle.as_ref())
            .map(|h| h.is_finished())
            .unwrap_or(false);
        if !finished {
            return false;
        }
        let Some(mut pending) = self.pending.take() else {
            return false;
        };
        let Some(handle) = pending.handle.take() else {
            return false;
        };
        let outcome = match handle.join() {
            Ok(res) => res,
            Err(_) => PendingResult {
                result: Err("worker panicked".to_string()),
                duration_ms: pending.started.elapsed().as_millis() as u64,
            },
        };
        match outcome.result {
            Ok(out) => {
                self.apply_run_output(
                    &pending.intent,
                    &pending.provenance,
                    &pending.source,
                    Some(pending.reference),
                    &out,
                    outcome.duration_ms,
                );
            }
            Err(e) => {
                let hint = classify_error_text(&e);
                self.output = format!("failed to run: {e}");
                self.output_scroll = 0;
                self.last_exit = Some(-1);
                self.status = format!("failed: {e} ({})", error_hint(hint));
                self.audit.executed(
                    &pending.intent,
                    &pending.provenance,
                    &pending.source,
                    crate::ops::audit::ExecutionOutcome {
                        exit_code: -1,
                        duration_ms: outcome.duration_ms,
                        output_sha256: String::new(),
                        error: Some(e),
                    },
                    Some(pending.reference),
                );
            }
        }
        true
    }

    fn apply_run_output(
        &mut self,
        intent: &Intent,
        provenance: &Provenance,
        source: &str,
        reference: Option<u64>,
        out: &crate::aws::runner::RunOutput,
        duration_ms: u64,
    ) {
        let duration = duration_ms;
        let output_sha = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(out.stdout.as_bytes());
            h.finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        self.last_json = serde_json::from_str(&out.stdout).ok();
        self.output = out.stdout.clone();
        self.output_scroll = 0;
        self.table_selected = None;
        self.drill_stack.clear();
        self.auto_drill();
        self.view = OutputView::Blocks;
        self.last_exit = Some(out.exit_code);
        self.audit.executed(
            intent,
            provenance,
            source,
            crate::ops::audit::ExecutionOutcome {
                exit_code: out.exit_code,
                duration_ms: duration,
                output_sha256: output_sha,
                error: if out.stderr.is_empty() {
                    None
                } else {
                    Some(out.stderr.clone())
                },
            },
            reference,
        );
        if !out.stderr.is_empty() {
            self.output.push_str("\n--- stderr ---\n");
            self.output.push_str(&out.stderr);
        }
        if out.exit_code == 0 {
            self.status = format!("ok (exit 0) in {}ms", duration);
        } else {
            let kind = classify_result(out);
            self.status = format!(
                "failed (exit {}) in {}ms ({})",
                out.exit_code,
                duration,
                error_hint(kind)
            );
        }
        self.last_source = "history-rerun".to_string();
    }

    /// If the current operation declares required input params, open the
    /// params editor; otherwise execute directly (fast path for Enter).
    pub fn open_param_popup(&mut self) {
        let Some(svc) = self.current_service() else {
            return;
        };
        let Some(intent) = self.intent.clone() else {
            return;
        };
        let Some(op) = svc.ops.iter().find(|o| o.cli_name == intent.operation) else {
            return;
        };
        if !op.input.iter().any(|p| p.required) {
            self.execute();
            return;
        }
        self.open_param_editor();
    }

    /// Always open the full params editor (required + optional params,
    /// `--query`, `--output`), even when nothing is required.
    pub fn open_param_editor(&mut self) {
        let Some(intent) = self.intent.clone() else {
            return;
        };
        let Some(svc) = self.current_service() else {
            return;
        };
        let Some(op) = svc.ops.iter().find(|o| o.cli_name == intent.operation) else {
            return;
        };
        // Pre-fill from any params already present in the intent (e.g. from a
        // natural-language hint) so reruns don't lose values.
        let existing: Vec<(String, String)> = intent.params.clone();
        let fields: Vec<ParamField> = op
            .input
            .iter()
            .map(|p| {
                let value = existing
                    .iter()
                    .find(|(k, _)| k == &p.cli_name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                ParamField {
                    cli_name: p.cli_name.clone(),
                    type_label: if p.is_structure {
                        "json".to_string()
                    } else {
                        p.type_name.clone()
                    },
                    type_name: p.type_name.clone(),
                    enum_values: p.enum_values.clone(),
                    is_list: p.is_list,
                    is_structure: p.is_structure,
                    docs: p.docs.clone(),
                    value,
                    required: p.required,
                }
            })
            .collect();
        self.param_popup = Some(ParamPopup {
            fields,
            active: 0,
            error: None,
            query: intent.query.clone().unwrap_or_default(),
            output: intent.output.clone().unwrap_or_default(),
            max_items: intent.max_items.clone().unwrap_or_default(),
            page_size: intent.page_size.clone().unwrap_or_default(),
            scroll: 0,
        });
    }

    /// Validate the popup fields and, when they pass, write them back into
    /// the intent and run the command. Empty optional params are skipped;
    /// an empty query/output clears that flag.
    pub fn confirm_params(&mut self) {
        let (params, query, output, max_items, page_size, validation_error) =
            match self.param_popup.as_ref() {
                Some(popup) => match collect_popup_values(popup) {
                    Ok((params, query, output, max_items, page_size)) => (
                        Some(params),
                        Some(query),
                        Some(output),
                        Some(max_items),
                        Some(page_size),
                        None,
                    ),
                    Err(e) => (None, None, None, None, None, Some(e)),
                },
                None => return,
            };
        if let Some(err) = validation_error {
            if let Some(popup) = self.param_popup.as_mut() {
                popup.error = Some(err);
            }
            return;
        }
        let (Some(params), Some(query), Some(output), Some(max_items), Some(page_size)) =
            (params, query, output, max_items, page_size)
        else {
            return;
        };
        // Final cross-check against known params/query rules before running.
        let candidate = match self.intent.clone() {
            Some(mut intent) => {
                intent.params = params;
                intent.query = query;
                intent.output = output;
                intent.max_items = max_items;
                intent.page_size = page_size;
                intent
            }
            None => return,
        };
        if self.read_only && Self::is_mutation_op(&candidate.operation) {
            if let Some(popup) = self.param_popup.as_mut() {
                popup.error = Some("read-only mode: mutations disabled".to_string());
            }
            return;
        }
        if let Err(e) = validate_intent(&self.index, &candidate) {
            if let Some(popup) = self.param_popup.as_mut() {
                popup.error = Some(e);
            }
            return;
        }
        self.param_popup = None;
        if let Some(intent) = self.intent.as_mut() {
            intent.params = candidate.params;
            intent.query = candidate.query;
            intent.output = candidate.output;
            intent.max_items = candidate.max_items;
            intent.page_size = candidate.page_size;
        }
        self.execute();
    }

    pub fn default_plan_path() -> std::path::PathBuf {
        if let Ok(p) = std::env::var("AWS_TUI_PLAN") {
            return std::path::PathBuf::from(p);
        }
        if let Ok(cwd) = std::env::current_dir() {
            let direct = cwd.join("plans").join("example.json");
            if direct.is_file() {
                return direct;
            }
            for ancestor in cwd.ancestors() {
                let candidate = ancestor.join("plans").join("example.json");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        std::path::PathBuf::from("plans/example.json")
    }

    /// Load a human-written plan file and validate it. Read-only: the plan
    /// is inspected in the TUI, never executed from here.
    pub fn load_plan(&mut self) {
        let path = Self::default_plan_path();
        self.plan_source = path.to_string_lossy().to_string();
        self.plan_scroll = 0;
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                self.plan = None;
                self.plan_error = Some(format!("cannot read {}: {e}", self.plan_source));
                self.show_plan = true;
                return;
            }
        };
        let plan = match crate::ops::plan::Plan::from_json(&text) {
            Ok(plan) => plan,
            Err(e) => {
                self.plan = None;
                self.plan_error = Some(e);
                self.show_plan = true;
                return;
            }
        };
        match crate::ops::plan::validate_plan(&self.index, &plan) {
            Ok(()) => {
                self.plan = Some(plan);
                self.plan_error = None;
                self.show_plan = true;
            }
            Err(e) => {
                self.plan = Some(plan);
                self.plan_error = Some(e);
                self.show_plan = true;
            }
        }
    }

    pub fn close_plan(&mut self) {
        self.show_plan = false;
    }

    pub fn default_incident_path() -> std::path::PathBuf {
        if let Ok(p) = std::env::var("AWS_TUI_INCIDENT") {
            return std::path::PathBuf::from(p);
        }
        if let Ok(cwd) = std::env::current_dir() {
            let direct = cwd.join("incidents").join("example.json");
            if direct.is_file() {
                return direct;
            }
            for ancestor in cwd.ancestors() {
                let candidate = ancestor.join("incidents").join("example.json");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        std::path::PathBuf::from("incidents/example.json")
    }

    /// Load a human-written incident file and validate it. Read-only: the
    /// workspace inspects evidence, hypotheses, linked plan, and approvals.
    /// Nothing executes from here.
    pub fn load_incident(&mut self) {
        let path = Self::default_incident_path();
        let source = path.to_string_lossy().to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                self.incident = None;
                self.incident_source = source;
                self.incident_error = Some(format!("cannot read {}: {e}", self.incident_source));
                self.linked_plan_title = None;
                self.linked_plan_steps = 0;
                self.linked_plan_error = None;
                self.show_incident = true;
                return;
            }
        };
        let incident = match crate::ops::incident::Incident::from_json(&text) {
            Ok(incident) => incident,
            Err(e) => {
                self.incident = None;
                self.incident_source = source;
                self.incident_error = Some(e);
                self.linked_plan_title = None;
                self.linked_plan_steps = 0;
                self.linked_plan_error = None;
                self.show_incident = true;
                return;
            }
        };
        if let Err(e) = crate::ops::incident::validate_incident(&incident) {
            self.incident = Some(incident);
            self.incident_source = source;
            self.incident_error = Some(e);
            self.linked_plan_title = None;
            self.linked_plan_steps = 0;
            self.linked_plan_error = None;
            self.show_incident = true;
            return;
        }
        self.incident_error = None;
        self.show_incident_value(incident, source);
    }

    pub fn close_incident(&mut self) {
        self.show_incident = false;
    }

    pub fn confirm_danger(&mut self) {
        let Some(d) = self.danger.as_mut() else {
            return;
        };
        if d.text.trim() != d.expected {
            d.error = Some(format!("type '{}' to confirm", d.expected));
            return;
        }
        self.danger = None;
        self.allow_prod_mutations = true;
        self.execute();
        self.allow_prod_mutations = false;
    }

    pub fn cancel_danger(&mut self) {
        self.danger = None;
        self.status = "cancelled: production mutation not confirmed".to_string();
    }

    pub fn open_history(&mut self) {
        self.history_error = None;
        self.history_idx = 0;
        self.history_scroll = 0;
        self.refresh_history();
        self.show_history = true;
    }

    pub fn close_history(&mut self) {
        self.show_history = false;
    }

    /// Import the shipped example files on first use so a fresh store has
    /// something to coordinate around. Best-effort: files may be absent.
    pub fn seed_op_store(&mut self) {
        let has_incidents = self
            .audit
            .list_incidents()
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if !has_incidents {
            let path = Self::default_incident_path();
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(incident) = crate::ops::incident::Incident::from_json(&text)
            {
                let _ = self.audit.put_incident(&incident);
            }
        }
        let has_plans = self
            .audit
            .list_plans()
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if !has_plans {
            let path = Self::default_plan_path();
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(plan) = crate::ops::plan::Plan::from_json(&text)
            {
                let _ = self.audit.put_plan(&plan);
            }
        }
    }

    pub fn open_inbox(&mut self) {
        self.seed_op_store();
        self.inbox_error = None;
        self.inbox_idx = 0;
        self.inbox_scroll = 0;
        self.refresh_inbox();
        self.show_inbox = true;
    }

    pub fn close_inbox(&mut self) {
        self.show_inbox = false;
    }

    fn refresh_inbox(&mut self) {
        match self.audit.list_incidents() {
            Ok(items) => {
                self.inbox_items = items;
                if !self.inbox_items.is_empty() && self.inbox_idx >= self.inbox_items.len() {
                    self.inbox_idx = self.inbox_items.len() - 1;
                }
            }
            Err(e) => {
                self.inbox_error = Some(e);
            }
        }
    }

    /// Load the selected inbox incident into the read-only workspace.
    pub fn inbox_open_selected(&mut self) {
        let Some(item) = self.inbox_items.get(self.inbox_idx).cloned() else {
            return;
        };
        match self.audit.get_incident(&item.id) {
            Ok(Some(incident)) => {
                self.show_incident_value(incident, format!("store:{0}", item.id));
                self.show_inbox = false;
            }
            Ok(None) => {
                self.inbox_error = Some(format!("incident '{}' no longer stored", item.id));
                self.refresh_inbox();
            }
            Err(e) => {
                self.inbox_error = Some(e);
            }
        }
    }

    /// Advance the selected inbox incident one step along the default chain
    /// (open → investigating → planned → approved → executing → verifying →
    /// closed). Validation errors stay visible in the inbox.
    pub fn inbox_advance_selected(&mut self) {
        let Some(item) = self.inbox_items.get(self.inbox_idx).cloned() else {
            return;
        };
        let next = match item.status.as_str() {
            "open" => "investigating",
            "investigating" => "planned",
            "planned" => "approved",
            "approved" => "executing",
            "executing" => "verifying",
            "verifying" => "closed",
            _ => {
                self.inbox_error = Some(format!("nothing to advance from '{}'", item.status));
                return;
            }
        };
        match self.audit.transition_incident(&item.id, next) {
            Ok(_) => {
                self.inbox_error = None;
                self.refresh_inbox();
            }
            Err(e) => {
                self.inbox_error = Some(e);
            }
        }
    }

    fn show_incident_value(&mut self, incident: crate::ops::incident::Incident, source: String) {
        self.incident_source = source;
        self.incident_scroll = 0;
        self.linked_plan_title = None;
        self.linked_plan_steps = 0;
        self.linked_plan_error = None;
        if let Some(plan_ref) = incident.plan_path.clone() {
            // Stored plans (keyed by title) win when the reference names
            // one; otherwise fall back to the file tree.
            let mut resolved = false;
            if let Ok(Some(plan)) = self.audit.get_plan(&plan_ref) {
                match crate::ops::plan::validate_plan(&self.index, &plan) {
                    Ok(()) => {
                        self.linked_plan_title = Some(plan.title.clone());
                        self.linked_plan_steps = plan.steps.len();
                    }
                    Err(e) => {
                        self.linked_plan_error = Some(e);
                    }
                }
                resolved = true;
            }
            if !resolved {
                let plan_path = std::path::PathBuf::from(&plan_ref);
                match std::fs::read_to_string(&plan_path) {
                    Ok(plan_text) => match crate::ops::plan::Plan::from_json(&plan_text) {
                        Ok(plan) => match crate::ops::plan::validate_plan(&self.index, &plan) {
                            Ok(()) => {
                                self.linked_plan_title = Some(plan.title.clone());
                                self.linked_plan_steps = plan.steps.len();
                            }
                            Err(e) => {
                                self.linked_plan_error = Some(e);
                            }
                        },
                        Err(e) => {
                            self.linked_plan_error = Some(e);
                        }
                    },
                    Err(e) => {
                        self.linked_plan_error = Some(format!("cannot read {plan_ref}: {e}"));
                    }
                }
            }
        }
        self.incident = Some(incident);
        self.show_incident = true;
    }

    fn refresh_history(&mut self) {
        let records = self.audit.history_recent(100);
        let mut items: Vec<HistoryItem> = records
            .iter()
            .map(|(seq, json)| summarize_history_record(*seq, json))
            .collect();
        items.reverse();
        if !items.is_empty() && self.history_idx >= items.len() {
            self.history_idx = items.len() - 1;
        }
        self.history_items = items;
    }

    /// Re-run the selected history entry after re-validating it.
    /// Never executes blindly: unknown operations or params are rejected.
    pub fn history_rerun(&mut self) {
        let Some(item) = self.history_items.get(self.history_idx).cloned() else {
            return;
        };
        let Some(json) = self.audit.history_get(item.seq) else {
            self.history_error = Some(format!("history entry {} no longer stored", item.seq));
            return;
        };
        let (intent, _) = match intent_from_history_json(&json) {
            Ok(v) => v,
            Err(e) => {
                self.history_error = Some(e);
                return;
            }
        };
        if self
            .index
            .get_operation(&intent.service, &intent.operation)
            .is_none()
        {
            self.history_error = Some(format!(
                "unknown operation {}.{} (models changed?)",
                intent.service, intent.operation
            ));
            return;
        }
        if let Err(e) = validate_intent(&self.index, &intent) {
            self.history_error = Some(e);
            return;
        }
        // Point the preview at the rerun target so details stay in sync,
        // then restore the stored params/query/output over the fresh preview.
        if let Some(row) = self
            .index
            .by_service_op
            .get(&(intent.service.clone(), intent.operation.clone()))
            .copied()
        {
            self.selected = Some(row);
        }
        self.build_preview();
        self.intent = Some(intent);
        self.last_source = "history-rerun".to_string();
        self.show_history = false;
        self.execute();
    }

    pub fn follow_up(&mut self, text: &str) {
        let Some(last) = self.last_json.clone() else {
            self.output = "no previous result to query".to_string();
            self.output_scroll = 0;
            return;
        };
        let raw = text.trim();
        if raw.is_empty() {
            self.output =
                "empty query: type `> <JMESPath>` (e.g. `> Subnets[].SubnetId`)".to_string();
            self.output_scroll = 0;
            return;
        }
        // Bare hints keep working (`> region`, `> tag`, `> id`); anything
        // shaped like a JMESPath expression is applied directly.
        let is_bare_hint = !raw.chars().any(|c| "[]().|*?'\"".contains(c));
        if is_bare_hint {
            let hint = raw.to_lowercase();
            let mapped = if hint.contains("region") {
                Some("Subnets[].AvailabilityZone")
            } else if hint.contains("tag") {
                Some("Subnets[].Tags[]")
            } else if hint.contains("id") {
                Some("Subnets[].SubnetId")
            } else {
                None
            };
            if let Some(q) = mapped {
                return self.apply_follow_up_query(&last, q);
            }
        }
        if let Err(e) = validate_query(raw) {
            self.output = format!("{e} (hint: `> region`, `> tag`, `> id`)");
            self.output_scroll = 0;
            return;
        }
        self.apply_follow_up_query(&last, raw);
    }

    fn apply_follow_up_query(&mut self, last: &serde_json::Value, query: &str) {
        match apply_query(last, query) {
            Ok(v) => {
                self.output = pretty_json(&v);
                self.last_json = Some(v);
                self.output_scroll = 0;
                self.table_selected = None;
                self.drill_stack.clear();
                self.auto_drill();
                self.view = OutputView::Blocks;
            }
            Err(e) => {
                self.output = format!("query failed: {e}");
                self.output_scroll = 0;
            }
        }
    }

    pub fn current_rows(&self) -> Option<Vec<ViewRow>> {
        let json = self.last_json.as_ref()?;
        Some(view_rows(json))
    }

    /// Drill one level into the selected block. If the block is a collection
    /// (array of objects / array), descend into it (tree, level by level). If
    /// the block is a parent object or scalar, show its full JSON instead.
    pub fn drill_into(&mut self) {
        let Some(rows) = self.current_rows() else {
            return;
        };
        let Some(idx) = self.table_selected else {
            return;
        };
        let Some(row) = rows.get(idx) else {
            return;
        };
        let Some(next) = row.drill.clone() else {
            return;
        };
        if let Some(parent) = self.last_json.take() {
            self.drill_stack.push(DrillLevel {
                label: row.label.clone(),
                json: parent,
                preview: crate::tui::jsonview::block_header(&next),
            });
        }
        self.last_json = Some(next);
        self.table_selected = Some(0);
        self.output_scroll = 0;
        match self.last_json.as_ref().unwrap() {
            // Collections continue the block tree; show the next level of blocks.
            serde_json::Value::Array(_) => {
                self.auto_drill();
                self.view = OutputView::Blocks;
            }
            // A parent object/scalar: show its JSON contents.
            _ => {
                self.view = OutputView::Json;
            }
        }
        self.output = pretty_json(self.last_json.as_ref().unwrap());
    }

    pub fn drill_up(&mut self) {
        if let Some(level) = self.drill_stack.pop() {
            self.last_json = Some(level.json);
            self.table_selected = Some(0);
            self.output_scroll = 0;
            self.view = OutputView::Blocks;
            self.output = pretty_json(self.last_json.as_ref().unwrap());
        }
    }

    /// Auto-descend through single-key wrappers and thin wrapper records so a
    /// result like `{"Reservations":[{...}]}` lands on the meaningful set (the
    /// Instances array), while `{"Vpcs":[{...}]}` stops at the Vpcs set. Each
    /// wrapper is pushed onto the drill stack.
    fn auto_drill(&mut self) {
        loop {
            let cur = match self.last_json.clone() {
                Some(v) => v,
                None => return,
            };
            let (label, next) = match &cur {
                serde_json::Value::Object(map) if map.len() == 1 => {
                    let (k, v) = map.iter().next().unwrap();
                    match (k.clone(), v.clone()) {
                        (k, v) if v.is_object() || v.is_array() => (k, Some(v)),
                        _ => (String::new(), None),
                    }
                }
                serde_json::Value::Object(map) => {
                    // A thin wrapper record whose only collection is e.g.
                    // Instances → drill into it (Reservation = 2 scalar ids +
                    // Instances; a VPC has many scalar fields so it is kept).
                    if Self::scalar_count(map) <= 2 {
                        let mut collections = map.iter().filter(|(_, v)| {
                            matches!(
                                v,
                                serde_json::Value::Array(a) if !a.is_empty() && a.iter().all(|i| i.is_object())
                            )
                        });
                        match (collections.next(), collections.next()) {
                            (Some((k, v)), None) => (k.clone(), Some(v.clone())),
                            _ => (String::new(), None),
                        }
                    } else {
                        (String::new(), None)
                    }
                }
                // Arrays of thin wrapper records that share one collection key,
                // e.g. Reservations[].Instances → merge into the meaningful set.
                serde_json::Value::Array(items)
                    if !items.is_empty() && items.iter().all(|i| i.is_object()) =>
                {
                    let mut common: Option<String> = None;
                    let mut ok = true;
                    let mut merged: Vec<serde_json::Value> = Vec::new();
                    for item in items {
                        let obj = item.as_object().unwrap();
                        if Self::scalar_count(obj) > 2 {
                            ok = false;
                            break;
                        }
                        let keys: Vec<String> = obj
                            .iter()
                            .filter(|(_, v)| {
                                matches!(
                                    v,
                                    serde_json::Value::Array(a) if !a.is_empty() && a.iter().all(|i| i.is_object())
                                )
                            })
                            .map(|(k, _)| k.clone())
                            .collect();
                        if keys.len() != 1 {
                            ok = false;
                            break;
                        }
                        if let Some(c) = &common {
                            if c != &keys[0] {
                                ok = false;
                                break;
                            }
                        } else {
                            common = Some(keys[0].clone());
                        }
                        if let serde_json::Value::Array(a) = obj.get(&keys[0]).unwrap() {
                            merged.extend(a.iter().cloned());
                        }
                    }
                    match (ok, common) {
                        (true, Some(k)) => (k, Some(serde_json::Value::Array(merged))),
                        _ => (String::new(), None),
                    }
                }
                _ => (String::new(), None),
            };
            match next {
                Some(v) => {
                    let preview = crate::tui::jsonview::block_header(&v);
                    self.drill_stack.push(DrillLevel {
                        label,
                        json: cur,
                        preview,
                    });
                    self.last_json = Some(v);
                }
                None => break,
            }
        }
    }

    /// Count scalar (non-object, non-array) fields in a record.
    fn scalar_count(map: &serde_json::Map<String, serde_json::Value>) -> usize {
        map.values()
            .filter(|v| !v.is_object() && !v.is_array())
            .count()
    }

    pub fn open_picker(&mut self, kind: Picker) {
        self.picker_idx = match kind {
            Picker::Region => self
                .env
                .regions
                .iter()
                .position(|r| *r == self.env.active_region)
                .unwrap_or(0),
            Picker::Profile => self
                .env
                .profiles
                .iter()
                .position(|p| *p == self.env.active_profile)
                .unwrap_or(0),
        };
        self.picker = Some(kind);
    }

    pub fn picker_items(&self) -> Option<&Vec<String>> {
        match self.picker? {
            Picker::Region => Some(&self.env.regions),
            Picker::Profile => Some(&self.env.profiles),
        }
    }

    pub fn select_picker(&mut self) {
        match self.picker {
            Some(Picker::Region) => {
                if let Some(r) = self.env.regions.get(self.picker_idx) {
                    self.env.active_region = r.clone();
                    self.env.region_source = "selected".to_string();
                    self.build_preview();
                }
            }
            Some(Picker::Profile) => {
                if let Some(p) = self.env.profiles.get(self.picker_idx) {
                    self.env.active_profile = p.clone();
                    self.build_preview();
                }
            }
            None => return,
        }
        self.picker = None;
    }

    pub fn picker_len(&self) -> usize {
        self.picker_items().map(|v| v.len()).unwrap_or(0)
    }

    pub fn output_line_count(&self) -> u16 {
        self.output.lines().count() as u16
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            return;
        }
        let idx = self
            .selected
            .and_then(|s| self.results.iter().position(|(i, _)| *i == s))
            .unwrap_or(0);
        let n = self.results.len() as isize;
        let new = (((idx as isize + delta) % n) + n) % n;
        self.selected = Some(self.results[new as usize].0);
        self.build_preview();
    }
}

/// Validate one popup field value against its model shape.
/// Empty values are the caller's concern (required vs skippable optional).
fn validate_param_value(field: &ParamField) -> Result<(), String> {
    let value = field.value.trim();
    if value.is_empty() {
        return Ok(());
    }
    if !field.enum_values.is_empty() {
        if field.is_list {
            for part in value.split([' ', ',']).filter(|s| !s.is_empty()) {
                if !field.enum_values.iter().any(|e| e == part) {
                    return Err(format!(
                        "--{}: '{part}' not in [{}]",
                        field.cli_name,
                        field.enum_values.join(", ")
                    ));
                }
            }
            return Ok(());
        }
        if !field.enum_values.iter().any(|e| e == value) {
            return Err(format!(
                "--{}: '{value}' not in [{}]",
                field.cli_name,
                field.enum_values.join(", ")
            ));
        }
        return Ok(());
    }
    match field.type_name.as_str() {
        "integer" | "long" => {
            if value.parse::<i64>().is_err() {
                return Err(format!("--{}: '{value}' is not an integer", field.cli_name));
            }
        }
        "float" | "double" => {
            if value.parse::<f64>().is_err() {
                return Err(format!("--{}: '{value}' is not a number", field.cli_name));
            }
        }
        "boolean" if !["true", "false", "True", "False", "TRUE", "FALSE"].contains(&value) => {
            return Err(format!(
                "--{}: '{value}' is not a boolean (true/false)",
                field.cli_name
            ));
        }
        _ => {}
    }
    if field.is_structure && (value.starts_with('{') || value.starts_with('[')) {
        serde_json::from_str::<serde_json::Value>(value)
            .map(|_| ())
            .map_err(|_| format!("--{}: invalid JSON", field.cli_name))?;
    }
    Ok(())
}

/// Collect popup values without side effects so the happy path is unit
/// testable without executing AWS.
/// Returns (params, query, output, max_items, page_size).
#[allow(clippy::type_complexity)]
fn collect_popup_values(
    popup: &ParamPopup,
) -> Result<
    (
        Vec<(String, String)>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    String,
> {
    if let Some(field) = popup
        .fields
        .iter()
        .find(|f| f.required && f.value.trim().is_empty())
    {
        return Err(format!("--{} is required", field.cli_name));
    }
    for field in popup.fields.iter().filter(|f| !f.value.trim().is_empty()) {
        validate_param_value(field)?;
    }
    if !popup.query.trim().is_empty() {
        validate_query(popup.query.trim())?;
    }
    if !popup.output.trim().is_empty() && !ALLOWED_OUTPUTS.contains(&popup.output.trim()) {
        return Err(format!(
            "unsupported --output '{}' (expected one of {})",
            popup.output.trim(),
            ALLOWED_OUTPUTS.join(", ")
        ));
    }
    for (label, raw) in [
        ("--max-items", &popup.max_items),
        ("--page-size", &popup.page_size),
    ] {
        if !raw.trim().is_empty() && raw.trim().parse::<u64>().ok().filter(|n| *n > 0).is_none() {
            return Err(format!("{label} must be a positive integer"));
        }
    }
    let params: Vec<(String, String)> = popup
        .fields
        .iter()
        .filter(|f| !f.value.trim().is_empty())
        .map(|f| (f.cli_name.clone(), f.value.trim().to_string()))
        .collect();
    let query = if popup.query.trim().is_empty() {
        None
    } else {
        Some(popup.query.trim().to_string())
    };
    let output = if popup.output.trim().is_empty() {
        None
    } else {
        Some(popup.output.trim().to_string())
    };
    let max_items = if popup.max_items.trim().is_empty() {
        None
    } else {
        Some(popup.max_items.trim().to_string())
    };
    let page_size = if popup.page_size.trim().is_empty() {
        None
    } else {
        Some(popup.page_size.trim().to_string())
    };
    Ok((params, query, output, max_items, page_size))
}

/// Summarize one stored audit record for the history list. Never fails:
/// unparseable records degrade to a placeholder row.
fn summarize_history_record(seq: u64, json: &str) -> HistoryItem {
    let mut item = HistoryItem {
        seq,
        ..Default::default()
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        item.service = "?".to_string();
        return item;
    };
    item.service = v
        .get("intent")
        .and_then(|i| i.get("service"))
        .and_then(|s| s.as_str())
        .unwrap_or("?")
        .to_string();
    item.operation = v
        .get("intent")
        .and_then(|i| i.get("operation"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    item.exit = v
        .get("result")
        .and_then(|r| r.get("exit_code"))
        .and_then(|c| c.as_i64())
        .map(|c| c as i32);
    item.ts = v
        .get("ts")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    item
}

/// Rebuild an executable intent plus its stored provenance from one audit
/// record. The caller must still re-validate before running.
fn intent_from_history_json(json: &str) -> Result<(Intent, Provenance), String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unreadable history entry: {e}"))?;
    let intent_json = v.get("intent").ok_or("history entry has no intent")?;
    let get_str = |obj: &serde_json::Value, key: &str| -> String {
        obj.get(key)
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string()
    };
    let get_opt = |obj: &serde_json::Value, key: &str| -> Option<String> {
        obj.get(key).and_then(|s| s.as_str()).map(|s| s.to_string())
    };
    let params: Vec<(String, String)> = intent_json
        .get("params")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|pair| {
                    let arr = pair.as_array()?;
                    let name = arr.first()?.as_str()?.to_string();
                    let value = arr.get(1)?.as_str()?.to_string();
                    Some((name, value))
                })
                .collect()
        })
        .unwrap_or_default();
    let intent = Intent {
        service: get_str(intent_json, "service"),
        operation: get_str(intent_json, "operation"),
        params,
        query: get_opt(intent_json, "query"),
        output: get_opt(intent_json, "output"),
        region: None,
        profile: None,
        max_items: get_opt(intent_json, "max_items"),
        page_size: get_opt(intent_json, "page_size"),
    };
    // Region/profile are not part of the intent snapshot; recover them from
    // the recorded argv (`... --region X --profile Y ...`), along with
    // pagination flags for records written before they were snapshotted.
    let mut intent = intent;
    if (intent.region.is_none() || intent.profile.is_none())
        && let Some(argv) = v.get("argv").and_then(|a| a.as_array())
    {
        let args: Vec<String> = argv
            .iter()
            .filter_map(|a| a.as_str().map(|s| s.to_string()))
            .collect();
        for w in args.windows(2) {
            if w[0] == "--region" && intent.region.is_none() {
                intent.region = Some(w[1].clone());
            }
            if w[0] == "--profile" && intent.profile.is_none() {
                intent.profile = Some(w[1].clone());
            }
            if w[0] == "--max-items" && intent.max_items.is_none() {
                intent.max_items = Some(w[1].clone());
            }
            if w[0] == "--page-size" && intent.page_size.is_none() {
                intent.page_size = Some(w[1].clone());
            }
        }
    }
    let prov = v.get("provenance");
    let provenance = Provenance {
        cli_version: prov
            .and_then(|p| p.get("cli_version"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        model_path: prov
            .and_then(|p| p.get("model_path"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        model_sha256: prov
            .and_then(|p| p.get("model_sha256"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        api_version: prov
            .and_then(|p| p.get("api_version"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
    };
    if intent.service.trim().is_empty() || intent.operation.trim().is_empty() {
        return Err("history entry has no service/operation".to_string());
    }
    Ok((intent, provenance))
}

pub fn run_tui(
    terminal: &mut ratatui::Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    let tick = Duration::from_millis(100);
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        app.poll_pending();
        if event::poll(tick)? {
            match event::read()? {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                }) => break,
                Event::Key(KeyEvent {
                    code, modifiers, ..
                }) => {
                    if app.param_popup.is_some() {
                        handle_param_popup_key(app, code);
                    } else if app.danger.is_some() {
                        handle_danger_key(app, code);
                    } else if app.picker.is_some() {
                        handle_picker_key(app, code);
                    } else if handle_key(app, code, modifiers) {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle_picker_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Up => {
            if app.picker_len() > 0 {
                app.picker_idx = (app.picker_idx + app.picker_len() - 1) % app.picker_len();
            }
        }
        KeyCode::Down => {
            if app.picker_len() > 0 {
                app.picker_idx = (app.picker_idx + 1) % app.picker_len();
            }
        }
        KeyCode::Enter => app.select_picker(),
        KeyCode::Esc => app.picker = None,
        _ => {}
    }
}

fn handle_param_popup_key(app: &mut App, code: KeyCode) {
    let total = app
        .param_popup
        .as_ref()
        .map(|p| p.total_rows())
        .unwrap_or(0);
    match code {
        KeyCode::Up => {
            if total > 0
                && let Some(p) = app.param_popup.as_mut()
            {
                p.active = (p.active + total - 1) % total;
                p.error = None;
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            if total > 0
                && let Some(p) = app.param_popup.as_mut()
            {
                p.active = (p.active + 1) % total;
                p.error = None;
            }
        }
        KeyCode::Enter => app.confirm_params(),
        KeyCode::Esc => app.param_popup = None,
        KeyCode::Backspace => {
            if let Some(p) = app.param_popup.as_mut() {
                if p.active < p.fields.len() {
                    if let Some(field) = p.fields.get_mut(p.active) {
                        field.value.pop();
                    }
                } else if p.active == p.fields.len() {
                    p.query.pop();
                } else if p.active == p.fields.len() + 1 {
                    p.output.pop();
                } else if p.active == p.fields.len() + 2 {
                    p.max_items.pop();
                } else {
                    p.page_size.pop();
                }
                p.error = None;
            }
        }
        KeyCode::Char(c) => {
            if let Some(p) = app.param_popup.as_mut() {
                if p.active < p.fields.len() {
                    if let Some(field) = p.fields.get_mut(p.active) {
                        field.value.push(c);
                    }
                } else if p.active == p.fields.len() {
                    p.query.push(c);
                } else if p.active == p.fields.len() + 1 {
                    p.output.push(c);
                } else if p.active == p.fields.len() + 2 {
                    p.max_items.push(c);
                } else {
                    p.page_size.push(c);
                }
                p.error = None;
            }
        }
        _ => {}
    }
}

fn handle_danger_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => app.confirm_danger(),
        KeyCode::Esc => app.cancel_danger(),
        KeyCode::Backspace => {
            if let Some(d) = app.danger.as_mut() {
                d.text.pop();
                d.error = None;
            }
        }
        KeyCode::Char(c) => {
            if let Some(d) = app.danger.as_mut() {
                d.text.push(c);
                d.error = None;
            }
        }
        _ => {}
    }
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    match code {
        KeyCode::Char('q') if modifiers.contains(KeyModifiers::CONTROL) => return true,
        KeyCode::Char('q') if app.focus == Focus::Search && app.query.is_empty() => return true,
        KeyCode::F(2) => app.open_picker(Picker::Region),
        KeyCode::F(3) => app.open_picker(Picker::Profile),
        KeyCode::Char('e')
            if !modifiers.contains(KeyModifiers::CONTROL) && app.focus == Focus::Results =>
        {
            app.open_param_editor();
        }
        KeyCode::Char('p')
            if !modifiers.contains(KeyModifiers::CONTROL) && app.focus == Focus::Results =>
        {
            app.load_plan();
        }
        KeyCode::Char('i')
            if !modifiers.contains(KeyModifiers::CONTROL) && app.focus == Focus::Results =>
        {
            app.load_incident();
        }
        KeyCode::Char('t')
            if !modifiers.contains(KeyModifiers::CONTROL) && app.focus == Focus::Results =>
        {
            app.open_inbox();
        }
        KeyCode::Char('a')
            if !modifiers.contains(KeyModifiers::CONTROL)
                && app.focus != Focus::Search
                && app.show_inbox =>
        {
            app.inbox_advance_selected();
        }
        KeyCode::Char('h')
            if !modifiers.contains(KeyModifiers::CONTROL) && app.focus == Focus::Results =>
        {
            app.open_history();
        }
        KeyCode::Char('s')
            if !modifiers.contains(KeyModifiers::CONTROL) && app.focus == Focus::Output =>
        {
            app.save_output();
        }
        KeyCode::Char('?') if !modifiers.contains(KeyModifiers::CONTROL) => {
            if app.focus != Focus::Search {
                app.show_help = true;
                app.help_scroll = 0;
            }
        }
        KeyCode::Char('x')
            if !modifiers.contains(KeyModifiers::CONTROL) && app.focus != Focus::Search =>
        {
            if app.is_running() {
                app.cancel_pending();
            }
        }
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Search => Focus::Results,
                Focus::Results => Focus::Output,
                Focus::Output => Focus::Search,
            };
        }
        KeyCode::Enter => {
            if app.show_inbox {
                app.inbox_open_selected();
            } else if app.show_history {
                app.history_rerun();
            } else if app.focus == Focus::Search && app.query.starts_with('>') {
                let text = app.query.trim_start_matches('>').trim().to_string();
                app.follow_up(&text);
            } else if app.focus == Focus::Output {
                match app.view {
                    OutputView::Blocks => app.drill_into(),
                    OutputView::Json => app.drill_up(),
                }
            } else {
                app.open_param_popup();
            }
        }
        KeyCode::Backspace => {
            if app.focus == Focus::Search {
                app.query.pop();
                app.refresh();
                app.build_preview();
            } else if app.focus == Focus::Output && !app.drill_stack.is_empty() {
                app.drill_up();
            }
        }
        KeyCode::Esc => {
            if app.picker.is_some() {
                app.picker = None;
            } else if app.show_inbox {
                app.close_inbox();
            } else if app.show_help {
                app.show_help = false;
            } else if app.show_history {
                app.close_history();
            } else if app.show_incident {
                app.close_incident();
            } else if app.show_plan {
                app.close_plan();
            } else if !app.drill_stack.is_empty() {
                app.drill_up();
            } else if app.focus != Focus::Search {
                app.focus = Focus::Search;
            } else if !app.query.is_empty() {
                app.query.clear();
                app.refresh();
                app.build_preview();
            }
        }
        KeyCode::Char('v') if app.focus == Focus::Output => {
            app.view = match app.view {
                OutputView::Blocks => OutputView::Json,
                OutputView::Json => OutputView::Blocks,
            };
        }
        KeyCode::Up => {
            if app.show_inbox {
                if !app.inbox_items.is_empty() {
                    app.inbox_idx = app
                        .inbox_idx
                        .saturating_sub(1)
                        .min(app.inbox_items.len() - 1);
                    app.inbox_error = None;
                }
            } else if app.show_help {
                app.help_scroll = app.help_scroll.saturating_sub(1);
            } else if app.show_history {
                if !app.history_items.is_empty() {
                    app.history_idx = app
                        .history_idx
                        .saturating_sub(1)
                        .min(app.history_items.len() - 1);
                    app.history_error = None;
                }
            } else if app.show_incident {
                app.incident_scroll = app.incident_scroll.saturating_sub(1);
            } else if app.show_plan {
                app.plan_scroll = app.plan_scroll.saturating_sub(1);
            } else {
                match app.focus {
                    Focus::Output => {
                        if app.view == OutputView::Blocks && app.table_selected.is_some() {
                            let row = app.table_selected.unwrap().saturating_sub(1);
                            app.table_selected = Some(row);
                        } else {
                            app.output_scroll = app.output_scroll.saturating_sub(1);
                        }
                    }
                    _ => app.move_selection(-1),
                }
            }
        }
        KeyCode::Down => {
            if app.show_inbox {
                if !app.inbox_items.is_empty() {
                    app.inbox_idx = (app.inbox_idx + 1).min(app.inbox_items.len() - 1);
                    app.inbox_error = None;
                }
            } else if app.show_help {
                app.help_scroll = app.help_scroll.saturating_add(1);
            } else if app.show_history {
                if !app.history_items.is_empty() {
                    app.history_idx = (app.history_idx + 1).min(app.history_items.len() - 1);
                    app.history_error = None;
                }
            } else if app.show_incident {
                app.incident_scroll = app.incident_scroll.saturating_add(1);
            } else if app.show_plan {
                app.plan_scroll = app.plan_scroll.saturating_add(1);
            } else {
                match app.focus {
                    Focus::Output => {
                        if app.view == OutputView::Blocks && app.table_selected.is_some() {
                            let row = app.table_selected.unwrap().saturating_add(1);
                            app.table_selected = Some(row);
                        } else {
                            app.output_scroll = app.output_scroll.saturating_add(1);
                        }
                    }
                    _ => app.move_selection(1),
                }
            }
        }
        KeyCode::PageUp => {
            if app.show_inbox {
                app.inbox_idx = app.inbox_idx.saturating_sub(10);
                app.inbox_error = None;
            } else if app.show_help {
                app.help_scroll = app.help_scroll.saturating_sub(20);
            } else if app.show_history {
                app.history_idx = app.history_idx.saturating_sub(10);
                app.history_error = None;
            } else if app.show_incident {
                app.incident_scroll = app.incident_scroll.saturating_sub(20);
            } else if app.show_plan {
                app.plan_scroll = app.plan_scroll.saturating_sub(20);
            } else {
                match app.focus {
                    Focus::Output => {
                        if app.view == OutputView::Blocks && app.table_selected.is_some() {
                            let row = app.table_selected.unwrap().saturating_sub(10);
                            app.table_selected = Some(row);
                        } else {
                            app.output_scroll = app.output_scroll.saturating_sub(20);
                        }
                    }
                    _ => app.details_scroll = app.details_scroll.saturating_sub(10),
                }
            }
        }
        KeyCode::PageDown => {
            if app.show_inbox {
                if !app.inbox_items.is_empty() {
                    app.inbox_idx = (app.inbox_idx + 10).min(app.inbox_items.len() - 1);
                    app.inbox_error = None;
                }
            } else if app.show_help {
                app.help_scroll = app.help_scroll.saturating_add(20);
            } else if app.show_history {
                if !app.history_items.is_empty() {
                    app.history_idx = (app.history_idx + 10).min(app.history_items.len() - 1);
                    app.history_error = None;
                }
            } else if app.show_incident {
                app.incident_scroll = app.incident_scroll.saturating_add(20);
            } else if app.show_plan {
                app.plan_scroll = app.plan_scroll.saturating_add(20);
            } else {
                match app.focus {
                    Focus::Output => {
                        if app.view == OutputView::Blocks && app.table_selected.is_some() {
                            let row = app.table_selected.unwrap().saturating_add(10);
                            app.table_selected = Some(row);
                        } else {
                            app.output_scroll = app.output_scroll.saturating_add(20);
                        }
                    }
                    _ => app.details_scroll = app.details_scroll.saturating_add(10),
                }
            }
        }
        KeyCode::Char(c) if app.focus == Focus::Search => {
            app.query.push(c);
            app.refresh();
            app.build_preview();
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        use crate::aws::awsconfig::AwsEnvironment;
        use crate::aws::index::SearchIndex;
        use crate::aws::loader::{InputParam, OpEntry, Provenance, ServiceEntry, ServiceTree};
        use crate::ops::audit::AuditLogger;

        fn param(cli_name: &str, required: bool) -> InputParam {
            InputParam {
                cli_name: cli_name.to_string(),
                api_name: cli_name.to_string(),
                required,
                type_name: "string".to_string(),
                enum_values: Vec::new(),
                is_list: false,
                is_map: false,
                is_structure: false,
                docs: String::new(),
            }
        }

        let tree = ServiceTree {
            services: vec![ServiceEntry {
                cli_name: "ec2".to_string(),
                full_name: "EC2".to_string(),
                service_id: "EC2".to_string(),
                ops: vec![OpEntry {
                    cli_name: "describe-subnets".to_string(),
                    api_name: "DescribeSubnets".to_string(),
                    docs: String::new(),
                    input: vec![param("subnet-ids", true), param("max-results", false)],
                    output_fields: Vec::new(),
                    output_shape_name: None,
                }],
                provenance: Provenance {
                    cli_version: "test".to_string(),
                    model_path: String::new(),
                    model_sha256: String::new(),
                    api_version: String::new(),
                },
            }],
        };
        let index = SearchIndex::build(&tree);
        let dir = std::env::temp_dir().join(format!(
            "aws-tui-app-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let audit = AuditLogger::new(&dir, "aws").unwrap();
        let env = AwsEnvironment {
            profiles: vec!["default".to_string()],
            regions: vec!["us-east-1".to_string()],
            active_profile: "default".to_string(),
            active_region: "us-east-1".to_string(),
            region_source: "test".to_string(),
        };
        let mut app = App::new(tree, index, audit, "aws".to_string(), env);
        let op_idx = app
            .index
            .by_service_op
            .get(&("ec2".to_string(), "describe-subnets".to_string()))
            .copied()
            .unwrap();
        app.selected = Some(op_idx);
        app.build_preview();
        app
    }

    #[test]
    fn editor_includes_optional_query_and_output() {
        let mut app = test_app();
        app.open_param_editor();
        let popup = app.param_popup.as_ref().unwrap();
        assert_eq!(popup.fields.len(), 2);
        assert!(popup.fields.iter().any(|f| f.required));
        assert!(popup.fields.iter().any(|f| !f.required));
        assert_eq!(popup.total_rows(), 6);
        assert_eq!(popup.output, "json");
    }

    #[test]
    fn collect_skips_empty_optional_and_trims() {
        let mut app = test_app();
        app.open_param_editor();
        let popup = app.param_popup.as_mut().unwrap();
        popup.fields[0].value = "  subnet-1  ".to_string();
        popup.fields[1].value = String::new();
        popup.query = "Subnets[].SubnetId".to_string();
        popup.output = "table".to_string();
        popup.max_items = "10".to_string();
        popup.page_size = String::new();
        let (params, query, output, max_items, page_size) = collect_popup_values(popup).unwrap();
        assert_eq!(
            params,
            vec![("subnet-ids".to_string(), "subnet-1".to_string())]
        );
        assert_eq!(query.as_deref(), Some("Subnets[].SubnetId"));
        assert_eq!(output.as_deref(), Some("table"));
        assert_eq!(max_items.as_deref(), Some("10"));
        assert_eq!(page_size, None);
    }

    #[test]
    fn collect_validates_types_and_paging() {
        let mut app = test_app();
        app.open_param_editor();
        // paging must be a positive integer
        let popup = app.param_popup.as_mut().unwrap();
        popup.fields[0].value = "subnet-1".to_string();
        popup.max_items = "0".to_string();
        assert!(collect_popup_values(popup).is_err());
        let popup = app.param_popup.as_mut().unwrap();
        popup.max_items = "25".to_string();
        popup.page_size = "abc".to_string();
        assert!(collect_popup_values(popup).is_err());
    }

    #[test]
    fn confirm_rejects_bad_query() {
        let mut app = test_app();
        app.open_param_editor();
        {
            let popup = app.param_popup.as_mut().unwrap();
            popup.fields[0].value = "subnet-1".to_string();
            popup.query = "Subnets[]..".to_string();
        }
        app.confirm_params();
        assert!(app.param_popup.is_some());
        assert!(
            app.param_popup
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap_or("")
                .contains("invalid JMESPath")
        );
    }

    #[test]
    fn confirm_rejects_bad_output_and_missing_required() {
        let mut app = test_app();
        app.open_param_editor();
        {
            let popup = app.param_popup.as_mut().unwrap();
            popup.fields[0].value = String::new();
        }
        app.confirm_params();
        assert!(app.param_popup.is_some());

        let mut app = test_app();
        app.open_param_editor();
        {
            let popup = app.param_popup.as_mut().unwrap();
            popup.fields[0].value = "subnet-1".to_string();
            popup.output = "bogus".to_string();
        }
        app.confirm_params();
        assert!(app.param_popup.is_some());
        assert!(
            app.param_popup
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap_or("")
                .contains("unsupported --output")
        );
    }

    #[test]
    fn load_plan_reports_missing_file() {
        let mut app = test_app();
        unsafe {
            std::env::set_var("AWS_TUI_PLAN", "/definitely/not/here-plan.json");
        }
        app.load_plan();
        unsafe {
            std::env::remove_var("AWS_TUI_PLAN");
        }
        assert!(app.show_plan);
        assert!(app.plan.is_none());
        assert!(app.plan_error.is_some());
        app.close_plan();
        assert!(!app.show_plan);
    }

    #[test]
    fn load_incident_reports_missing_file() {
        let mut app = test_app();
        unsafe {
            std::env::set_var("AWS_TUI_INCIDENT", "/definitely/not/here-incident.json");
        }
        app.load_incident();
        unsafe {
            std::env::remove_var("AWS_TUI_INCIDENT");
        }
        assert!(app.show_incident);
        assert!(app.incident.is_none());
        assert!(app.incident_error.is_some());
        app.close_incident();
        assert!(!app.show_incident);
    }

    #[test]
    fn history_summarize_parses_record() {
        let json = r#"{"seq":7,"ts":"2026-01-01T00:00:00Z","intent":{"service":"ec2","operation":"describe-subnets"},"result":{"exit_code":0}}"#;
        let item = summarize_history_record(7, json);
        assert_eq!(item.seq, 7);
        assert_eq!(item.service, "ec2");
        assert_eq!(item.operation, "describe-subnets");
        assert_eq!(item.exit, Some(0));
        let bad = summarize_history_record(8, "not json");
        assert_eq!(bad.service, "?");
    }

    #[test]
    fn history_intent_round_trips_argv_region() {
        let json = serde_json::json!({
            "intent": {"service": "ec2", "operation": "describe-subnets",
                       "params": [["subnet-ids", "subnet-1"]],
                       "query": "Subnets[].SubnetId", "output": "json"},
            "argv": ["aws", "ec2", "describe-subnets", "--region", "eu-west-1",
                     "--profile", "dev", "--query", "Subnets[].SubnetId"],
            "provenance": {"cli_version": "t", "model_path": "p",
                           "model_sha256": "s", "api_version": "a"}
        })
        .to_string();
        let (intent, prov) = intent_from_history_json(&json).unwrap();
        assert_eq!(intent.service, "ec2");
        assert_eq!(
            intent.params,
            vec![("subnet-ids".to_string(), "subnet-1".to_string())]
        );
        assert_eq!(intent.region.as_deref(), Some("eu-west-1"));
        assert_eq!(intent.profile.as_deref(), Some("dev"));
        assert_eq!(prov.api_version, "a");
    }

    #[test]
    fn history_intent_keeps_paging_flags() {
        let json = serde_json::json!({
            "intent": {"service": "ec2", "operation": "describe-instances",
                       "params": [], "max_items": "10", "page_size": "5"},
            "argv": ["aws", "ec2", "describe-instances"],
            "provenance": {"cli_version": "t", "model_path": "p",
                           "model_sha256": "s", "api_version": "a"}
        })
        .to_string();
        let (intent, _) = intent_from_history_json(&json).unwrap();
        assert_eq!(intent.max_items.as_deref(), Some("10"));
        assert_eq!(intent.page_size.as_deref(), Some("5"));
    }

    #[test]
    fn follow_up_applies_arbitrary_jmespath() {
        let mut app = test_app();
        app.last_json = Some(serde_json::json!({"Subnets": [{"SubnetId": "s-1"}]}));
        app.follow_up("Subnets[0].SubnetId");
        assert_eq!(app.last_json, Some(serde_json::json!("s-1")));
    }

    #[test]
    fn follow_up_keeps_bare_hints_and_rejects_garbage() {
        let mut app = test_app();
        app.last_json =
            Some(serde_json::json!({"Subnets": [{"SubnetId": "s-1", "AvailabilityZone": "z"}]}));
        app.follow_up("id");
        assert_eq!(app.last_json, Some(serde_json::json!(["s-1"])));
        app.follow_up("??? not a query [[[");
        assert!(app.output.contains("invalid JMESPath"));
    }

    fn typed_field(cli_name: &str, type_name: &str, value: &str) -> ParamField {
        ParamField {
            cli_name: cli_name.to_string(),
            type_label: type_name.to_string(),
            type_name: type_name.to_string(),
            enum_values: Vec::new(),
            is_list: false,
            is_structure: false,
            docs: String::new(),
            value: value.to_string(),
            required: false,
        }
    }

    #[test]
    fn param_values_validate_against_shapes() {
        assert!(validate_param_value(&typed_field("n", "integer", "42")).is_ok());
        assert!(validate_param_value(&typed_field("n", "integer", "4.5")).is_err());
        assert!(validate_param_value(&typed_field("b", "boolean", "false")).is_ok());
        assert!(validate_param_value(&typed_field("b", "boolean", "maybe")).is_err());
        let mut f = typed_field("fmt", "string", "yaml");
        f.enum_values = vec!["json".to_string(), "yaml".to_string()];
        assert!(validate_param_value(&f).is_ok());
        f.value = "bogus".to_string();
        assert!(validate_param_value(&f).is_err());
        let mut s = typed_field("doc", "structure", "{\"a\":1}");
        s.is_structure = true;
        assert!(validate_param_value(&s).is_ok());
        s.value = "{oops".to_string();
        assert!(validate_param_value(&s).is_err());
    }

    #[test]
    fn read_only_blocks_mutations_only() {
        assert!(App::is_mutation_op("run-instances"));
        assert!(!App::is_mutation_op("describe-instances"));
        assert!(!App::is_mutation_op("list-buckets"));
        assert!(!App::is_mutation_op("get-caller-identity"));
        let mut app = test_app();
        app.read_only = true;
        app.selected = app
            .index
            .by_service_op
            .get(&("ec2".to_string(), "describe-subnets".to_string()))
            .copied();
        app.build_preview();
        app.intent.as_mut().unwrap().operation = "run-instances".to_string();
        app.execute();
        assert!(app.pending.is_none());
        assert!(app.status.contains("read-only"));
    }

    #[test]
    fn prod_mutation_requires_typed_confirmation() {
        let mut app = test_app();
        app.aws_bin = "/definitely/not/here-aws".to_string();
        app.env.active_profile = "prod-admin".to_string();
        app.intent.as_mut().unwrap().operation = "run-instances".to_string();
        app.execute();
        assert!(app.pending.is_none());
        assert_eq!(
            app.danger.as_ref().unwrap().expected,
            "run-instances".to_string()
        );
        app.danger.as_mut().unwrap().text = "wrong".to_string();
        app.confirm_danger();
        assert!(app.danger.is_some());
        app.danger.as_mut().unwrap().text = "run-instances".to_string();
        app.confirm_danger();
        assert!(app.danger.is_none());
        // A worker was spawned with a bogus binary; reap it.
        let mut waited = 0;
        while app.pending.is_some() && waited < 100 {
            app.poll_pending();
            std::thread::sleep(std::time::Duration::from_millis(10));
            waited += 1;
        }
        assert!(app.output.contains("failed to run"));
    }

    fn stored_incident(id: &str, status: &str) -> crate::ops::incident::Incident {
        crate::ops::incident::Incident {
            version: 1,
            id: id.to_string(),
            title: format!("title {id}"),
            status: status.to_string(),
            severity: None,
            account: String::new(),
            region: String::new(),
            signals: Vec::new(),
            hypotheses: Vec::new(),
            plan_path: None,
            approvals: Vec::new(),
        }
    }

    #[test]
    fn inbox_lists_stored_and_advances_status() {
        let mut app = test_app();
        app.audit
            .put_incident(&stored_incident("INC-9", "open"))
            .unwrap();
        app.open_inbox();
        assert!(app.show_inbox);
        assert!(app.inbox_items.iter().any(|i| i.id == "INC-9"));
        app.inbox_idx = app
            .inbox_items
            .iter()
            .position(|i| i.id == "INC-9")
            .unwrap();
        app.inbox_advance_selected();
        assert!(app.inbox_error.is_none());
        assert_eq!(
            app.audit.get_incident("INC-9").unwrap().unwrap().status,
            "investigating"
        );
        app.inbox_open_selected();
        assert!(app.show_incident);
        assert_eq!(app.incident.as_ref().unwrap().id, "INC-9");
        app.close_inbox();
        assert!(!app.show_inbox);
    }
}
