// src/bin/gnostr_dashboard.rs
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use std::{
    io::{self, Read},
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::{Duration, Instant},
};
use vt100::Parser;

const BITCOIN_LOGO: [&str; 15] = [
    "⠀⠀⠀⠀⠀⠀⠀⠀⣀⣤⣴⣶⣾⣿⣿⣿⣿⣷⣶⣦⣤⣀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⣠⣴⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣦⣄⠀⠀⠀⠀⠀",
    "⠀⠀⠀⣠⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣄⠀⠀⠀",
    "⠀⠀⣴⣿⣿⣿⣿⣿⣿⣿⠟⠿⠿⡿⠀⢰⣿⠁⢈⣿⣿⣿⣿⣿⣿⣿⣿⣦⠀⠀",
    "⠀⣼⣿⣿⣿⣿⣿⣿⣿⣿⣤⣄⠀⠀⠀⠈⠉⠀⠸⠿⣿⣿⣿⣿⣿⣿⣿⣿⣧⠀",
    "⢰⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠀⠀⢠⣶⣶⣤⡀⠀⠈⢻⣿⣿⣿⣿⣿⣿⣿⡆",
    "⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠃⠀⠀⠼⣿⣿⡿⠃⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣷",
    "⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡟⠀⠀⢀⣀⣀⠀⠀⠀⠀⢴⣿⣿⣿⣿⣿⣿⣿⣿⣿",
    "⢿⣿⣿⣿⣿⣿⣿⣿⢿⣿⠁⠀⠀⣼⣿⣿⣿⣦⠀⠀⠈⢻⣿⣿⣿⣿⣿⣿⣿⡿",
    "⠸⣿⣿⣿⣿⣿⣿⣏⠀⠀⠀⠀⠀⠛⠛⠿⠟⠋⠀⠀⠀⣾⣿⣿⣿⣿⣿⣿⣿⠇",
    "⠀⢻⣿⣿⣿⣿⣿⣿⣿⣿⠇⠀⣤⡄⠀⣀⣀⣀⣀⣠⣾⣿⣿⣿⣿⣿⣿⣿⡟⠀",
    "⠀⠀⠻⣿⣿⣿⣿⣿⣿⣿⣄⣰⣿⠁⢀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠟⠀⠀",
    "⠀⠀⠀⠙⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠋⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠙⠻⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠟⠋⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠉⠛⠻⠿⢿⣿⣿⣿⣿⡿⠿⠟⠛⠉⠀⠀⠀⠀⠀⠀⠀⠀",
];

struct TuiNode {
    parser: Arc<Mutex<Parser>>,
    pty_pair: portable_pty::PtyPair,
    ready: Arc<AtomicBool>,
}

impl TuiNode {
    fn new(width: u16, height: u16) -> Self {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize { rows: height, cols: width, pixel_width: 0, pixel_height: 0 })
            .expect("failed to open pty");

        Self {
            parser: Arc::new(Mutex::new(Parser::new(height, width, 100))),
            pty_pair,
            ready: Arc::new(AtomicBool::new(false)),
        }
    }

    fn spawn(&self, args: Vec<String>, cwd: PathBuf) -> io::Result<()> {
        let mut cmd = CommandBuilder::new("cargo");
        cmd.args(["run", "--bin", "gnostr-bitcoin", "--"]);
        cmd.args(args);
        cmd.cwd(cwd); 
        cmd.env("TERM", "xterm-256color");
        let _child = self.pty_pair.slave.spawn_command(cmd).expect("failed to spawn command");

        let mut reader = self.pty_pair.master.try_clone_reader().expect("failed to clone reader");
        let parser = Arc::clone(&self.parser);
        let ready = Arc::clone(&self.ready);

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 { break; }
                ready.store(true, Ordering::SeqCst);
                let mut p = parser.lock().unwrap();
                p.process(&buf[..n]);
            }
        });
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    execute!(io::stdout(), EnterAlternateScreen)?;

    let nodes = vec![TuiNode::new(80, 24), TuiNode::new(80, 24)];
    let project_root = std::env::current_dir()?;

    for (i, node) in nodes.iter().enumerate() {
        let mut args = vec!["--datadir".into(), format!("./test_data_{}", i + 1)];
        if i == 1 { args.extend(vec!["--target-peer-addr".into(), "127.0.0.1:8333".into()]); }
        node.spawn(args, project_root.clone())?;
    }

    let start_time = Instant::now();
    let min_splash_duration = Duration::from_secs(3); // PERSISTENCE TIMER

    loop {
        terminal.draw(|f| {
            let area = f.area();
            
            // 1. ALWAYS render the Dashboard content first (Bottom Layer)
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(48), Constraint::Min(2), Constraint::Percentage(48)])
                .split(area);

            for (idx, &chunk_idx) in [0, 2].iter().enumerate() {
                let chunk = chunks[chunk_idx];
                let p = nodes[idx].parser.lock().unwrap();
                let screen = p.screen();
                let mut lines = Vec::new();
                for row in 0..screen.size().0 {
                    let mut spans = Vec::new();
                    for col in 0..screen.size().1 {
                        if let Some(cell) = screen.cell(row, col) {
                            spans.push(Span::raw(cell.contents().to_string()));
                        }
                    }
                    lines.push(Line::from(spans));
                }
                f.render_widget(Paragraph::new(lines), chunk);
            }

            // 2. LAYER the Logo on top if nodes aren't ready OR min time hasn't passed
            let nodes_ready = nodes.iter().all(|n| n.ready.load(Ordering::SeqCst));
            let time_passed = start_time.elapsed() > min_splash_duration;

            if !nodes_ready || !time_passed {
                let splash_area = centered_rect(70, 70, area);
                
                // Clear the terminal area where the logo will sit so they don't overlap messy-ly
                f.render_widget(Clear, splash_area); 
                
                let logo_lines: Vec<Line> = BITCOIN_LOGO.iter()
                    .map(|&l| Line::from(Span::styled(l, Style::default().fg(Color::Rgb(247, 147, 26)))))
                    .collect();

                f.render_widget(
                    Paragraph::new(logo_lines)
                        .alignment(Alignment::Center)
                        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray))),
                    splash_area
                );
            }
        })?;

        if event::poll(Duration::from_millis(33))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') { break; }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
