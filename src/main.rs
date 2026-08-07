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
    Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row,
    Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState,
};
const CLI_VERSION: &str = "2.36.18";

struct Theme {
    accent: Color,
    border: Color,
    focus: Color,
    ok: Color,
    err: Color,
    warn: Color,
}
const THEME: Theme = Theme {
    accent: Color::Cyan,
    border: Color::DarkGray,
    focus: Color::Cyan,
    ok: Color::LightGreen,
    err: Color::LightRed,
    warn: Color::Yellow,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputView {
    Auto,
    Raw,
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
    view: OutputView,

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
                self.view = OutputView::Auto;
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
                    self.last_json = Some(v);
                    self.output_scroll = 0;
                    self.view = OutputView::Auto;
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
        view: OutputView::Auto,
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
        KeyCode::Char('v') if app.focus == Focus::Output => {
            app.view = match app.view {
                OutputView::Auto => OutputView::Raw,
                OutputView::Raw => OutputView::Auto,
            };
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

    draw_status(f, app, chunks[0]);
    draw_search(f, app, chunks[1]);

    // Middle: results + details
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(chunks[2]);
    draw_results(f, app, mid[0]);
    draw_details(f, app, mid[1]);

    draw_output(f, app, chunks[3]);
    draw_footer(f, app, chunks[4]);

    if let Some(kind) = app.picker {
        draw_picker(f, app, kind);
    }
}

fn pane_border(focus: bool) -> BorderType {
    if focus {
        BorderType::Double
    } else {
        BorderType::Rounded
    }
}

fn pane_border_color(focus: bool) -> Color {
    if focus {
        THEME.focus
    } else {
        THEME.border
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let focus_name = match app.focus {
        Focus::Search => "search",
        Focus::Results => "results",
        Focus::Output => "output",
    };
    let status = Line::from(vec![
        Span::styled(" aws-tui ", Style::default().fg(Color::Black).add_modifier(Modifier::BOLD)),
        Span::raw(" │ "),
        Span::styled("region ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.env.active_region.clone(),
            Style::default().fg(Color::Black).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" ({})", app.env.region_source), Style::default().fg(Color::DarkGray)),
        Span::raw("  │  "),
        Span::styled("profile ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.env.active_profile.clone(), Style::default().fg(Color::Black).add_modifier(Modifier::BOLD)),
        Span::raw("  │  "),
        Span::styled("focus ", Style::default().fg(Color::DarkGray)),
        Span::styled(focus_name, Style::default().fg(Color::Black).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(
        Paragraph::new(status)
            .style(Style::default().bg(Color::Cyan))
            .block(Block::default().style(Style::default().bg(Color::Cyan))),
        area,
    );
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Search;
    let input = Paragraph::new(app.query.as_str()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(pane_border(focused))
            .border_style(Style::default().fg(pane_border_color(focused)))
            .title(Span::styled(
                " Search ",
                Style::default().fg(THEME.accent).add_modifier(Modifier::BOLD),
            ))
            .title(Span::styled(
                " F2 region · F3 profile · Enter run · '>' follow-up ",
                Style::default().fg(Color::DarkGray),
            )),
    );
    f.render_widget(input, area);
    f.set_cursor_position((area.x + app.query.len() as u16 + 1, area.y + 1));
}

fn draw_results(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Results;
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|(i, _)| {
            let row = &app.index.rows[*i];
            if row.operation.is_empty() {
                // service row
                ListItem::new(Span::styled(
                    format!("[{}]", row.service),
                    Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
                ))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{:.<20}", row.service), Style::default().fg(Color::DarkGray)),
                    Span::styled(row.operation.clone(), Style::default().fg(Color::White)),
                ]))
            }
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(pane_border(focused))
                .border_style(Style::default().fg(pane_border_color(focused)))
                .title(Span::styled(
                    format!(" Results ({}) ", app.results.len()),
                    Style::default().fg(THEME.accent).add_modifier(Modifier::BOLD),
                )),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.accent)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▍");
    let mut st = ListState::default();
    if let Some(sel) = app.selected {
        if let Some(pos) = app.results.iter().position(|(i, _)| *i == sel) {
            st = ListState::default().with_selected(Some(pos));
        }
    }
    f.render_stateful_widget(list, area, &mut st);
}

fn draw_details(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Results || app.focus == Focus::Search;
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

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("$ ", Style::default().fg(Color::DarkGray)),
        Span::styled(cmd_line, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ]));
    match &validation {
        Some(e) => lines.push(Line::from(vec![
            Span::styled("validation ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("[ERR] {e}"), Style::default().fg(THEME.err).add_modifier(Modifier::BOLD)),
        ])),
        None => lines.push(Line::from(vec![
            Span::styled("validation ", Style::default().fg(Color::DarkGray)),
            Span::styled("[OK]", Style::default().fg(THEME.ok).add_modifier(Modifier::BOLD)),
        ])),
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "input params:",
        Style::default().fg(THEME.accent).add_modifier(Modifier::BOLD),
    )));
    if params.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for p in &params {
            let star = if p.required { "*" } else { " " };
            let ty = if p.is_structure {
                "json".to_string()
            } else {
                p.type_name.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(star, Style::default().fg(if p.required { THEME.warn } else { Color::DarkGray })),
                Span::styled(
                    format!("--{}", p.cli_name),
                    Style::default().fg(if p.required { Color::White } else { Color::DarkGray }),
                ),
                Span::styled(format!("  {ty}"), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }
    if !docs.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "docs:",
            Style::default().fg(THEME.accent).add_modifier(Modifier::BOLD),
        )));
        for wrapped in wrap_text(&docs, 72).lines() {
            lines.push(Line::from(Span::styled(wrapped.to_string(), Style::default().fg(Color::Gray))));
        }
    }
    let details_lines = lines.len() as u16;
    let details_widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(pane_border(focused))
            .border_style(Style::default().fg(pane_border_color(focused)))
            .title(Span::styled(
                " Details ",
                Style::default().fg(THEME.accent).add_modifier(Modifier::BOLD),
            )),
    );
    f.render_widget(details_widget, area);
    if details_lines > area.height.saturating_sub(2) {
        let max_scroll = details_lines.saturating_sub(area.height.saturating_sub(2));
        if app.details_scroll > max_scroll {
            app.details_scroll = max_scroll;
        }
        let mut sb = ScrollbarState::new(details_lines as usize).position(app.details_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            area,
            &mut sb,
        );
    }
}

