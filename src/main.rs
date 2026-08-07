use std::path::PathBuf;
use std::time::{Duration, Instant};

use aws_tui::audit::AuditLogger;
use aws_tui::builder::{validate_intent, Intent};
use aws_tui::index::SearchIndex;
use aws_tui::loader::{default_data_dir, load_services, ServiceEntry, ServiceTree};
use aws_tui::runner::{apply_query, run_command, validate_query};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
const CLI_VERSION: &str = "2.36.18";

struct App {
    tree: ServiceTree,
    index: SearchIndex,
    audit: AuditLogger,
    aws_bin: String,

    query: String,
    results: Vec<(usize, u32)>,
    selected: Option<usize>,

    // Intent preview / output
    intent: Option<Intent>,
    validation_error: Option<String>,
    output: String,
    provenance_line: String,
    last_exit: Option<i32>,

    last_json: Option<serde_json::Value>,
    last_source: String,
}

impl App {
    fn refresh(&mut self) {
        self.results = self.index.search(&self.query, 50);
        if self.selected.is_none() && !self.results.is_empty() {
            self.selected = Some(self.results[0].0);
        }
    }

    fn selected_row(&self) -> Option<&aws_tui::index::SearchRow> {
        self.selected.and_then(|i| self.index.rows.get(i))
    }

    fn current_service(&self) -> Option<&ServiceEntry> {
        let row = self.selected_row()?;
        self.tree
            .services
            .iter()
            .find(|s| s.cli_name == row.service)
    }

    fn build_preview(&mut self) {
        let Some(row) = self.selected_row() else {
            self.intent = None;
            return;
        };
        let service = self.current_service().map(|s| s.cli_name.clone()).unwrap_or_default();
        let op = row.operation.clone();
        if op.is_empty() {
            self.intent = None;
            self.provenance_line = String::new();
            return;
        }
        let region = current_region();
        let profile = current_profile();
        let intent = Intent {
            service,
            operation: op,
            region: Some(region),
            profile: Some(profile),
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

    fn execute(&mut self) {
        let Some(intent) = self.intent.clone() else {
            return;
        };
        // Only validate the query when one is present; ops like
        // get-caller-identity have no --query and an empty expression is
        // invalid JMESPath.
        if let Some(q) = &intent.query {
            if let Err(e) = validate_query(q) {
                self.validation_error = Some(e);
                return;
            }
        }
        let provenance = self
            .current_service()
            .map(|s| s.provenance.clone())
            .unwrap_or(aws_tui::loader::Provenance {
                cli_version: CLI_VERSION.to_string(),
                model_path: String::new(),
                model_sha256: String::new(),
                api_version: String::new(),
            });
        let argv = intent.argv(&self.aws_bin);
        self.audit.propose(&intent, &provenance, &self.last_source);

        let start = Instant::now();
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
                self.last_exit = Some(out.exit_code);
                self.audit.executed(
                    &intent,
                    &provenance,
                    &self.last_source,
                    out.exit_code,
                    duration,
                    output_sha,
                    if out.stderr.is_empty() {
                        None
                    } else {
                        Some(out.stderr.clone())
                    },
                );
                if !out.stderr.is_empty() {
                    self.output.push_str("\n--- stderr ---\n");
                    self.output.push_str(&out.stderr);
                }
                self.last_source = "history-rerun".to_string();
            }
            Err(e) => {
                self.output = format!("failed to run: {e}");
                self.last_exit = Some(-1);
            }
        }
    }

