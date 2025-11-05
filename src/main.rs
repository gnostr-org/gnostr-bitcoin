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

    let serializable_peers: Vec<PeerInfo> = peers.iter().map(|(addr, (inbound, outbound))| {
        PeerInfo {
            addr: addr.clone(),
            inbound_traffic: *inbound,
            outbound_traffic: *outbound,
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
        (p.addr, (p.inbound_traffic, p.outbound_traffic))
    }).collect();
    log::info!("Loaded {} peers.", peers_map.len());
    Ok(peers_map)
}

fn main() -> Result<()> {
    init_logger()?;

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let peer_list_for_shutdown = Arc::new(Mutex::new(std::collections::HashMap::<String, (u64, u64)>::new()));
    let peer_list_for_shutdown_clone = Arc::clone(&peer_list_for_shutdown);

    ctrlc::set_handler(move || {
        log::info!("Ctrl-C received. Initiating shutdown...");
        r.store(false, Ordering::SeqCst);
        // Attempt to save peers on shutdown
        if let Err(e) = save_peers(&peer_list_for_shutdown_clone.lock().unwrap()) {
            log::error!("Failed to save peers on shutdown: {}", e);
        }
    }).expect("Error setting Ctrl-C handler");

    // 1. Setup shared state for messages and block height
    let messages = Arc::new(Mutex::new(Vec::<(String, SystemTime)>::new()));
    let block_height = Arc::new(Mutex::new(0));
    let peer_list = Arc::new(Mutex::new(std::collections::HashMap::<String, (u64, u64)>::new()));
    let discovered_peers_queue = Arc::new(Mutex::new(Vec::<String>::new()));

    // Load peers from file on startup
    match load_peers() {
        Ok(loaded_peers) => {
            let mut discovered_peers_queue_lock = discovered_peers_queue.lock().unwrap();
            for (addr, _) in loaded_peers.iter() {
                discovered_peers_queue_lock.push(addr.clone());
            }
            log::info!("Added {} loaded peers to discovery queue.", loaded_peers.len());
        },
        Err(e) => {
            log::error!("Failed to load peers on startup: {}", e);
        }
    }

    // Clone messages for the network thread
    let messages_clone = Arc::clone(&messages);
    let running_network_clone = Arc::clone(&running);
    let block_height_clone = Arc::clone(&block_height);
    let peer_list_clone = Arc::clone(&peer_list);
    let discovered_peers_queue_clone = Arc::clone(&discovered_peers_queue);
    let peer_list_for_shutdown_network_clone = Arc::clone(&peer_list_for_shutdown);

    // 2. Spawn a thread for network operations
    let _network_thread_handle = std::thread::spawn(move || {
        let add_message = |msg: String| {
            messages_clone.lock().unwrap().push((msg, SystemTime::now()));
        };

        add_message("Starting Bitcoin P2P client...".to_string());

        loop { // Main loop for managing connections
            if !running_network_clone.load(Ordering::SeqCst) {
                add_message("Network thread received shutdown signal.".to_string());
                // Before breaking, ensure the peer_list_for_shutdown is updated with the latest peer_list
                let current_peer_list = peer_list_clone.lock().unwrap();
                *peer_list_for_shutdown_network_clone.lock().unwrap() = current_peer_list.clone();
                break;
            }

            let num_connected_peers = peer_list_clone.lock().unwrap().len();
            if num_connected_peers >= MAX_PEERS {
                std::thread::sleep(Duration::from_secs(5)); // Wait before checking again
                continue;
            }

            let mut target_peer_addr: Option<String> = None;
            // Prioritize connecting to discovered peers
            if let Some(peer) = discovered_peers_queue_clone.lock().unwrap().pop() {
                target_peer_addr = Some(peer);
            }

            add_message(format!("Attempting to connect and handshake ({} / {} peers)...", num_connected_peers, MAX_PEERS));
            let (tx_conn, rx_conn) = std::sync::mpsc::channel();
            let block_height_clone_for_conn = Arc::clone(&block_height_clone);
            let running_network_clone_for_conn = Arc::clone(&running_network_clone);
            let peer_list_clone_for_conn = Arc::clone(&peer_list_clone);
            let messages_clone_for_logging = Arc::clone(&messages_clone);
            let discovered_peers_queue_for_conn = Arc::clone(&discovered_peers_queue_clone);

            std::thread::spawn(move || {
                let conn_result: Result<(TcpStream, String, Vec<String>), anyhow::Error> = connect_and_handshake(
                    DNS_SEEDS,
                    DEFAULT_PORT,
                    block_height_clone_for_conn,
                    running_network_clone_for_conn,
                    target_peer_addr,
                );
                if let Ok((_, peer_addr, new_peers)) = &conn_result {
                    peer_list_clone_for_conn.lock().unwrap().insert(peer_addr.clone(), (0, 0));
                    messages_clone_for_logging.lock().unwrap().push((format!("Connected to: {}", peer_addr), SystemTime::now()));
                    discovered_peers_queue_for_conn.lock().unwrap().extend(new_peers.clone());
                }
                let _ = tx_conn.send(conn_result);
            });

            let stream_result: Result<(TcpStream, String, Vec<String>), anyhow::Error> = match rx_conn.recv_timeout(Duration::from_secs(10)) {
                Ok(Ok((stream, peer_addr, new_peers))) => {
                    Ok((stream, peer_addr, new_peers))
                },
                Ok(Err(e)) => {
                    add_message(format!("[ERROR] Failed to connect and handshake: {}. Trying next peer...", e));
                    Err(e)
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    add_message("[WARN] Connection and handshake timed out after 10 seconds. Trying next peer...".to_string());
                    Err(anyhow::anyhow!("Connection timeout"))
                },
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    add_message("[ERROR] Connection thread disconnected before sending result. Trying next peer...".to_string());
                    Err(anyhow::anyhow!("Connection thread disconnected"))
                },
            };

            if let Ok((mut stream, connected_peer_addr, _)) = stream_result {
                let peer_list_clone_for_peer_thread = Arc::clone(&peer_list_clone);
                let messages_clone_for_peer_thread = Arc::clone(&messages_clone);
                let running_network_clone_for_peer_thread = Arc::clone(&running_network_clone);
                let peer_addr_for_peer_thread = connected_peer_addr.clone();

                std::thread::spawn(move || {
                    let add_message_for_peer = |msg: String| {
                        messages_clone_for_peer_thread.lock().unwrap().push((format!("[{}] {}", peer_addr_for_peer_thread, msg), SystemTime::now()));
                    };

                    let mut inbound_traffic: u64 = 0;
                    let mut outbound_traffic: u64 = 0;
                    let mut last_traffic_update = Instant::now();

                    add_message_for_peer("Entering message processing loop...".to_string());

                    // Request mempool
                    add_message_for_peer("Requesting mempool information...".to_string());
                    match build_mempool_message() {
                        Ok(mempool_message) => {
                            if let Err(e) = stream.write_all(&mempool_message) {
                                add_message_for_peer(format!("[ERROR] Failed to send mempool request: {}", e));
                            } else {
                                outbound_traffic += mempool_message.len() as u64;
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

                        // Periodically update traffic in the shared peer_list
                                        if last_traffic_update.elapsed() >= Duration::from_secs(1) {
                                            let mut peer_list_lock = peer_list_clone_for_peer_thread.lock().unwrap();
                                            if let Some(peer_entry) = peer_list_lock.iter_mut().find(|(addr, _traffic_tuple)| {
                                                *addr == &peer_addr_for_peer_thread
                                            }) {
                                                peer_entry.1.0 = inbound_traffic; // Accessing the first u64 in the tuple
                                                peer_entry.1.1 = outbound_traffic; // Accessing the second u64 in the tuple
                                            }
                                            last_traffic_update = Instant::now();
                                        }
                        if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(60))) {
                            add_message_for_peer(format!("[ERROR] Failed to set read timeout: {}", e));
                            break; // Exit inner loop
                        }

                        match read_message(&mut stream) {
                            Ok((header, payload)) => {
                                inbound_traffic += (header.len() + payload.len()) as u64;
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
                                                                                                                    outbound_traffic += pong_message.len() as u64;
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
                                                                                                outbound_traffic += ping_message.len() as u64;
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
                    add_message_for_peer(format!("Disconnected from {}. Total In: {} B, Total Out: {} B", peer_addr_for_peer_thread, inbound_traffic, outbound_traffic));
                    // Update traffic before removing the peer
                    if let Some(peer_entry) = peer_list_clone_for_peer_thread.lock().unwrap().get_mut(&peer_addr_for_peer_thread) {
                        peer_entry.0 = inbound_traffic;
                        peer_entry.1 = outbound_traffic;
                    }
                    peer_list_clone_for_peer_thread.lock().unwrap().remove(&peer_addr_for_peer_thread);
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
    let mut app = App::new(Arc::clone(&messages), Arc::clone(&running), Arc::clone(&block_height), Arc::clone(&peer_list));

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