fn draw_output(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Output;
    let view_label = if app.view == OutputView::Raw { "json" } else { "table" };
    let title = format!(
        " Output{} · {view_label} ",
        if let Some(rc) = app.last_exit {
            format!("(exit {rc})")
        } else {
            String::new()
        }
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(pane_border(focused))
        .border_style(Style::default().fg(pane_border_color(focused)))
        .title(Span::styled(
            title,
            Style::default().fg(THEME.accent).add_modifier(Modifier::BOLD),
        ));

    // Try structured table view first (Auto mode + parseable JSON).
    if app.view == OutputView::Auto {
        if let Some(json) = &app.last_json {
            if let Some((headers, rows)) = table_from_json(json) {
                if !rows.is_empty() {
                    let inner_w = area.width.saturating_sub(2);
                    let widths = column_widths(&headers, &rows, inner_w);
                    let cell_widths: Vec<usize> = widths
                        .iter()
                        .map(|c| match c {
                            Constraint::Length(l) => (*l).max(1) as usize,
                            _ => 20,
                        })
                        .collect();
                    let header_cells = headers
                        .iter()
                        .zip(&cell_widths)
                        .map(|(h, w)| {
                            Cell::from(truncate_cell(h, *w))
                                .style(Style::default().fg(Color::Black).bg(THEME.accent).add_modifier(Modifier::BOLD))
                        })
                        .collect::<Vec<_>>();
                    let header = Row::new(header_cells);
                    let body = rows.iter().enumerate().map(|(i, r)| {
                        let cells = r
                            .iter()
                            .zip(&cell_widths)
                            .map(|(c, w)| Cell::from(wrap_text(c, *w)))
                            .collect::<Vec<_>>();
                        let style = if i % 2 == 0 {
                            Style::default().fg(Color::White)
                        } else {
                            Style::default().fg(Color::White).bg(Color::Indexed(235))
                        };
                        Row::new(cells).style(style)
                    });
                    let mut table_state = TableState::default();
                    table_state.scroll_down_by(app.output_scroll);
                    let table = Table::new(body, widths)
                        .header(header)
                        .block(block)
                        .column_spacing(2)
                        .row_highlight_style(Style::default().bg(Color::Black));
                    f.render_stateful_widget(table, area, &mut table_state);

                    let total = rows.len() as u16 + 1;
                    let viewport = area.height.saturating_sub(2);
                    if total > viewport {
                        let max_scroll = total.saturating_sub(viewport);
                        if app.output_scroll > max_scroll {
                            app.output_scroll = max_scroll;
                        }
                        let mut sb = ScrollbarState::new(total as usize)
                            .position(app.output_scroll as usize);
                        f.render_stateful_widget(
                            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
                            area,
                            &mut sb,
                        );
                    }
                    return;
                }
            }
        }
    }

    // Raw / syntax-highlighted JSON
    let lines = highlight_json(&app.output);
    let out = Paragraph::new(lines).block(block).scroll((app.output_scroll, 0));
    f.render_widget(out, area);
    let total = app.output_line_count();
    let viewport = area.height.saturating_sub(2);
    if total > viewport {
        let max_scroll = total.saturating_sub(viewport);
        if app.output_scroll > max_scroll {
            app.output_scroll = max_scroll;
        }
        let mut sb = ScrollbarState::new(total as usize).position(app.output_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            area,
            &mut sb,
        );
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let status_style = match &app.status {
        s if s.starts_with("ok") => Style::default().fg(THEME.ok),
        s if s.starts_with("failed") || s.starts_with("running") => Style::default().fg(THEME.err),
        _ => Style::default().fg(Color::DarkGray),
    };
    let footer = Line::from(vec![
        Span::styled(" [F2] region  [F3] profile  [Tab] focus  ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter run  ", Style::default().fg(THEME.ok).add_modifier(Modifier::BOLD)),
        Span::styled("[PgUp/PgDn] scroll  [v] table/json  ", Style::default().fg(Color::DarkGray)),
        Span::styled("[Esc] back  [q/Ctrl-C] quit", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(
                "  |  {}{}",
                app.provenance_line,
                if app.status.is_empty() {
                    String::new()
                } else {
                    format!("  |  {}", app.status)
                }
            ),
            status_style,
        ),
    ]);
    f.render_widget(Paragraph::new(footer).style(Style::default().bg(Color::Black)), area);
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
            let selected = i == app.picker_idx;
            let mark = if selected { "▸ " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::default().fg(THEME.accent)),
                Span::styled(name.clone(), Style::default().fg(if selected { Color::Black } else { Color::White })),
            ]))
        })
        .collect();
    let list = List::new(list_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::default().fg(THEME.accent))
                .title(Span::styled(title, Style::default().fg(THEME.accent).add_modifier(Modifier::BOLD)))
                .style(Style::default().bg(Color::DarkGray)),
        )
        .highlight_style(Style::default().bg(THEME.accent).fg(Color::Black).add_modifier(Modifier::BOLD))
        .highlight_symbol("");
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

