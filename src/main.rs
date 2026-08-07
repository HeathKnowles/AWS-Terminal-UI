use std::path::PathBuf;
use std::time::{Duration, Instant};

use aws_tui::audit::AuditLogger;
use aws_tui::awsconfig::{build_environment, AwsEnvironment};
use aws_tui::builder::{validate_intent, Intent};
use aws_tui::index::SearchIndex;
use aws_tui::loader::{default_data_dir, load_services, ServiceEntry, ServiceTree};
use aws_tui::runner::{apply_query, run_command, validate_query};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState,
};
const CLI_VERSION: &str = "2.36.18";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Search,
    Results,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Picker {
    Region,
    Profile,
}

struct App {
    tree: ServiceTree,
    index: SearchIndex,
    audit: AuditLogger,
    aws_bin: String,
    env: AwsEnvironment,

    query: String,
    results: Vec<(usize, u32)>,
    selected: Option<usize>,

    // Intent preview / output
    intent: Option<Intent>,
    validation_error: Option<String>,
    output: String,
    provenance_line: String,
    last_exit: Option<i32>,
    output_scroll: u16,
    details_scroll: u16,

    // UI state
    focus: Focus,
    picker: Option<Picker>,
    picker_idx: usize,
    status: String,

    last_json: Option<serde_json::Value>,
    last_source: String,
}

