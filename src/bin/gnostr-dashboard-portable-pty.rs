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

struct TuiNode {
    parser: Arc<Mutex<Parser>>,
    pty_pair: portable_pty::PtyPair,
    ready: Arc<AtomicBool>, // Tracks if we've received the first byte of data
}

impl TuiNode {
    fn new(width: u16, height: u16) -> Self {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: height,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            })
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
        cmd.env("COLORTERM", "truecolor");

        let _child = self.pty_pair.slave.spawn_command(cmd).expect("failed to spawn command");

        let mut reader = self.pty_pair.master.try_clone_reader().expect("failed to clone reader");
        let parser = Arc::clone(&self.parser);
        let ready = Arc::clone(&self.ready);

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 { break; }
                if !ready.load(Ordering::SeqCst) {
                    ready.store(true, Ordering::SeqCst);
                }
                let mut p = parser.lock().unwrap();
                p.process(&buf[..n]);
            }
        });

        Ok(())
    }

    fn resize(&self, width: u16, height: u16) {
        let mut p = self.parser.lock().unwrap();
        if p.screen().size() != (height, width) {
            p.set_size(height, width);
            self.pty_pair.master.resize(PtySize {
                rows: height,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            }).ok();
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let nodes = vec![TuiNode::new(80, 24), TuiNode::new(80, 24)];
    let project_root = std::env::current_dir()?;

    for (i, node) in nodes.iter().enumerate() {
        let mut args = vec!["--datadir".into(), format!("./test_data_{}", i + 1)];
        if i == 1 { args.extend(vec!["--target-peer-addr".into(), "127.0.0.1:8333".into()]); }
        node.spawn(args, project_root.clone())?;
    }

    let tick_rate = Duration::from_millis(33);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(48),
                    Constraint::Min(2),
                    Constraint::Percentage(48),
                ])
                .split(area);

            for (idx, &chunk_idx) in [0, 2].iter().enumerate() {
                let chunk = chunks[chunk_idx];
                let node = &nodes[idx];
                
                if node.ready.load(Ordering::SeqCst) {
                    node.resize(chunk.width, chunk.height);
                    let p = node.parser.lock().unwrap();
                    let screen = p.screen();
                    let mut lines = Vec::new();

                    for row in 0..screen.size().0 {
                        let mut spans = Vec::new();
                        for col in 0..screen.size().1 {
                            if let Some(cell) = screen.cell(row, col) {
                                spans.push(Span::styled(
                                    cell.contents().to_string(),
                                    Style::default()
                                        .fg(map_vt_color(cell.fgcolor()))
                                        .bg(map_vt_color(cell.bgcolor()))
                                ));
                            }
                        }
                        lines.push(Line::from(spans));
                    }
                    f.render_widget(Paragraph::new(lines), chunk);
                } else {
                    // LOADING WIDGET
                    let loading_msg = format!(" Loading Node {}... ", idx + 1);
                    let loading_para = Paragraph::new(loading_msg)
                        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                        .alignment(Alignment::Center)
                        .block(Block::default().borders(Borders::ALL).border_type(ratatui::widgets::BorderType::Rounded));
                    
                    // Center the loading box vertically
                    let vertical_center = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Percentage(40),
                            Constraint::Length(3),
                            Constraint::Percentage(40),
                        ])
                        .split(chunk)[1];

                    f.render_widget(loading_para, vertical_center);
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

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

fn map_vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
