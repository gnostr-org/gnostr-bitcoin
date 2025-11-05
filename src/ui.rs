use std::collections::HashMap;
use std::{io, sync::{mpsc, Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant, SystemTime}};
use time::{OffsetDateTime, macros::format_description};
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

#[rustfmt::skip]
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

const LOGO_HEIGHT: u16 = 15;
const LOGO_WIDTH: u16 = 80;

const BITCOIN_ICON: [&str; 9] = [
   "⠀⠀⠀⠀⣿⡇⠀⢸⣿⡇⠀⠀⠀⠀",
   "⠸⠿⣿⣿⣿⡿⠿⠿⣿⣿⣿⣶⣄⠀",
   "⠀⠀⢸⣿⣿⡇⠀⠀⠀⠈⣿⣿⣿⠀",
   "⠀⠀⢸⣿⣿⡇⠀⠀⢀⣠⣿⣿⠟⠀",
   "⠀⠀⢸⣿⣿⡿⠿⠿⠿⣿⣿⣥⣄⠀",
   "⠀⠀⢸⣿⣿⡇⠀⠀⠀⠀⢻⣿⣿⣧",
   "⠀⠀⢸⣿⣿⡇⠀⠀⠀⠀⣼⣿⣿⣿",
   "⢰⣶⣿⣿⣿⣷⣶⣶⣾⣿⣿⠿⠛⠁",
   "⠀⠀⠀⠀⣿⡇⠀⢸⣿⡇⠀⠀⠀⠀",
];


const BITCOIN_LOGO_LARGE: [&str; 30] = [
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣠⣤⣴⣶⣶⣿⣿⣿⣿⣿⣿⣿⣿⣶⣶⣶⣤⣄⣀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣠⣴⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣦⣄⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣤⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣤⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣠⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣄⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⣠⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣄⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⣠⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠉⠛⠛⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣄⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⣴⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠁⠀⠀⢰⣿⣿⠇⠀⠉⠉⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣦⠀⠀⠀⠀⠀",
    "⠀⠀⠀⢀⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠉⠉⠛⠛⠿⠿⡏⠀⠀⠀⣾⣿⡿⠀⠀⠀⣸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⡀⠀⠀⠀",
    "⠀⠀⢀⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠙⠛⠃⠀⠀⢀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⡀⠀⠀",
    "⠀⠀⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣶⣆⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠉⠛⠻⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⠀⠀",
    "⠀⣸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠀⠀⠀⠀⠀⢠⣶⣦⣤⣀⡀⠀⠀⠀⠀⠀⠀⠙⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣇⠀",
    "⢀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠁⠀⠀⠀⠀⠀⣼⣿⣿⣿⣿⣿⣷⡄⠀⠀⠀⠀⠀⠈⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡄",
    "⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠀⠀⠀⠀⠀⢠⣿⣿⣿⣿⣿⣿⣿⡿⠀⠀⠀⠀⠀⠀⣸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇",
    "⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠁⠀⠀⠀⠀⠀⠘⠿⠿⢿⣿⣿⡿⠟⠁⠀⠀⠀⠀⠀⢀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿",
    "⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣠⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿",
    "⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠁⠀⠀⠀⠀⠀⣴⣶⣤⣤⣀⡀⠀⠀⠀⠀⠀⠀⠐⠿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿",
    "⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠀⠀⠀⠀⠀⢠⣿⣿⣿⣿⣿⣿⣷⣦⡀⠀⠀⠀⠀⠀⠘⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿",
    "⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠿⠿⢿⡿⠁⠀⠀⠀⠀⠀⣼⣿⣿⣿⣿⣿⣿⣿⣿⣧⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡇",
    "⠈⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠏⠀⠀⠀⠀⠀⠀⠀⠀⠀⠠⣿⣿⣿⣿⣿⣿⣿⣿⣿⠇⠀⠀⠀⠀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠃",
    "⠀⢹⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣯⣄⣀⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠉⠉⠉⠉⠉⠀⠀⠀⠀⠀⠀⠀⢠⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠀",
    "⠀⠀⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣶⠀⠀⠀⢀⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣠⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠀⠀",
    "⠀⠀⠈⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡏⠀⠀⠀⣾⣿⣿⠀⠀⠀⢠⣤⣄⣀⣀⣀⣀⣤⣴⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠁⠀⠀",
    "⠀⠀⠀⠈⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠁⠀⠀⢰⣿⣿⡇⠀⠀⢀⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠁⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠻⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣶⣦⣾⣿⣿⡀⠀⠀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠟⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠙⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠋⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠙⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠋⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠙⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠋⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠛⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠛⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠙⠻⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⠟⠋⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠉⠙⠛⠻⠿⠿⢿⣿⣿⣿⣿⣿⣿⡿⠿⠿⠟⠛⠋⠉⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
];


