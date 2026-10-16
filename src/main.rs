use std::path::PathBuf;

use aws_tui::audit::AuditLogger;
use aws_tui::awsconfig::build_environment;
use aws_tui::index::SearchIndex;
use aws_tui::loader::{default_data_dir, load_services};
use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::CrosstermBackend;

mod app;
mod jsonview;
mod theme;
mod ui;

use app::{App, CLI_VERSION, run_tui};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir =
        default_data_dir().ok_or("aws-cli botocore data dir not found; set AWS_TUI_DATA_DIR")?;
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
    let base = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aws-tui");
    let aws_bin = std::env::var("AWS_TUI_AWS_BIN").unwrap_or_else(|_| "aws".to_string());
    let audit = AuditLogger::new(&base, aws_bin.clone())?;
    let env = build_environment();

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(tree, index, audit, aws_bin, env);

    let res = run_tui(&mut terminal, &mut app);

    disable_raw_mode()?;
    terminal.show_cursor()?;
    terminal.clear()?;
    drop(terminal);
    std::io::stdout().execute(LeaveAlternateScreen)?;
    res
}
