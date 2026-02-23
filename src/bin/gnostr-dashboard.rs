use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use std::{io, process::Stdio, time::{Duration, Instant}};
use tokio::process::Command;

struct NodeState {
    name: String,
    datadir: String,
    args: Vec<String>,
    status: String,
}

impl NodeState {
    fn new(id: usize, is_listener: bool) -> Self {
        let datadir = format!("./test_data_{}", id);
        let mut args = vec!["--datadir".to_string(), datadir.clone()];
        
        if is_listener {
            args.push("--listen".to_string());
        } else {
            args.push("--target-peer-addr".to_string());
            args.push("127.0.0.1:8333".to_string());
        }

        Self {
            name: format!("Node {}", id),
            datadir,
            args,
            status: "Ready".to_string(),
        }
    }

    /// The Cross-Platform System Command
    async fn spawn_node(&mut self) -> io::Result<tokio::process::Child> {
        self.status = "Starting...".to_string();
        
        // Construct: cargo run --bin gnostr-bitcoin -- <args>
        let mut cmd = Command::new("cargo");
        cmd.arg("run")
           .arg("--bin")
           .arg("gnostr-bitcoin")
           .arg("--")
           .args(&self.args)
           .stdout(Stdio::piped())
           .stderr(Stdio::null()); // Silence cargo warnings/build noise

        let child = cmd.spawn()?;
        self.status = "Running".to_string();
        Ok(child)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 2. Initialize Nodes
    let mut nodes = vec![
        NodeState::new(1, true),
        NodeState::new(2, false),
        NodeState::new(3, false),
        NodeState::new(4, false),
    ];

    // 3. Spawn Subprocesses
    let mut children = Vec::new();
    for node in nodes.iter_mut() {
        // Ensure data directory exists
        std::fs::create_dir_all(&node.datadir)?;
        children.push(node.spawn_node().await?);
    }

    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    // 4. Main Event Loop
    loop {
        terminal.draw(|f| draw_ui(f, &nodes))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if let KeyCode::Char('q') = key.code {
                    // Kill all children on exit
                    for mut child in children {
                        let _ = child.kill().await;
                    }
                    break;
                }
            }
        }
        
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    // 5. Restoration
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    Ok(())
}

fn draw_ui(f: &mut ratatui::Frame, nodes: &[NodeState]) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(f.size());

    // Monitor Pane
    f.render_widget(
        Paragraph::new(format!("Active Managed Processes: {} | Cross-platform Mode: ON", nodes.len()))
            .block(Block::default().title(" System Monitor ").borders(Borders::ALL)),
        chunks[0],
    );

    // 2x2 Grid
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    for (row_idx, row_area) in rows.iter().enumerate() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(*row_area);
            
        for (col_idx, col_area) in cols.iter().enumerate() {
            let node_idx = row_idx * 2 + col_idx;
            if let Some(node) = nodes.get(node_idx) {
                let block = Block::default().title(node.name.as_str()).borders(Borders::ALL);
                let info = format!("DataDir: {}\nStatus: {}\nArgs: {:?}", node.datadir, node.status, node.args);
                f.render_widget(Paragraph::new(info).block(block), *col_area);
            }
        }
    }

    f.render_widget(Paragraph::new(" [q] Kill Nodes & Quit | Dashboard v2.0 "), chunks[2]);
}
