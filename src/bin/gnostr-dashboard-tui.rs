// src/bin/gnostr_dashboard.rs
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use std::{io, process::Stdio, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use vt100::Parser;

struct TuiNode {
    name: String,
    parser: Arc<Mutex<Parser>>,
}

impl TuiNode {
    fn new(name: &str, width: u16, height: u16) -> Self {
        Self {
            name: name.to_string(),
            parser: Arc::new(Mutex::new(Parser::new(height, width, 100))),
        }
    }

    async fn spawn(&self, args: Vec<String>) -> io::Result<tokio::process::Child> {
        let mut child = Command::new("cargo")
            .args(["run", "--bin", "gnostr-bitcoin", "--"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Force child to treat the pipe as a color-capable terminal
            .env("TERM", "xterm-256color") 
            .env("COLORTERM", "truecolor")
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

    // IMPORTANT: Match internal resolution to the approximate quadrant size
    let nodes = vec![
        TuiNode::new("Node 1 (Hub)", 120, 40),
        TuiNode::new("Node 2", 120, 40),
        TuiNode::new("Node 3", 120, 40),
        TuiNode::new("Node 4", 120, 40),
    ];

    let mut children = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        let mut args = vec!["--datadir".into(), format!("./test_data_{}", i + 1)];
        if i == 0 { args.push("--listen".into()); }
        else { args.extend(vec!["--target-peer-addr".into(), "127.0.0.1:8333".into()]); }
        children.push(node.spawn(args).await?);
    }

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)])
                .split(f.area());

            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);

            for (r_idx, row_area) in rows.iter().enumerate() {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(*row_area);

                for (c_idx, col_area) in cols.iter().enumerate() {
                    let idx = r_idx * 2 + c_idx;
                    let p = nodes[idx].parser.lock().unwrap();
                    let screen = p.screen();
                    
                    let mut lines = Vec::new();
                    for row in 0..screen.size().0 {
                        let mut spans = Vec::new();
                        for col in 0..screen.size().1 {
                            if let Some(cell) = screen.cell(row, col) {
                                let style = Style::default()
                                    .fg(match cell.fgcolor() {
                                        vt100::Color::Default => Color::Reset,
                                        vt100::Color::Idx(i) => Color::Indexed(i),
                                        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
                                    })
                                    .bg(match cell.bgcolor() {
                                        vt100::Color::Default => Color::Reset,
                                        vt100::Color::Idx(i) => Color::Indexed(i),
                                        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
                                    });
                                spans.push(Span::styled(cell.contents().to_string(), style));
                            }
                        }
                        lines.push(Line::from(spans));
                    }

                    f.render_widget(
                        Paragraph::new(lines)
                            .block(Block::default().title(nodes[idx].name.as_str()).borders(Borders::ALL)),
                        *col_area,
                    );
                }
            }
        })?;

        if event::poll(Duration::from_millis(33))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') { break; }
            }
        }
    }

    for mut child in children { let _ = child.kill().await; }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}
