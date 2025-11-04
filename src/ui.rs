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

pub enum FocusedWidget {
    BlockHeight,
    Instructions,
    Log,
}

pub struct App {
    pub messages: Arc<Mutex<Vec<String>>>,
    pub scroll_state: u16,
    pub running: Arc<AtomicBool>,
    pub block_height: Arc<Mutex<i32>>,
    pub focused_widget: FocusedWidget,
}

impl App {
    pub fn new(messages: Arc<Mutex<Vec<String>>>, running: Arc<AtomicBool>, block_height: Arc<Mutex<i32>>) -> App {
        App {
            messages,
            scroll_state: 0,
            running,
            block_height,
            focused_widget: FocusedWidget::Log,
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
                    .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Min(0)].as_ref())
                    .split(f.size());

                let block_height_value = *self.block_height.lock().unwrap();
                let block_height_widget = Paragraph::new(format!("Current Block Height: {}", block_height_value))
                    .block(Block::default().borders(Borders::ALL).title("Block Height").border_style(match self.focused_widget {
                        FocusedWidget::BlockHeight => Style::default().fg(Color::Magenta),
                        _ => Style::default().fg(Color::White),
                    }))
                    .style(Style::default().fg(Color::Cyan));
                f.render_widget(block_height_widget, chunks[0]);

                let instruction_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(33), Constraint::Percentage(33), Constraint::Percentage(34)].as_ref())
                    .split(chunks[1]);

                let instruction_quit = Paragraph::new("Press 'q' to quit.")
                    .block(Block::default().borders(Borders::ALL).title("Instructions").border_style(match self.focused_widget {
                        FocusedWidget::Instructions => Style::default().fg(Color::Magenta),
                        _ => Style::default().fg(Color::Yellow),
                    }))
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(instruction_quit, instruction_chunks[0]);

                let instruction_scroll_up = Paragraph::new("Press 'Up' to scroll up.")
                    .block(Block::default().borders(Borders::ALL).title("").border_style(match self.focused_widget {
                        FocusedWidget::Instructions => Style::default().fg(Color::Magenta),
                        _ => Style::default().fg(Color::Yellow),
                    }))
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(instruction_scroll_up, instruction_chunks[1]);

                let instruction_scroll_down = Paragraph::new("Press 'Down' to scroll down.")
                    .block(Block::default().borders(Borders::ALL).title("").border_style(match self.focused_widget {
                        FocusedWidget::Instructions => Style::default().fg(Color::Magenta),
                        _ => Style::default().fg(Color::Yellow),
                    }))
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(instruction_scroll_down, instruction_chunks[2]);

                let messages = self.messages.lock().unwrap();
                let formatted_messages: Vec<Line> = messages
                    .iter()
                    .map(|msg| Line::from(Span::raw(msg.clone())))
                    .collect();

                let paragraph = Paragraph::new(formatted_messages)
                    .block(Block::default().borders(Borders::ALL).title("Bitcoin P2P Client Log").border_style(match self.focused_widget {
                        FocusedWidget::Log => Style::default().fg(Color::Magenta),
                        _ => Style::default().fg(Color::White),
                    }))
                    .style(Style::default().fg(Color::White))
                    .wrap(Wrap { trim: true })
                    .scroll((self.scroll_state, 0));

                f.render_widget(paragraph, chunks[2]);
            })?;

            match rx.recv_timeout(tick_rate) {
                Ok(Event::Input(event)) => match event.code {
                    KeyCode::Char('q') => {
                        self.running.store(false, Ordering::SeqCst);
                    },
                    KeyCode::Tab => {
                        self.focused_widget = match self.focused_widget {
                            FocusedWidget::BlockHeight => FocusedWidget::Instructions,
                            FocusedWidget::Instructions => FocusedWidget::Log,
                            FocusedWidget::Log => FocusedWidget::BlockHeight,
                        };
                    },
                    KeyCode::Down => {
                        if let FocusedWidget::Log = self.focused_widget {
                            self.scroll_state = self.scroll_state.saturating_add(1);
                        }
                    },
                    KeyCode::Up => {
                        if let FocusedWidget::Log = self.focused_widget {
                            self.scroll_state = self.scroll_state.saturating_sub(1);
                        }
                    },
                    KeyCode::Left => {
                        if let FocusedWidget::Instructions = self.focused_widget {
                            self.focused_widget = FocusedWidget::BlockHeight;
                        }
                    },
                    KeyCode::Right => {
                        if let FocusedWidget::Instructions = self.focused_widget {
                            self.focused_widget = FocusedWidget::Log;
                        }
                    },
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