pub enum Event<I> {
    Input(I),
    Tick,
}

#[derive(PartialEq, Copy, Clone)]
pub enum FocusedWidget {
    BlockHeight,
    Instructions,
    PeerList,
    Log,
}

pub struct App {
    pub messages: Arc<Mutex<Vec<(String, SystemTime)>>>,
    pub scroll_state: u16,
    pub running: Arc<AtomicBool>,
    pub block_height: Arc<Mutex<i32>>,
    pub block_hash: Arc<Mutex<String>>,
    pub peer_list: Arc<Mutex<HashMap<String, (u64, u64)>>>,
    pub focused_widget: FocusedWidget,
    last_user_input_time: Instant,
    auto_scroll_enabled: bool,
    current_scroll_y: f32, // Animated scroll position
    scroll_animation_speed: f32, // Controls animation speed
    log_widget_height: u16,
    pub log_visible: bool,
}

impl App {
    pub fn new(messages: Arc<Mutex<Vec<(String, SystemTime)>>>, running: Arc<AtomicBool>, block_height: Arc<Mutex<i32>>, peer_list: Arc<Mutex<HashMap<String, (u64, u64)>>>) -> App {
        App {
            messages,
            scroll_state: 0,
            running,
            block_height,
            block_hash: Arc::new(Mutex::new(String::new())),
            peer_list,
            focused_widget: FocusedWidget::Log,
            last_user_input_time: Instant::now(),
            auto_scroll_enabled: true,
            current_scroll_y: 0.0, // Initialize animated scroll position
            scroll_animation_speed: 0.1, // Initialize animation speed
            log_widget_height: 0,
            log_visible: true,
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
                let block_hash_value = self.block_hash.lock().unwrap();
                let block_height_widget = Paragraph::new("")
                    .block(Block::default().borders(Borders::ALL).title(format!("Block Height: {} Hash: {}", block_height_value, block_hash_value)).border_style(match self.focused_widget {
                        FocusedWidget::BlockHeight => Style::default().fg(Color::Magenta),
                        _ => Style::default().fg(Color::White),
                    }))
                    .style(Style::default().fg(Color::Cyan));
                f.render_widget(block_height_widget, chunks[0]);

                let instruction_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
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

                let bottom_half_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                    .split(chunks[2]);

                if self.log_visible {
                    let messages = self.messages.lock().unwrap();
                    let num_messages = messages.len();
                    let log_area_height = bottom_half_chunks[0].height;
                    let log_height = {
                        let content_height = if num_messages == 0 { 1 } else { num_messages };
                        let desired_height = (content_height + 2) as u16; // +2 for borders
                        std::cmp::min(desired_height, log_area_height)
                    };
                    self.log_widget_height = log_height;

                    let formatted_messages: Vec<Line> = messages
                        .iter()
                        .map(|(msg, timestamp)| {
                            let offset_datetime: OffsetDateTime = timestamp.clone().into();
                            let format = format_description!("[hour]:[minute]:[second]");
                            Line::from(Span::raw(format!("[{}] {}", offset_datetime.format(&format).unwrap_or_default(), msg)))
                        })
                        .collect();

                    let paragraph = Paragraph::new(formatted_messages)
                        .block(Block::default().borders(Borders::ALL).title("Bitcoin P2P Client Log").border_style(match self.focused_widget {
                            FocusedWidget::Log => Style::default().fg(Color::Magenta),
                            _ => Style::default().fg(Color::White),
                        }))
                        .style(Style::default().fg(Color::White))
                        .wrap(Wrap { trim: true })
                        .scroll((self.current_scroll_y.round() as u16, 0));

                    f.render_widget(paragraph, bottom_half_chunks[0]);
                } else { // Log is hidden, display Bitcoin logo
                    let logo_area = bottom_half_chunks[0]; // The area where the logo should be displayed

                    // Calculate vertical margins to center the logo
                    let vertical_margin_total = logo_area.height.saturating_sub(LOGO_HEIGHT);
                    let top_margin = vertical_margin_total / 2;
                    let bottom_margin = vertical_margin_total.saturating_sub(top_margin);

                    // Calculate horizontal margins to center the logo
                    let horizontal_margin_total = logo_area.width.saturating_sub(LOGO_WIDTH);
                    let left_margin = horizontal_margin_total / 2;
                    let right_margin = horizontal_margin_total.saturating_sub(left_margin);

                    // Create a vertical layout for centering
                    let centered_layout_vertical = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(top_margin),
                            Constraint::Length(LOGO_HEIGHT),
                            Constraint::Length(bottom_margin),
                        ])
                        .split(logo_area);

                    // Create a horizontal layout for centering within the vertical middle chunk
                    let centered_layout_horizontal = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([
                            Constraint::Length(left_margin),
                            Constraint::Length(LOGO_WIDTH),
                            Constraint::Length(right_margin),
                        ])
                        .split(centered_layout_vertical[1]); // Use the middle chunk from vertical split

                    let logo_lines: Vec<Line> = BITCOIN_LOGO.iter().map(|line| Line::from(Span::raw(*line))).collect();
                    let logo_widget = Paragraph::new(logo_lines)
                        .block(Block::default().borders(Borders::ALL).title("Bitcoin Logo").border_style(Style::default().fg(Color::Yellow))) // Neutral border style
                        .style(Style::default().fg(Color::Yellow)); // Neutral text style

                    // Render the logo widget in the center chunk
                    f.render_widget(logo_widget, centered_layout_horizontal[1]);
                }

