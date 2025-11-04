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

#[derive(PartialEq)]
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
    last_user_input_time: Instant,
    auto_scroll_enabled: bool,
    current_scroll_y: f32, // Animated scroll position
    scroll_animation_speed: f32, // Controls animation speed
    log_widget_height: u16,
}

impl App {
    pub fn new(messages: Arc<Mutex<Vec<String>>>, running: Arc<AtomicBool>, block_height: Arc<Mutex<i32>>) -> App {
        App {
            messages,
            scroll_state: 0,
            running,
            block_height,
            focused_widget: FocusedWidget::Log,
            last_user_input_time: Instant::now(),
            auto_scroll_delay: Duration::from_secs(5),
            auto_scroll_enabled: true,
            current_scroll_y: 0.0, // Initialize animated scroll position
            scroll_animation_speed: 0.1, // Initialize animation speed
            log_widget_height: 0,
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
                let size = f.size();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Length(3), Constraint::Min(0)].as_ref())
                    .split(size);

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

                let peer_list_placeholder = Paragraph::new("Peer List:\n- 127.0.0.1:8333\n- ...")
    .block(Block::default().borders(Borders::ALL).title("Peers").border_style(match self.focused_widget {
        FocusedWidget::Instructions => Style::default().fg(Color::Magenta),
        _ => Style::default().fg(Color::Yellow),
    }))
    .style(Style::default().fg(Color::Yellow));
f.render_widget(peer_list_placeholder, instruction_chunks[2]);

                let messages = self.messages.lock().unwrap();
                let num_messages = messages.len();
                let log_height = {
                    let content_height = if num_messages == 0 { 1 } else { num_messages };
                    let desired_height = (content_height + 2) as u16; // +2 for borders
                    let max_height = size.height / 2;
                    std::cmp::min(desired_height, max_height)
                };
                self.log_widget_height = log_height;

                let log_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(log_height), Constraint::Min(0)].as_ref())
                    .split(chunks[2]);

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
                    .scroll((self.current_scroll_y.round() as u16, 0));

                f.render_widget(paragraph, log_chunks[0]);
            })?;

            match rx.recv_timeout(tick_rate) {
                Ok(Event::Input(event)) => {
                    self.last_user_input_time = Instant::now(); // Update timer on any input
                    match event.code {
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
                                let messages_count = self.messages.lock().unwrap().len();
                                let visible_lines = self.log_widget_height.saturating_sub(2);
                                let max_scroll = if (messages_count as u16) > visible_lines {
                                    messages_count as u16 - visible_lines
                                } else {
                                    0
                                };
                                if self.scroll_state >= max_scroll {
                                    self.scroll_state = max_scroll;
                                    self.auto_scroll_enabled = true;
                                } else {
                                    self.auto_scroll_enabled = false;
                                }
                            }
                        },
                        KeyCode::Up => {
                            if let FocusedWidget::Log = self.focused_widget {
                                self.scroll_state = self.scroll_state.saturating_sub(1);
                                self.auto_scroll_enabled = false; // User manually scrolled
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
                        KeyCode::Esc => {
                            if self.focused_widget == FocusedWidget::Log && !self.auto_scroll_enabled {
                                self.auto_scroll_enabled = true;
                                self.last_user_input_time = Instant::now(); // Reset timer to allow auto-scroll after delay
                            }
                        },
                        KeyCode::Enter => {
                            if let FocusedWidget::Log = self.focused_widget {
                                let messages_count = self.messages.lock().unwrap().len();
                                if messages_count > 0 {
                                    let visible_lines = self.log_widget_height.saturating_sub(2);
                                    let max_scroll = if (messages_count as u16) > visible_lines {
                                        messages_count as u16 - visible_lines
                                    } else {
                                        0
                                    };
                                    self.scroll_state = max_scroll;
                                } else {
                                    self.scroll_state = 0;
                                }
                                self.auto_scroll_enabled = true;
                            }
                        },
                        _ => {},
                    }
                },
                Ok(Event::Tick) => {
                    // --- Scrolling Animation Logic ---
                    let messages_count = self.messages.lock().unwrap().len();
                    let visible_lines = self.log_widget_height.saturating_sub(2);
                    let bottom_scroll_target = if (messages_count as u16) > visible_lines {
                        (messages_count as u16 - visible_lines) as f32
                    } else {
                        0.0
                    };

                    if self.focused_widget == FocusedWidget::Log {
                        if self.auto_scroll_enabled {
                            // Auto-scroll towards the bottom
                            if self.current_scroll_y < bottom_scroll_target {
                                let diff = bottom_scroll_target - self.current_scroll_y;
                                self.current_scroll_y += diff * self.scroll_animation_speed;
                                // Snap to bottom if very close
                                if (bottom_scroll_target - self.current_scroll_y).abs() < 0.1 {
                                    self.current_scroll_y = bottom_scroll_target;
                                }
                            } else if self.current_scroll_y > bottom_scroll_target {
                                // Ensure we don't scroll past the bottom
                                self.current_scroll_y = bottom_scroll_target;
                            }
                        } else {
                            // Manual scroll animation towards target scroll_state
                            if self.current_scroll_y != self.scroll_state as f32 {
                                let diff = self.scroll_state as f32 - self.current_scroll_y;
                                self.current_scroll_y += diff * self.scroll_animation_speed;
                                // Snap to target if very close
                                if (self.scroll_state as f32 - self.current_scroll_y).abs() < 0.1 {
                                    self.current_scroll_y = self.scroll_state as f32;
                                }
                            }
                        }
                    }
                    // --- End Scrolling Logic ---
                },
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
