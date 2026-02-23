// src/bin/gnostr_dashboard.rs
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Terminal,
};
use std::{
    io::{self, Read},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use vt100::Parser;

struct TuiNode {
    parser: Arc<Mutex<Parser>>,
    pty_pair: portable_pty::PtyPair,
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
        }
    }

    fn spawn(&self, args: Vec<String>, cwd: PathBuf) -> io::Result<()> {
        let mut cmd = CommandBuilder::new("cargo");
        cmd.args(["run", "--bin", "gnostr-bitcoin", "--"]);
        cmd.args(args);
        
        // CRITICAL: Setting the Current Working Directory
        cmd.cwd(cwd); 
        
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        // Spawn the command attached to the PTY slave
        let _child = self.pty_pair.slave.spawn_command(cmd).expect("failed to spawn command");

        let mut reader = self.pty_pair.master.try_clone_reader().expect("failed to clone reader");
        let parser = Arc::clone(&self.parser);

        // Reader thread to feed the ANSI parser
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 { break; }
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
            // Inform the child process of the new window size (squeezing)
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
    
    // Define the CWD - change this to match your project structure if needed
    let project_root = std::env::current_dir()?;

    for (i, node) in nodes.iter().enumerate() {
        let mut args = vec!["--datadir".into(), format!("./test_data_{}", i + 1)];
        if i == 0 { args.push("--listen".into()); }
        else { args.extend(vec!["--target-peer-addr".into(), "127.0.0.1:8333".into()]); }
        
        node.spawn(args, project_root.clone())?;
    }

    let tick_rate = Duration::from_millis(33);
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|f| {
            let area = f.area();
            
            // Your 48/48 logic with a spacer
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
                nodes[idx].resize(chunk.width, chunk.height);

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
                f.render_widget(Paragraph::new(lines), chunk);
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