impl App {
    fn refresh(&mut self) {
        self.results = self.index.search(&self.query, 200);
        // Align selection with the current results: prefer keeping the same
        // row if it still matches, otherwise jump to the top result.
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
        self.details_scroll = 0;
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

    fn execute(&mut self) {
        let Some(intent) = self.intent.clone() else {
            return;
        };
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

    fn follow_up(&mut self, text: &str) {
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
                    self.output = serde_json::to_string_pretty(&v).unwrap_or_default();
                    self.output_scroll = 0;
                }
                Err(e) => self.output = format!("query failed: {e}"),
            },
            None => {
                self.output = "unrecognized follow-up".to_string();
            }
        }
    }

    fn open_picker(&mut self, kind: Picker) {
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

    fn picker_items(&self) -> Option<&Vec<String>> {
        match self.picker? {
            Picker::Region => Some(&self.env.regions),
            Picker::Profile => Some(&self.env.profiles),
        }
    }

    fn select_picker(&mut self) {
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

    fn picker_len(&self) -> usize {
        self.picker_items().map(|v| v.len()).unwrap_or(0)
    }

    fn output_line_count(&self) -> u16 {
        self.output.lines().count() as u16
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = default_data_dir().ok_or("aws-cli botocore data dir not found; set AWS_TUI_DATA_DIR")?;
    let tree = load_services(&data_dir, CLI_VERSION);
    if tree.services.is_empty() {
        eprintln!("no services loaded from {}", data_dir.display());
        std::process::exit(1);
    }

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
    let env = build_environment();

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(CrosstermBackend::new(stdout))?;

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
        focus: Focus::Search,
        picker: None,
        picker_idx: 0,
        status: String::new(),
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
                Event::Key(KeyEvent { code, modifiers, .. }) => {
                    if app.picker.is_some() {
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
            } else {
                app.execute();
            }
        }
        KeyCode::Backspace => {
            if app.focus == Focus::Search {
                app.query.pop();
                app.refresh();
                app.build_preview();
            }
        }
        KeyCode::Esc => {
            if app.picker.is_some() {
                app.picker = None;
            } else if app.focus != Focus::Search {
                app.focus = Focus::Search;
            } else if !app.query.is_empty() {
                app.query.clear();
                app.refresh();
                app.build_preview();
            }
        }
        KeyCode::Up => match app.focus {
            Focus::Output => {
                app.output_scroll = app.output_scroll.saturating_sub(1);
            }
            _ => move_selection(app, -1),
        },
        KeyCode::Down => match app.focus {
            Focus::Output => {
                app.output_scroll = app.output_scroll.saturating_add(1);
            }
            _ => move_selection(app, 1),
        },
        KeyCode::PageUp => match app.focus {
            Focus::Output => app.output_scroll = app.output_scroll.saturating_sub(20),
            _ => app.details_scroll = app.details_scroll.saturating_sub(10),
        },
        KeyCode::PageDown => match app.focus {
            Focus::Output => app.output_scroll = app.output_scroll.saturating_add(20),
            _ => app.details_scroll = app.details_scroll.saturating_add(10),
        },
        KeyCode::Char(c) => {
            if app.focus == Focus::Search {
                app.query.push(c);
                app.refresh();
                app.build_preview();
            }
        }
        _ => {}
    }
    false
}

fn move_selection(app: &mut App, delta: isize) {
    if app.results.is_empty() {
        return;
    }
    let idx = app
        .selected
        .and_then(|s| app.results.iter().position(|(i, _)| *i == s))
        .unwrap_or(0);
    let n = app.results.len() as isize;
    let new = (((idx as isize + delta) % n) + n) % n;
    app.selected = Some(app.results[new as usize].0);
    app.build_preview();
}

fn draw(f: &mut Frame, app: &mut App) {
    // Layout must fit small terminals (e.g. 24 rows). Order matters:
    // Min panes absorb the leftover space so nothing underflows.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    // Status line
    let focus_name = match app.focus {
        Focus::Search => "search",
        Focus::Results => "results",
        Focus::Output => "output",
    };
    let status = format!(
        " aws-tui  |  region: {} ({})  |  profile: {}  |  focus: {}",
        app.env.active_region, app.env.region_source, app.env.active_profile, focus_name
    );
    f.render_widget(
        Paragraph::new(status)
            .style(Style::default().fg(Color::Black).bg(Color::Cyan))
            .add_modifier(Modifier::BOLD),
        chunks[0],
    );

    // Search input
    let border = if app.focus == Focus::Search {
        BorderType::Double
    } else {
        BorderType::Rounded
    };
    let input = Paragraph::new(app.query.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(border)
                .title(" Search  (F2 region · F3 profile · Enter run · '>' follow-up) "),
        );
    f.render_widget(input, chunks[1]);
    f.set_cursor_position((chunks[1].x + app.query.len() as u16 + 1, chunks[1].y + 1));

    // Middle: results + details
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[2]);

    // Results list
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|(i, _)| {
            let row = &app.index.rows[*i];
            let op = if row.operation.is_empty() {
                format!("[{}]", row.service)
            } else {
                format!("{}.{}", row.service, row.operation)
            };
            ListItem::new(op)
        })
        .collect();
    let list_title = format!(" Results ({})  ", app.results.len());
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(if app.focus == Focus::Results {
                BorderType::Double
            } else {
                BorderType::Rounded
            })
            .title(list_title),
    );
    let mut st = ListState::default();
    if let Some(sel) = app.selected {
        if let Some(pos) = app.results.iter().position(|(i, _)| *i == sel) {
            st = ListState::default().with_selected(Some(pos));
        }
    }
    f.render_stateful_widget(list, mid[0], &mut st);

    // Details pane
    let (cmd_line, docs, params, validation) = match (&app.intent, &app.validation_error) {
        (Some(i), err) => {
            let op = app
                .current_service()
                .and_then(|s| s.ops.iter().find(|o| o.cli_name == i.operation));
            let docs = op.map(|o| o.docs.clone()).unwrap_or_default();
            let params = op.map(|o| o.input.clone()).unwrap_or_default();
            let err = err.clone();
            (i.command_line(&app.aws_bin), docs, params, err)
        }
        _ => (String::new(), String::new(), Vec::new(), None),
    };

    let mut details = String::new();
    details.push_str(&cmd_line);
    details.push('\n');
    match &validation {
        Some(e) => details.push_str(&format!("[validation] {e}\n")),
        None => details.push_str("[validation] OK\n"),
    }
    details.push('\n');
    details.push_str("input params:\n");
    if params.is_empty() {
        details.push_str("  (none)\n");
    } else {
        for p in &params {
            let star = if p.required { "*" } else { " " };
            let ty = if p.is_structure {
                "json".to_string()
            } else {
                p.type_name.clone()
            };
            details.push_str(&format!("  {star} --{:<28} {}\n", p.cli_name, ty));
        }
    }
    if !docs.is_empty() {
        details.push_str("\ndocs:\n");
        details.push_str(&wrap_text(&docs, 72));
        details.push('\n');
    }
    let details_lines = details.lines().count() as u16;
    let details_widget = Paragraph::new(details)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Details  (PageUp/PageDown scroll) "),
        )
        .scroll((app.details_scroll, 0));
    f.render_widget(details_widget, mid[1]);
    if details_lines > mid[1].height.saturating_sub(2) {
        let mut sb = ScrollbarState::new(details_lines as usize).position(app.details_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            mid[1],
            &mut sb,
        );
    }

    // Output pane
    let border = if app.focus == Focus::Output {
        BorderType::Double
    } else {
        BorderType::Rounded
    };
    let title = format!(
        " Output{} ",
        if let Some(rc) = app.last_exit {
            format!("(exit {rc})")
        } else {
            String::new()
        }
    );
    let out = Paragraph::new(app.output.clone())
        .block(Block::default().borders(Borders::ALL).border_type(border).title(title))
        .scroll((app.output_scroll, 0));
    f.render_widget(out, chunks[3]);
    let total = app.output_line_count();
    let viewport = chunks[3].height.saturating_sub(2);
    if total > viewport {
        let max_scroll = total.saturating_sub(viewport);
        if app.output_scroll > max_scroll {
            app.output_scroll = max_scroll;
        }
        let mut sb = ScrollbarState::new(total as usize).position(app.output_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            chunks[3],
            &mut sb,
        );
    }

    // Footer keybindings
    let footer = format!(
        " [F2] region  [F3] profile  [Tab] focus  [Enter] run  [PgUp/PgDn] scroll  [Esc] back  [q/Ctrl-C] quit  |  {}{}",
        app.provenance_line,
        if app.status.is_empty() {
            String::new()
        } else {
            format!("  |  {}", app.status)
        }
    );
    f.render_widget(Paragraph::new(footer), chunks[4]);

    // Picker overlay
    if let Some(kind) = app.picker {
        draw_picker(f, app, kind);
    }
}