/// Try to present a JSON value as a (headers, rows) table.
/// Handles: array of objects, array of scalars, object-of-objects via its
/// first list-valued key, and flat key/value maps.
fn table_from_json(value: &serde_json::Value) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    match value {
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return None;
            }
            if items.iter().all(|i| i.is_object()) {
                let mut headers: Vec<String> = Vec::new();
                for item in items {
                    if let Some(obj) = item.as_object() {
                        for (k, _) in obj {
                            if !headers.contains(k) {
                                headers.push(k.clone());
                            }
                        }
                    }
                }
                if headers.is_empty() {
                    return None;
                }
                let rows = items
                    .iter()
                    .map(|item| {
                        let obj = item.as_object().unwrap();
                        headers
                            .iter()
                            .map(|h| obj.get(h).map(cell_str).unwrap_or_default())
                            .collect::<Vec<String>>()
                    })
                    .collect();
                Some((headers, rows))
            } else if items.iter().all(|i| i.is_string() || i.is_number() || i.is_boolean() || i.is_null()) {
                let rows = items
                    .iter()
                    .map(|i| vec![cell_str(i)])
                    .collect::<Vec<Vec<String>>>();
                Some((vec!["value".to_string()], rows))
            } else {
                None
            }
        }
        serde_json::Value::Object(map) => {
            // Find the first list-valued key whose items are objects.
            for (_key, val) in map {
                if let serde_json::Value::Array(items) = val {
                    if !items.is_empty() && items.iter().all(|i| i.is_object()) {
                        let mut headers: Vec<String> = Vec::new();
                        for item in items {
                            for (k, _) in item.as_object().unwrap() {
                                if !headers.contains(k) {
                                    headers.push(k.clone());
                                }
                            }
                        }
                        if headers.is_empty() {
                            continue;
                        }
                        let rows = items
                            .iter()
                            .map(|item| {
                                let obj = item.as_object().unwrap();
                                headers
                                    .iter()
                                    .map(|h| obj.get(h).map(cell_str).unwrap_or_default())
                                    .collect::<Vec<String>>()
                            })
                            .collect();
                        return Some((headers, rows));
                    }
                }
            }
            // Flat map of scalars -> two column key/value table.
            if map.values().all(|v| v.is_string() || v.is_number() || v.is_boolean() || v.is_null()) {
                let rows = map
                    .iter()
                    .map(|(k, v)| vec![k.clone(), cell_str(v)])
                    .collect::<Vec<Vec<String>>>();
                return Some((vec!["key".to_string(), "value".to_string()], rows));
            }
            None
        }
        _ => None,
    }
}

