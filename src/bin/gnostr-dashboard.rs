// src/bin/gnostr_dashboard.rs
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use std::{io, process::Stdio, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use vt100::Parser;

struct TuiWrapper {
    name: String,
    parser: Arc<Mutex<Parser>>,
}

impl TuiWrapper {
    fn new(name: &str, width: u16, height: u16) -> Self {
        // Fix: Added the missing 3rd argument (scrollback_len)
        Self {
            name: name.to_string(),
            parser: Arc::new(Mutex::new(Parser::new(height, width, 1000))),
        }
    }

    async fn spawn(&self, args: Vec<String>) -> io::Result<tokio::process::Child> {
        let mut child = Command::new("cargo")
            .args(["run", "--bin", "gnostr-bitcoin", "--"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("TERM", "xterm-256color") 
            .spawn()?;

        let mut stdout = child.stdout.take().unwrap();
        let parser = Arc::clone(&self.parser);

        tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            while let Ok(n) = stdout.read(&mut buf).await {
                if n == 0 { break; }
                let mut p = parser.lock().unwrap();
                p.process(&buf[..n]);
            }
        });

        Ok(child)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let wrappers = vec![
        TuiWrapper::new("Node 1 (Primary)", 80, 24),
        TuiWrapper::new("Node 2", 80, 24),
        TuiWrapper::new("Node 3", 80, 24),
        TuiWrapper::new("Node 4", 80, 24),
    ];

    let mut children = Vec::new();
    for (i, w) in wrappers.iter().enumerate() {
        let mut args = vec!["--datadir".into(), format!("./test_data_{}", i + 1)];
        if i == 0 { args.push("--listen".into()); }
        else { args.extend(vec!["--target-peer-addr".into(), "127.0.0.1:8333".into()]); }
        children.push(w.spawn(args).await?);
    }

    let tick_rate = Duration::from_millis(50);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| {
            // Fix: Changed .size() to .area() per deprecation warning
            let area = f.area(); 
            
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(0),
                    Constraint::Length(1),
                ])
                .split(area);

            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);

            for (r, row_area) in rows.iter().enumerate() {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(*row_area);

                for (c, col_area) in cols.iter().enumerate() {
                    let idx = r * 2 + c;
                    let p = wrappers[idx].parser.lock().unwrap();
                    let screen = p.screen();
                    
                    let mut lines = Vec::new();
                    // Map virtual terminal rows to Ratatui Lines
                    for row in 0..screen.size().0 {
                        let mut line_content = String::new();
                        for col in 0..screen.size().1 {
                            if let Some(cell) = screen.cell(row, col) {
                                line_content.push_str(&cell.contents());
                            }
                        }
                        lines.push(Line::from(line_content));
                    }

                    f.render_widget(
                        Paragraph::new(lines)
                            .block(Block::default().title(wrappers[idx].name.as_str()).borders(Borders::ALL)),
                        *col_area,
                    );
                }
            }
        })?;

        if event::poll(tick_rate.saturating_sub(last_tick.elapsed()))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') { break; }
            }
        }
        if last_tick.elapsed() >= tick_rate { last_tick = Instant::now(); }
    }

    // Cleanup
    for mut child in children { let _ = child.kill().await; }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}