    /// Resolve "give me the regions of this" against the last result.
    fn follow_up(&mut self, text: &str) {
        let Some(last) = self.last_json.clone() else {
            self.output = "no previous result to query".to_string();
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
                Ok(v) => self.output = serde_json::to_string_pretty(&v).unwrap_or_default(),
                Err(e) => self.output = format!("query failed: {e}"),
            },
            None => {
                self.output = "unrecognized follow-up".to_string();
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = default_data_dir().ok_or("aws-cli botocore data dir not found; set AWS_TUI_DATA_DIR")?;
    let tree = load_services(&data_dir, CLI_VERSION);
    if tree.services.is_empty() {
        eprintln!("no services loaded from {}", data_dir.display());
        std::process::exit(1);
    }

    // Headless mode: dump services/operations and exit (no TTY required).
    if std::env::args().any(|a| a == "--dump") {
        for svc in &tree.services {
            let ops = svc
                .ops
                .iter()
                .map(|o| o.cli_name.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "{} [{}] provenance={} sha={}.. api={} ops=[{}]",
                svc.cli_name,
                svc.full_name,
                svc.provenance.model_path,
                &svc.provenance.model_sha256[..8.min(svc.provenance.model_sha256.len())],
                svc.provenance.api_version,
                ops
            );
        }
        return Ok(());
    }

    let index = SearchIndex::build(&tree);
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("aws-tui");
    let audit = AuditLogger::new(&base)?;
    let aws_bin = std::env::var("AWS_TUI_AWS_BIN").unwrap_or_else(|_| "aws".to_string());

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App {
        tree,
        index,
        audit,
        aws_bin,
        query: String::new(),
        results: Vec::new(),
        selected: None,
        intent: None,
        validation_error: None,
        output: String::new(),
        provenance_line: String::new(),
        last_exit: None,
        last_json: None,
        last_source: "typed-direct".to_string(),
    };
    app.refresh();
    app.build_preview();

    let res = run_tui(&mut terminal, &mut app);

    disable_raw_mode()?;
    terminal.show_cursor()?;
    terminal.clear()?;
    drop(terminal);
    std::io::stdout().execute(LeaveAlternateScreen)?;
    res
}

fn run_tui(
    terminal: &mut ratatui::Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    let tick = Duration::from_millis(100);
    loop {
        terminal.draw(|f| draw(f, app))?;
        if event::poll(tick)? {
            match event::read()? {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                }) => break,
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }) => {
                    if app.query.starts_with(">") {
                        let text = app.query.trim_start_matches('>').trim().to_string();
                        app.follow_up(&text);
                    } else {
                        app.execute();
                    }
                }
                Event::Key(KeyEvent { code: KeyCode::Backspace, .. }) => {
                    app.query.pop();
                    app.refresh();
                    app.build_preview();
                }
                Event::Key(KeyEvent { code: KeyCode::Esc, .. }) => {
                    if app.query.is_empty() {
                        break;
                    }
                    app.query.clear();
                    app.refresh();
                    app.build_preview();
                }
                Event::Key(KeyEvent { code: KeyCode::Up, .. }) => {
                    if !app.results.is_empty() {
                        let idx = app
                            .selected
                            .and_then(|s| app.results.iter().position(|(i, _)| *i == s))
                            .unwrap_or(0);
                        let new = (idx + app.results.len() - 1) % app.results.len();
                        app.selected = Some(app.results[new].0);
                        app.build_preview();
                    }
                }
                Event::Key(KeyEvent { code: KeyCode::Down, .. }) => {
                    if !app.results.is_empty() {
                        let idx = app
                            .selected
                            .and_then(|s| app.results.iter().position(|(i, _)| *i == s))
                            .unwrap_or(0);
                        let new = (idx + 1) % app.results.len();
                        app.selected = Some(app.results[new].0);
                        app.build_preview();
                    }
                }
                Event::Key(KeyEvent { code: KeyCode::Char(c), .. }) => {
                    app.query.push(c);
                    app.refresh();
                    app.build_preview();
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Search input
    let input = Paragraph::new(app.query.as_str())
        .block(Block::default().borders(Borders::ALL).title("Search (Enter = run, Esc = clear)"));
    f.render_widget(input, chunks[0]);
    f.set_cursor_position((
        chunks[0].x + app.query.len() as u16 + 1,
        chunks[0].y + 1,
    ));

    // Command preview
    let preview = if let Some(i) = &app.intent {
        let cl = i.command_line(&app.aws_bin);
        let mut t = format!("{} > {cl}", if app.validation_error.is_none() { "OK" } else { "ERR" });
        if let Some(e) = &app.validation_error {
            t.push_str(&format!("  [{e}]"));
        }
        t
    } else if !app.query.is_empty() {
        "no operation selected".to_string()
    } else {
        "type to search, arrows to navigate, Enter to run".to_string()
    };
    f.render_widget(
        Paragraph::new(preview).block(Block::default().borders(Borders::ALL).title("Command")),
        chunks[1],
    );

    // Results list
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|(i, _)| {
            let row = &app.index.rows[*i];
            let op = if row.operation.is_empty() {
                format!("{}  [service]", row.service)
            } else {
                format!("{} {}", row.service, row.operation)
            };
            ListItem::new(op)
        })
        .collect();
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("Results"));
    let mut st = ListState::default();
    if let Some(sel) = app.selected {
        if let Some(pos) = app.results.iter().position(|(i, _)| *i == sel) {
            st = ListState::default().with_selected(Some(pos));
        }
    }
    f.render_stateful_widget(list, chunks[2], &mut st);

    // Output
    let out = Paragraph::new(app.output.clone())
        .block(Block::default().borders(Borders::ALL).title("Output"));
    f.render_widget(out, chunks[3]);

    // Provenance footer
    let footer = format!(
        "{}  |  {}", 
        app.provenance_line,
        if let Some(rc) = app.last_exit {
            format!("last exit code: {rc}")
        } else {
            "follow-up: prefix with '>' e.g. '> regions of this'".to_string()
        }
    );
    f.render_widget(Paragraph::new(footer), chunks[4]);
}

fn current_region() -> String {
    std::env::var("AWS_REGION")
        .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|_| "us-east-1".to_string())
}

fn current_profile() -> String {
    std::env::var("AWS_PROFILE").unwrap_or_else(|_| "default".to_string())
}
