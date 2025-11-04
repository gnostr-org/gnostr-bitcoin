use std::{io, sync::{mpsc, Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use crossterm::{
    event::{self, Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};

pub enum Event<I> {
    Input(I),
    Tick,
}

pub struct App {
    pub messages: Arc<Mutex<Vec<String>>>,
    pub scroll_state: u16,
    pub running: Arc<AtomicBool>,
    pub block_height: Arc<Mutex<i32>>,
}

impl App {
    pub fn new(messages: Arc<Mutex<Vec<String>>>, running: Arc<AtomicBool>, block_height: Arc<Mutex<i32>>) -> App {
        App {
            messages,
            scroll_state: 0,
            running,
            block_height,
        }
    }

    pub fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        let tick_rate = Duration::from_millis(250);
        let running_clone = self.running.clone();
        std::thread::spawn(move || {
            let mut last_tick = Instant::now();
            loop {
                if !running_clone.load(Ordering::SeqCst) {
                    break;
                }
                let timeout = tick_rate
                    .checked_sub(last_tick.elapsed())
                    .unwrap_or_else(|| Duration::from_secs(0));

                if event::poll(timeout).expect("poll works") {
                    if let CEvent::Key(key) = event::read().expect("can read events") {
                        tx.send(Event::Input(key)).expect("can send events");
                    }
                }

                if last_tick.elapsed() >= tick_rate {
                    tx.send(Event::Tick).expect("can send tick event");
                    last_tick = Instant::now();
                }
            }
        });

        while self.running.load(Ordering::SeqCst) {
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
                    .split(f.size());

                let block_height_value = *self.block_height.lock().unwrap();
                let block_height_widget = Paragraph::new(format!("Current Block Height: {}", block_height_value))
                    .block(Block::default().borders(Borders::ALL).title("Block Height"))
                    .style(Style::default().fg(Color::Cyan));
                f.render_widget(block_height_widget, chunks[0]);

                let messages = self.messages.lock().unwrap();
                let formatted_messages: Vec<Line> = messages
                    .iter()
                    .map(|msg| Line::from(Span::raw(msg.clone())))
                    .collect();

                let paragraph = Paragraph::new(formatted_messages)
                    .block(Block::default().borders(Borders::ALL).title("Bitcoin P2P Client Log"))
                    .style(Style::default().fg(Color::White))
                    .wrap(Wrap { trim: true })
                    .scroll((self.scroll_state, 0));

                f.render_widget(paragraph, chunks[1]);
            })?;

            match rx.recv_timeout(tick_rate) {
                Ok(Event::Input(event)) => match event.code {
                    KeyCode::Char('q') => {
                        self.running.store(false, Ordering::SeqCst);
                    },
                    KeyCode::Down => self.scroll_state = self.scroll_state.saturating_add(1),
                    KeyCode::Up => self.scroll_state = self.scroll_state.saturating_sub(1),
                    _ => {},
                },
                Ok(Event::Tick) => {},
                Err(mpsc::RecvTimeoutError::Timeout) => {},
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.running.store(false, Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }
}

pub fn init_tui() -> anyhow::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

pub fn restore_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
