use crate::ui::{init_tui, restore_tui, App};
use gnostr_bitcoin::{connect_and_handshake, build_mempool_message, build_ping_message, build_pong_message, read_message, DNS_SEEDS, DEFAULT_PORT, init_logger};
use std::io::Write;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use anyhow::Result;
use ctrlc;
use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};

mod ui;

pub const MAX_PEERS: usize = 8;
const PEERS_FILE_NAME: &str = "peers.json";

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PeerInfo {
    addr: String,
    inbound_traffic: u64,
    outbound_traffic: u64,
    total_inbound_traffic: u64,
    total_outbound_traffic: u64,
}

fn get_app_data_dir() -> Result<PathBuf> {
    let mut path = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine application data directory"))?;
    path.push("gnostr-bitcoin");
    Ok(path)
}

fn save_peers(peers: &std::collections::HashMap<String, (u64, u64)>) -> Result<()> {
    let app_data_dir = get_app_data_dir()?;
    fs::create_dir_all(&app_data_dir)?;
    let peers_file_path = app_data_dir.join(PEERS_FILE_NAME);

    let serializable_peers: Vec<PeerInfo> = peers.iter().map(|(addr, (total_inbound, total_outbound))| {
        PeerInfo {
            addr: addr.clone(),
            inbound_traffic: 0, // This will be updated by the active connection
            outbound_traffic: 0, // This will be updated by the active connection
            total_inbound_traffic: *total_inbound,
            total_outbound_traffic: *total_outbound,
        }
    }).collect();

    let json = serde_json::to_string_pretty(&serializable_peers)?;
    fs::write(peers_file_path, json)?;
    log::info!("Saved {} peers.", peers.len());
    Ok(())
}

fn load_peers() -> Result<std::collections::HashMap<String, (u64, u64)>> {
    let app_data_dir = get_app_data_dir()?;
    let peers_file_path = app_data_dir.join(PEERS_FILE_NAME);

    if !peers_file_path.exists() {
        log::info!("No peers file found at {:?}. Starting with empty peer list.", peers_file_path);
        return Ok(std::collections::HashMap::new());
    }

    let json = fs::read_to_string(peers_file_path)?;
    let serializable_peers: Vec<PeerInfo> = serde_json::from_str(&json)?;
    let peers_map: std::collections::HashMap<String, (u64, u64)> = serializable_peers.into_iter().map(|p| {
        (p.addr, (p.total_inbound_traffic, p.total_outbound_traffic))
    }).collect();
    log::info!("Loaded {} peers.", peers_map.len());
    Ok(peers_map)
}