fn cell_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => compact_value(other, 48),
    }
}

fn compact_value(v: &serde_json::Value, budget: usize) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }
            let mut parts: Vec<String> = Vec::new();
            let mut len = 0usize;
            for it in items {
                let p = compact_value(it, budget.saturating_sub(len).max(4) / 2);
                len += p.len() + 2;
                if len > budget {
                    break;
                }
                parts.push(p);
            }
            if items.len() > parts.len() {
                format!("[{} items]", items.len())
            } else {
                format!("[{}]", parts.join(", "))
            }
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return "{}".to_string();
            }
            let mut parts: Vec<String> = Vec::new();
            let mut len = 1usize;
            for (k, _) in map {
                let kv = k.clone();
                len += kv.len() + 2;
                if len > budget {
                    break;
                }
                parts.push(kv);
            }
            if map.len() > parts.len() {
                format!("{{{}, …}}", parts.join(", "))
            } else {
                format!("{{{}}}", parts.join(", "))
            }
        }
    }
}

fn truncate_cell(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_string();
    }
    let take = width.saturating_sub(1);
    let head: String = chars[..take].iter().collect();
    format!("{head}…")
}

fn column_widths(headers: &[String], rows: &[Vec<String>], available: u16) -> Vec<Constraint> {
    let n = headers.len();
    let min_w = 4u16;
    let max_w = 40u16;
    let mut desired: Vec<u16> = headers
        .iter()
        .map(|h| h.chars().count() as u16)
        .collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            if i >= desired.len() {
                continue;
            }
            let len = c.chars().count() as u16;
            if len > desired[i] {
                desired[i] = len;
            }
        }
    }
    for d in &mut desired {
        *d = (*d).clamp(min_w, max_w);
    }
    let spacing = n.saturating_sub(1) as u16;
    let total: u16 = desired.iter().sum::<u16>() + spacing;
    if total <= available {
        return desired.iter().map(|d| Constraint::Length(*d)).collect();
    }
    // Too wide: shrink each column proportionally so the whole table fits.
    let budget = available.saturating_sub(spacing);
    let sumw: u64 = desired.iter().map(|d| *d as u64).sum();
    desired
        .iter()
        .map(|d| {
            let scaled = ((budget as u64) * (*d as u64) / sumw.max(1)) as u16;
            Constraint::Length(scaled.max(2))
        })
        .collect()
}

/// Minimal JSON syntax highlighter producing styled lines.
fn highlight_json(s: &str) -> Vec<Line<'static>> {
    let key_style = Style::default().fg(Color::Cyan);
    let str_style = Style::default().fg(Color::LightGreen);
    let num_style = Style::default().fg(Color::Yellow);
    let word_style = Style::default().fg(Color::Magenta);
    let punct_style = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                let mut buf = String::new();
                buf.push('"');
                let mut escaped = false;
                while let Some(c) = chars.next() {
                    buf.push(c);
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        break;
                    }
                }
                // A string is a key if the next non-space char is ':'
                let mut look = chars.clone();
                let next_non_space = look.find(|c| !c.is_whitespace());
                let is_key = next_non_space == Some(':');
                spans.push(Span::styled(buf, if is_key { key_style } else { str_style }));
            }
            '0'..='9' | '-' => {
                let mut buf = String::new();
                buf.push(ch);
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E' {
                        buf.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                spans.push(Span::styled(buf, num_style));
            }
            't' | 'f' | 'n' => {
                let mut buf = String::new();
                buf.push(ch);
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphabetic() {
                        buf.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                spans.push(Span::styled(buf, word_style));
            }
            '\n' => {
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
            c if c.is_whitespace() => {
                spans.push(Span::raw(c.to_string()));
            }
            c => {
                spans.push(Span::styled(c.to_string(), punct_style));
            }
        }
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}