fn draw_picker(f: &mut Frame, app: &App, kind: Picker) {
    let title = match kind {
        Picker::Region => " Select region ",
        Picker::Profile => " Select profile ",
    };
    let items = app.picker_items().unwrap();
    let pop_w = 40.min(f.area().width.saturating_sub(4));
    let pop_h = (items.len() as u16 + 2).min(f.area().height.saturating_sub(4));
    let x = f.area().x + f.area().width.saturating_sub(pop_w) / 2;
    let y = f.area().y + f.area().height.saturating_sub(pop_h) / 2;
    let pop = Rect::new(x, y, pop_w, pop_h);

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mark = if i == app.picker_idx { "> " } else { "  " };
            ListItem::new(format!("{mark}{name}"))
        })
        .collect();
    let list = List::new(list_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .title(title)
            .style(Style::default().bg(Color::DarkGray)),
    );
    let mut st = ListState::default().with_selected(Some(app.picker_idx));
    f.render_widget(Clear, pop);
    f.render_stateful_widget(list, pop, &mut st);
}

fn wrap_text(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut line_len = 0usize;
    for word in s.split_whitespace() {
        if line_len + word.len() + 1 > width && line_len > 0 {
            out.push('\n');
            line_len = 0;
        }
        if line_len > 0 {
            out.push(' ');
            line_len += 1;
        }
        out.push_str(word);
        line_len += word.len();
    }
    out
}