                // GEMINI - each peer in the list should be selectable which reveals other traits
                // common to the bitcoin core peer list detail view
                let peer_list_content: Vec<Line> = self.peer_list.lock().unwrap().iter()
                    .map(|(peer_addr, bytes_transferred)| Line::from(Span::raw(format!("{} In: {} B, Out: {} B", peer_addr, bytes_transferred.0, bytes_transferred.1))))
                    .collect();

                let peer_list_widget = Paragraph::new(peer_list_content)
                    .block(Block::default().borders(Borders::ALL).title("Peers").border_style(match self.focused_widget {
                        FocusedWidget::PeerList => Style::default().fg(Color::Magenta),
                        _ => Style::default().fg(Color::Yellow),
                    }))
                    .style(Style::default().fg(Color::Yellow));
                f.render_widget(peer_list_widget, bottom_half_chunks[1]);
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
                                FocusedWidget::Log => FocusedWidget::PeerList,
                                FocusedWidget::PeerList => FocusedWidget::BlockHeight,
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
                            self.focused_widget = match self.focused_widget {
                                FocusedWidget::Instructions => FocusedWidget::BlockHeight,
                                FocusedWidget::PeerList => FocusedWidget::Log,
                                FocusedWidget::Log => FocusedWidget::Instructions,
                                _ => self.focused_widget,
                            };
                        },
                        KeyCode::Right => {
                            self.focused_widget = match self.focused_widget {
                                FocusedWidget::BlockHeight => FocusedWidget::Instructions,
                                FocusedWidget::Instructions => FocusedWidget::Log,
                                FocusedWidget::Log => FocusedWidget::PeerList,
                                _ => self.focused_widget,
                            };
                        },
                        KeyCode::Esc => {
                            if self.focused_widget == FocusedWidget::Log && !self.auto_scroll_enabled {
                                self.auto_scroll_enabled = true;
                                self.last_user_input_time = Instant::now(); // Reset timer to allow auto-scroll after delay
                            }
                        },
                        KeyCode::Char('l') => {
                            self.log_visible = !self.log_visible;
                            if !self.log_visible && self.focused_widget == FocusedWidget::Log {
                                self.focused_widget = FocusedWidget::BlockHeight;
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
                        _ => {{}},
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

                    // Auto-scroll logic should apply if auto_scroll_enabled is true, regardless of focus
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
                    }

                    // Manual scroll animation should only happen when the log is focused
                    if self.focused_widget == FocusedWidget::Log {
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
                    // --- End Scrolling Logic ---
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {{}},
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
