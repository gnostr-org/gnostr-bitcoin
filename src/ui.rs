use std::{io, sync::{mpsc, Arc, Mutex}, time::{Duration, Instant}};
use crossterm::{
    event::{self, Event as CEvent, KeyCode, KeyEventKind},
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
}

impl App {
    pub fn new(messages: Arc<Mutex<Vec<String>>>) -> App {
        App {
            messages,
            scroll_state: 0,
        }
    }

    pub fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel();
        let tick_rate = Duration::from_millis(250);
        std::thread::spawn(move || {
            let mut last_tick = Instant::now();
            loop {
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

        loop {
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(0)].as_ref())
                    .split(f.size());

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

                f.render_widget(paragraph, chunks[0]);
            })?;

            match rx.recv()? {
                Event::Input(event) => match event.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Down => self.scroll_state = self.scroll_state.saturating_add(1),
                    KeyCode::Up => self.scroll_state = self.scroll_state.saturating_sub(1),
                    _ => {},
                },
                Event::Tick => {},
            }
        }
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