fn main() -> Result<()> {
    init_logger()?;

    let running = Arc::new(AtomicBool::new(true));
    let messages = Arc::new(Mutex::new(Vec::<(String, SystemTime)>::new()));
    let block_height = Arc::new(Mutex::new(0));
    let known_peers = Arc::new(Mutex::new(load_peers().unwrap_or_else(|e| {
        log::error!("Failed to load known peers on startup: {}", e);
        std::collections::HashMap::new()
    })));
    let active_peers = Arc::new(Mutex::new(std::collections::HashMap::<String, (u64, u64, SystemTime)>::new()));
    let discovered_peers_queue = Arc::new(Mutex::new(Vec::<String>::new()));

    // Populate discovered_peers_queue with known peers
    let mut discovered_peers_queue_lock = discovered_peers_queue.lock().unwrap();
    for (addr, _) in known_peers.lock().unwrap().iter() {
        discovered_peers_queue_lock.push(addr.clone());
    }
    log::info!("Added {} known peers to discovery queue.", discovered_peers_queue_lock.len());
    drop(discovered_peers_queue_lock); // Drop the lock before spawning threads

    // --- Start of new code for initial DNS seed discovery ---
    let (tx_initial_peers, rx_initial_peers) = std::sync::mpsc::channel();
    let mut handles = Vec::new();

    for &seed_addr_str in DNS_SEEDS.iter() {
        let seed_addr = seed_addr_str.to_string();
        let tx_clone = tx_initial_peers.clone();
        let block_height_clone = Arc::clone(&block_height);
        let running_clone = Arc::clone(&running);
        let messages_clone = Arc::clone(&messages);
        let _known_peers_clone = Arc::clone(&known_peers);

        let handle = std::thread::spawn(move || {
            let add_message = |msg: String| {
                messages_clone.lock().unwrap().push((msg, SystemTime::now()));
            };
            add_message(format!("Attempting initial connection to DNS seed: {}", seed_addr));
            let conn_result = connect_and_handshake(
                DNS_SEEDS,
                DEFAULT_PORT,
                block_height_clone,
                running_clone,
                Some(seed_addr.clone()),
            );
            if let Ok((_, _, new_peers)) = conn_result {
                add_message(format!("Discovered {} new peers from {}.", new_peers.len(), seed_addr));
                let _ = tx_clone.send(new_peers);
            } else if let Err(e) = conn_result {
                add_message(format!("[ERROR] Initial connection to {} failed: {}", seed_addr, e));
            }
        });
        handles.push(handle);
    }

    // Drop the original sender to signal that no more senders will exist
    drop(tx_initial_peers);

    // Collect results from initial peer discovery threads
    let mut initial_discovered_peers: Vec<String> = Vec::new();
    for new_peers_from_seed in rx_initial_peers.iter() {
        initial_discovered_peers.extend(new_peers_from_seed);
    }
    log::info!("Collected {} initial peers from DNS seeds.", initial_discovered_peers.len());

    // Add newly discovered peers to the main discovered_peers_queue and known_peers
    let mut discovered_peers_queue_lock = discovered_peers_queue.lock().unwrap();
    let mut known_peers_lock = known_peers.lock().unwrap();
    for peer in initial_discovered_peers {
        if !discovered_peers_queue_lock.contains(&peer) {
            discovered_peers_queue_lock.push(peer.clone());
        }
        if !known_peers_lock.contains_key(&peer) {
            known_peers_lock.insert(peer, (0, 0)); // Initialize with 0 total traffic
        }
    }
    log::info!("Total peers in discovery queue after initial DNS scan: {}.", discovered_peers_queue_lock.len());
    drop(discovered_peers_queue_lock); // Drop the lock before spawning threads
    drop(known_peers_lock); // Drop the lock before spawning threads
    // --- End of new code for initial DNS seed discovery ---

    // Clones for Ctrl-C handler
    let r_ctrlc = running.clone();
    let known_peers_for_shutdown_ctrlc = Arc::clone(&known_peers);

    ctrlc::set_handler(move || {
        log::info!("Ctrl-C received. Initiating shutdown...");
        r_ctrlc.store(false, Ordering::SeqCst);
        if let Err(e) = save_peers(&known_peers_for_shutdown_ctrlc.lock().unwrap()) {
            log::error!("Failed to save peers on shutdown: {}", e);
        }
    }).expect("Error setting Ctrl-C handler");

    // Clones for network thread
    let messages_network = Arc::clone(&messages);
    let running_network = Arc::clone(&running);
    let block_height_network = Arc::clone(&block_height);
    let active_peers_network = Arc::clone(&active_peers);
    let known_peers_network = Arc::clone(&known_peers);
    let discovered_peers_queue_network = Arc::clone(&discovered_peers_queue);

    // 2. Spawn a thread for network operations
    let _network_thread_handle = std::thread::spawn(move || {
        let add_message = |msg: String| {
            messages_network.lock().unwrap().push((msg, SystemTime::now()));
        };

        add_message("Starting Bitcoin P2P client...".to_string());

        loop { // Main loop for managing connections
            if !running_network.load(Ordering::SeqCst) {
                add_message("Network thread received shutdown signal.".to_string());
                // On shutdown, ensure known_peers is updated with the latest active_peers traffic
                let mut known_peers_lock = known_peers_network.lock().unwrap();
                let active_peers_lock = active_peers_network.lock().unwrap();
                for (addr, (in_traffic, out_traffic, _connection_time)) in active_peers_lock.iter() {
                    known_peers_lock.entry(addr.clone()).and_modify(|(total_in, total_out)| {
                        *total_in += in_traffic;
                        *total_out += out_traffic;
                    }).or_insert(( *in_traffic, *out_traffic));
                }
                break;
            }

            let num_connected_peers = active_peers_network.lock().unwrap().len();
            if num_connected_peers >= MAX_PEERS {
                std::thread::sleep(Duration::from_secs(5)); // Wait before checking again
                continue;
            }

            let mut target_peer_addr: Option<String> = None;
            // Prioritize connecting to discovered peers
            if let Some(peer) = discovered_peers_queue_network.lock().unwrap().pop() {
                target_peer_addr = Some(peer);
            }

            add_message(format!("Attempting to connect and handshake ({} / {} peers)...", num_connected_peers, MAX_PEERS));
            let (tx_conn, rx_conn) = std::sync::mpsc::channel();
            let block_height_clone_for_conn = Arc::clone(&block_height_network);
            let running_network_clone_for_conn = Arc::clone(&running_network);
            let active_peers_clone_for_conn = Arc::clone(&active_peers_network);
            let known_peers_clone_for_conn = Arc::clone(&known_peers_network);
            let messages_clone_for_logging = Arc::clone(&messages_network);
            let discovered_peers_queue_for_conn = Arc::clone(&discovered_peers_queue_network);
            let target_peer_addr_for_thread = target_peer_addr.clone();

            std::thread::spawn(move || {
                let conn_result: Result<(TcpStream, String, Vec<String>), anyhow::Error> = connect_and_handshake(
                    DNS_SEEDS,
                    DEFAULT_PORT,
                    block_height_clone_for_conn,
                    running_network_clone_for_conn,
                    target_peer_addr_for_thread,
                );
                if let Ok((_, peer_addr, new_peers)) = &conn_result {
                    let mut active_peers_lock = active_peers_clone_for_conn.lock().unwrap();
                    active_peers_lock.insert(peer_addr.clone(), (0, 0, SystemTime::now())); // Initialize session traffic to 0 and set connection time

                    let mut known_peers_lock = known_peers_clone_for_conn.lock().unwrap();
                    if !known_peers_lock.contains_key(peer_addr) {
                        known_peers_lock.insert(peer_addr.clone(), (0, 0)); // Add to known_peers if new
                    }

                    messages_clone_for_logging.lock().unwrap().push((format!("Connected to: {}", peer_addr), SystemTime::now()));
                    let mut discovered_peers_queue_lock = discovered_peers_queue_for_conn.lock().unwrap();
                    for new_peer in new_peers.iter() {
                        if !discovered_peers_queue_lock.contains(new_peer) {
                            discovered_peers_queue_lock.push(new_peer.clone());
                        }
                        if !known_peers_lock.contains_key(new_peer) {
                            known_peers_lock.insert(new_peer.clone(), (0, 0)); // Add to known_peers if new
                        }
                    }
                }
                let _ = tx_conn.send(conn_result);
            });

            let stream_result: Result<(TcpStream, String, Vec<String>), anyhow::Error> = match rx_conn.recv_timeout(Duration::from_secs(10)) {
                Ok(Ok((stream, peer_addr, new_peers))) => {
                    Ok((stream, peer_addr, new_peers))
                },
                Ok(Err(e)) => {
                    add_message(format!("[ERROR] Failed to connect and handshake: {}. Trying next peer...", e));
                    // Re-add failed peer to discovered_peers_queue for retry with backoff
                    if let Some(failed_peer) = target_peer_addr {
                        let mut discovered_peers_queue_lock = discovered_peers_queue_network.lock().unwrap();
                        // Only re-add if not already in queue to avoid duplicates
                        if !discovered_peers_queue_lock.contains(&failed_peer) {
                            discovered_peers_queue_lock.push(failed_peer.clone());
                            add_message(format!("[INFO] Re-added {} to discovery queue for retry.", failed_peer));
                        }
                    }
                    Err(e)
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    add_message("[WARN] Connection and handshake timed out after 10 seconds. Trying next peer...".to_string());
                    // Re-add timed out peer to discovered_peers_queue for retry with backoff
                    if let Some(timed_out_peer) = target_peer_addr {
                        let mut discovered_peers_queue_lock = discovered_peers_queue_network.lock().unwrap();
                        if !discovered_peers_queue_lock.contains(&timed_out_peer) {
                            discovered_peers_queue_lock.push(timed_out_peer.clone());
                            add_message(format!("[INFO] Re-added {} to discovery queue for retry (timeout).", timed_out_peer));
                        }
                    }
                    Err(anyhow::anyhow!("Connection timeout"))
                },
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    add_message("[ERROR] Connection thread disconnected before sending result. Trying next peer...".to_string());
                    Err(anyhow::anyhow!("Connection thread disconnected"))
                },
            };

            if let Ok((mut stream, connected_peer_addr, _)) = stream_result {
                let active_peers_clone_for_peer_thread = Arc::clone(&active_peers_network);
                let known_peers_clone_for_peer_thread = Arc::clone(&known_peers_network);
                let messages_clone_for_peer_thread = Arc::clone(&messages_network);
                let running_network_clone_for_peer_thread = Arc::clone(&running_network);
                let peer_addr_for_peer_thread = connected_peer_addr.clone();

                std::thread::spawn(move || {
                    let add_message_for_peer = |msg: String| {
                        messages_clone_for_peer_thread.lock().unwrap().push((format!("[{}] {}", peer_addr_for_peer_thread, msg), SystemTime::now()));
                    };

                    let (mut session_inbound_traffic, mut session_outbound_traffic) = (0, 0);
                    let mut last_traffic_update = Instant::now();

                    add_message_for_peer("Entering message processing loop...".to_string());

                    // Request mempool
                    add_message_for_peer("Requesting mempool information...".to_string());
                    match build_mempool_message() {
                        Ok(mempool_message) => {
                            if let Err(e) = stream.write_all(&mempool_message) {
                                add_message_for_peer(format!("[ERROR] Failed to send mempool request: {}", e));
                            } else {
                                session_outbound_traffic += mempool_message.len() as u64;
                                add_message_for_peer("Sent 'mempool' request.".to_string());
                            }
                        },
                        Err(e) => {
                            add_message_for_peer(format!("[ERROR] Failed to build mempool message: {}", e));
                        }
                    }

                    loop {
                        if !running_network_clone_for_peer_thread.load(Ordering::SeqCst) {
                            add_message_for_peer("Peer thread received shutdown signal.".to_string());
                            break; // Exit inner loop
                        }

                        // Periodically update session traffic in active_peers
                        if last_traffic_update.elapsed() >= Duration::from_secs(1) {
                            let mut active_peers_lock = active_peers_clone_for_peer_thread.lock().unwrap();
                            if let Some(peer_entry) = active_peers_lock.get_mut(&peer_addr_for_peer_thread) {
                                peer_entry.0 = session_inbound_traffic; // Update session inbound traffic
                                peer_entry.1 = session_outbound_traffic; // Update session outbound traffic
                                // peer_entry.2 (SystemTime) remains unchanged
                            }
                            last_traffic_update = Instant::now();
                        }
                        if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(60))) {
                            add_message_for_peer(format!("[ERROR] Failed to set read timeout: {}", e));
                            break; // Exit inner loop
                        }

                        match read_message(&mut stream) {
                            Ok((header, payload)) => {
                                session_inbound_traffic += (header.len() + payload.len()) as u64;
                                let command_result = std::str::from_utf8(&header[4..16]);
                                match command_result {
                                    Ok(command) => {
                                        let command = command.trim_end_matches('\0');
                                        let log_msg = format!("[RECEIVED] Command: '{}', Payload Size: {} bytes", command, payload.len());
                                        add_message_for_peer(log_msg);

                                        match command {
                                            "version" => {
                                                add_message_for_peer("[INFO] Received 'version' message again.".to_string());
                                            }
                                            "verack" => {
                                                add_message_for_peer("[INFO] Received 'verack' message.".to_string());
                                            }
                                            "ping" => {
                                                add_message_for_peer("[INFO] Received 'ping' message. Sending 'pong'.".to_string());
                                                if let Ok(nonce) = payload.try_into().map_err(|_| "Invalid ping nonce size") {
                                                    match build_pong_message(nonce) {
                                                        Ok(pong_message) => {
                                                                                                                if let Err(e) = stream.write_all(&pong_message) {
                                                                                                                    add_message_for_peer(format!("[ERROR] Failed to send pong: {}", e));
                                                                                                                } else {
                                                                                                                    session_outbound_traffic += pong_message.len() as u64;
                                                                                                                    add_message_for_peer("[SENT] 'pong' message.".to_string());
                                                                                                                }                                                        },
                                                        Err(e) => add_message_for_peer(format!("[ERROR] Failed to build pong message: {}", e)),
                                                    }
                                                } else {
                                                    add_message_for_peer("[ERROR] Invalid ping nonce size.".to_string());
                                                }
                                            }
                                            "pong" => {
                                                add_message_for_peer("[INFO] Received 'pong' message.".to_string());
                                            }
                                            "mempool" => {
                                                add_message_for_peer("[INFO] Received 'mempool' response (or another mempool request).".to_string());
                                            }
                                            "inv" => {
                                                add_message_for_peer("[INFO] Received 'inv' message (inventory).".to_string());
                                            }
                                            "tx" => {
                                                add_message_for_peer("[INFO] Received 'tx' message (transaction).".to_string());
                                            }
                                            "block" => {
                                                add_message_for_peer("[INFO] Received 'block' message.".to_string());
                                            }
                                            "headers" => {
                                                add_message_for_peer("[INFO] Received 'headers' message.".to_string());
                                            }
                                            "getheaders" => {
                                                add_message_for_peer("[INFO] Received 'getheaders' message.".to_string());
                                            }
                                            "getdata" => {
                                                add_message_for_peer("[INFO] Received 'getdata' message.".to_string());
                                            }
                                            "addr" => {
                                                add_message_for_peer("[INFO] Received 'addr' message.".to_string());
                                            }
                                            _ => {
                                                add_message_for_peer(format!("[INFO] Received unhandled command: '{}'", command));
                                            }
                                        }
                                    },
                                    Err(e) => {
                                        add_message_for_peer(format!("[ERROR] Failed to parse command from header: {}", e));
                                        break; // Exit loop on command parsing error
                                    }
                                }
                            }
                            Err(e) => {
                                if let Some(io_error) = e.downcast_ref::<std::io::Error>() {
                                    if io_error.kind() == std::io::ErrorKind::TimedOut {
                                        add_message_for_peer("[INFO] Read timeout. No data received for 60 seconds. Sending ping...".to_string());
                                        let nonce_u64 = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                                        let mut nonce_bytes = [0u8; 8];
                                        nonce_bytes.copy_from_slice(&nonce_u64.to_le_bytes());
                                        match build_ping_message(nonce_bytes) {
                                            Ok(ping_message) => {
                                                                                            if let Err(e) = stream.write_all(&ping_message) {
                                                                                                add_message_for_peer(format!("[ERROR] Failed to send ping: {}", e));
                                                                                            } else {
                                                                                                session_outbound_traffic += ping_message.len() as u64;
                                                                                                add_message_for_peer("[SENT] 'ping' message with nonce.".to_string());
                                                                                            }                                            },
                                            Err(e) => add_message_for_peer(format!("[ERROR] Failed to build ping message: {}", e)),
                                        }
                                        continue; // Continue the loop to wait for pong
                                    }
                                }
                                add_message_for_peer(format!("[ERROR] Failed to read message: {}", e));
                                break; // Exit loop on other read errors
                            }
                        }
                    }
                    add_message_for_peer(format!("Disconnected from {}. Session In: {} B, Session Out: {} B", peer_addr_for_peer_thread, session_inbound_traffic, session_outbound_traffic));
                    // Update known_peers with cumulative traffic and remove from active_peers
                    let mut known_peers_lock = known_peers_clone_for_peer_thread.lock().unwrap();
                    known_peers_lock.entry(peer_addr_for_peer_thread.clone()).and_modify(|(total_in, total_out)| {
                        *total_in += session_inbound_traffic;
                        *total_out += session_outbound_traffic;
                    }).or_insert((session_inbound_traffic, session_outbound_traffic));

                    let mut active_peers_lock = active_peers_clone_for_peer_thread.lock().unwrap();
                    active_peers_lock.remove(&peer_addr_for_peer_thread);
                });
            } else {
                std::thread::sleep(Duration::from_secs(2)); // Wait a bit before retrying connection attempt
            }
        }
        add_message("Network thread finished. Press 'q' to exit TUI.".to_string());
    });

    // 3. Initialize TUI
    let mut terminal = init_tui()?;

    // 4. Create App instance
    let mut app = App::new(Arc::clone(&messages), Arc::clone(&running), Arc::clone(&block_height), Arc::clone(&active_peers));

    // 5. Run the TUI application loop
    // The `App::run` method will draw messages from `app.messages` and handle user input.
    // It uses its own internal `rx` channel for input events and ticks.
    // If the network thread panics or finishes, its `messages_clone` sender is dropped.
    // This will cause `tui_rx.recv()` in `App::run` to eventually error out, causing `App::run` to return an error,
    // which will trigger `restore_tui`.
    app.run(&mut terminal)?;

    // 6. Restore TUI
    restore_tui(&mut terminal)?;

    Ok(())
}
