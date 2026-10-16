use std::time::{Duration, Instant};

use aws_tui::audit::AuditLogger;
use aws_tui::awsconfig::AwsEnvironment;
use aws_tui::builder::{Intent, validate_intent};
use aws_tui::index::SearchIndex;
use aws_tui::loader::{InputParam, ServiceEntry, ServiceTree};
use aws_tui::runner::{apply_query, run_command, validate_query};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::CrosstermBackend;

use crate::jsonview::{ViewRow, pretty_json, view_rows};
use crate::ui;

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
/// One input field in the required-parameters popup.
pub struct ParamField {
    pub cli_name: String,
    pub type_label: String,
    pub value: String,
}

pub struct ParamPopup {
    pub fields: Vec<ParamField>,
    pub active: usize,
    pub error: Option<String>,
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

    pub fn selected_row(&self) -> Option<&aws_tui::index::SearchRow> {
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
            self.provenance_line = format!(
                "fetched from {} (sha256:{}..) aws-cli {} api {}",
                p.model_path,
                &p.model_sha256[..8],
                p.cli_version,
                p.api_version
            );
        }
    }

    pub fn execute(&mut self) {
        let Some(intent) = self.intent.clone() else {
            return;
        };
        if let Some(q) = &intent.query
            && let Err(e) = validate_query(q)
        {
            self.validation_error = Some(e);
            return;
        }
        let provenance = self
            .current_service()
            .map(|s| s.provenance.clone())
            .unwrap_or_else(|| aws_tui::loader::Provenance {
                cli_version: CLI_VERSION.to_string(),
                model_path: String::new(),
                model_sha256: String::new(),
                api_version: String::new(),
            });
        let argv = intent.argv(&self.aws_bin);
        let reference = self.audit.propose(&intent, &provenance, &self.last_source);

        let start = Instant::now();
        self.status = format!("running: {}", intent.command_line(&self.aws_bin));
        match run_command(&argv, None) {
            Ok(out) => {
                let duration = start.elapsed().as_millis() as u64;
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
                    &intent,
                    &provenance,
                    &self.last_source,
                    aws_tui::audit::ExecutionOutcome {
                        exit_code: out.exit_code,
                        duration_ms: duration,
                        output_sha256: output_sha,
                        error: if out.stderr.is_empty() {
                            None
                        } else {
                            Some(out.stderr.clone())
                        },
                    },
                    Some(reference),
                );
                if !out.stderr.is_empty() {
                    self.output.push_str("\n--- stderr ---\n");
                    self.output.push_str(&out.stderr);
                }
                let status = if out.exit_code == 0 { "ok" } else { "failed" };
                self.status = format!("{status} (exit {}) in {}ms", out.exit_code, duration);
                self.last_source = "history-rerun".to_string();
            }
            Err(e) => {
                self.output = format!("failed to run: {e}");
                self.output_scroll = 0;
                self.last_exit = Some(-1);
                self.status = format!("failed: {e}");
            }
        }
    }

    /// If the current operation declares required input params, open the
    /// required-params popup; otherwise execute directly.
    pub fn open_param_popup(&mut self) {
        let Some(intent) = self.intent.clone() else {
            return;
        };
        let Some(svc) = self.current_service() else {
            return;
        };
        let Some(op) = svc.ops.iter().find(|o| o.cli_name == intent.operation) else {
            return;
        };
        let required: Vec<&InputParam> = op.input.iter().filter(|p| p.required).collect();
        if required.is_empty() {
            self.execute();
            return;
        }
        // Pre-fill from any params already present in the intent (e.g. from a
        // natural-language hint) so reruns don't lose values.
        let existing: Vec<(String, String)> = intent.params.clone();
        let fields: Vec<ParamField> = required
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
                    value,
                }
            })
            .collect();
        self.param_popup = Some(ParamPopup {
            fields,
            active: 0,
            error: None,
        });
    }

    /// Validate the popup fields and, when all required params are filled,
    /// write them back into the intent and run the command.
    pub fn confirm_params(&mut self) {
        let Some(popup) = self.param_popup.as_mut() else {
            return;
        };
        if let Some(field) = popup.fields.iter().find(|f| f.value.trim().is_empty()) {
            popup.error = Some(format!("--{} is required", field.cli_name));
            return;
        }
        let params: Vec<(String, String)> = popup
            .fields
            .iter()
            .map(|f| (f.cli_name.clone(), f.value.trim().to_string()))
            .collect();
        self.param_popup = None;
        if let Some(intent) = self.intent.as_mut() {
            intent.params = params;
        }
        self.execute();
    }

    pub fn follow_up(&mut self, text: &str) {
        let Some(last) = self.last_json.clone() else {
            self.output = "no previous result to query".to_string();
            self.output_scroll = 0;
            return;
        };
        let hint = text.to_lowercase();
        let query = if hint.contains("region") {
            Some("Subnets[].AvailabilityZone")
        } else if hint.contains("tag") {
            Some("Subnets[].Tags[]")
        } else if hint.contains("id") {
            Some("Subnets[].SubnetId")
        } else {
            None
        };
        match query {
            Some(q) => match apply_query(&last, q) {
                Ok(v) => {
                    self.output = pretty_json(&v);
                    self.last_json = Some(v);
                    self.output_scroll = 0;
                    self.table_selected = None;
                    self.drill_stack.clear();
                    self.auto_drill();
                    self.view = OutputView::Blocks;
                }
                Err(e) => self.output = format!("query failed: {e}"),
            },
            None => {
                self.output = "unrecognized follow-up".to_string();
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
                preview: crate::jsonview::block_header(&next),
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
                    let preview = crate::jsonview::block_header(&v);
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

pub fn run_tui(
    terminal: &mut ratatui::Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    let tick = Duration::from_millis(100);
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
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
    let fields_len = app
        .param_popup
        .as_ref()
        .map(|p| p.fields.len())
        .unwrap_or(0);
    match code {
        KeyCode::Up => {
            if fields_len > 0
                && let Some(p) = app.param_popup.as_mut()
            {
                p.active = (p.active + fields_len - 1) % fields_len;
            }
        }
        KeyCode::Down | KeyCode::Tab => {
            if fields_len > 0
                && let Some(p) = app.param_popup.as_mut()
            {
                p.active = (p.active + 1) % fields_len;
            }
        }
        KeyCode::Enter => app.confirm_params(),
        KeyCode::Esc => app.param_popup = None,
        KeyCode::Backspace => {
            if let Some(p) = app.param_popup.as_mut()
                && let Some(field) = p.fields.get_mut(p.active)
            {
                field.value.pop();
                p.error = None;
            }
        }
        KeyCode::Char(c) => {
            if let Some(p) = app.param_popup.as_mut()
                && let Some(field) = p.fields.get_mut(p.active)
            {
                field.value.push(c);
                p.error = None;
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
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Search => Focus::Results,
                Focus::Results => Focus::Output,
                Focus::Output => Focus::Search,
            };
        }
        KeyCode::Enter => {
            if app.focus == Focus::Search && app.query.starts_with('>') {
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
        KeyCode::Up => match app.focus {
            Focus::Output => {
                if app.view == OutputView::Blocks && app.table_selected.is_some() {
                    let row = app.table_selected.unwrap().saturating_sub(1);
                    app.table_selected = Some(row);
                } else {
                    app.output_scroll = app.output_scroll.saturating_sub(1);
                }
            }
            _ => app.move_selection(-1),
        },
        KeyCode::Down => match app.focus {
            Focus::Output => {
                if app.view == OutputView::Blocks && app.table_selected.is_some() {
                    let row = app.table_selected.unwrap().saturating_add(1);
                    app.table_selected = Some(row);
                } else {
                    app.output_scroll = app.output_scroll.saturating_add(1);
                }
            }
            _ => app.move_selection(1),
        },
        KeyCode::PageUp => match app.focus {
            Focus::Output => {
                if app.view == OutputView::Blocks && app.table_selected.is_some() {
                    let row = app.table_selected.unwrap().saturating_sub(10);
                    app.table_selected = Some(row);
                } else {
                    app.output_scroll = app.output_scroll.saturating_sub(20);
                }
            }
            _ => app.details_scroll = app.details_scroll.saturating_sub(10),
        },
        KeyCode::PageDown => match app.focus {
            Focus::Output => {
                if app.view == OutputView::Blocks && app.table_selected.is_some() {
                    let row = app.table_selected.unwrap().saturating_add(10);
                    app.table_selected = Some(row);
                } else {
                    app.output_scroll = app.output_scroll.saturating_add(20);
                }
            }
            _ => app.details_scroll = app.details_scroll.saturating_add(10),
        },
        KeyCode::Char(c) if app.focus == Focus::Search => {
            app.query.push(c);
            app.refresh();
            app.build_preview();
        }
        _ => {}
    }
    false
}
