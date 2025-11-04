use gnostr_bitcoin::ui::{init_tui, restore_tui, App};
use gnostr_bitcoin::{connect_and_handshake, build_mempool_message, build_ping_message, build_pong_message, read_message, DNS_SEEDS, DEFAULT_PORT};
use std::io::Write;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use anyhow::Result;

fn main() -> Result<()> {
    // 1. Setup shared state for messages
    let messages = Arc::new(Mutex::new(Vec::new()));

    // Clone messages for the network thread
    let messages_clone = Arc::clone(&messages);

    // 2. Spawn a thread for network operations
    let _network_thread_handle = std::thread::spawn(move || {
        let mut stream_result: Result<TcpStream, Box<dyn std::error::Error>> = Err("Not connected".into());

        let add_message = |msg: String| {
            messages_clone.lock().unwrap().push(msg);
        };

        add_message("Starting Bitcoin P2P client...".to_string());

        match connect_and_handshake(DNS_SEEDS, DEFAULT_PORT) {
            Ok(mut stream) => {
                add_message("Successfully connected and handshaked.".to_string());

                // Request mempool
                add_message("Requesting mempool information...".to_string());
                match build_mempool_message() {
                    Ok(mempool_message) => {
                        if let Err(e) = stream.write_all(&mempool_message) {
                            add_message(format!("[ERROR] Failed to send mempool request: {}", e));
                        } else {
                            add_message("Sent 'mempool' request.".to_string());
                        }
                    },
                    Err(e) => {
                        add_message(format!("[ERROR] Failed to build mempool message: {}", e));
                    }
                }
                stream_result = Ok(stream);

                // --- Node Life Cycle Loop ---
                add_message("Entering message processing loop...".to_string());
                let mut current_stream = stream_result.as_mut().unwrap();
                loop {
                    if let Err(e) = current_stream.set_read_timeout(Some(Duration::from_secs(60))) {
                        add_message(format!("[ERROR] Failed to set read timeout: {}", e));
                        break;
                    }

                    match read_message(&mut current_stream) {
                        Ok((header, payload)) => {
                            let command_result = std::str::from_utf8(&header[4..16]);
                            match command_result {
                                Ok(command) => {
                                    let command = command.trim_end_matches('\0');
                                    let log_msg = format!("[RECEIVED] Command: '{}', Payload Size: {} bytes", command, payload.len());
                                    add_message(log_msg);

                                    match command {
                                        "version" => {
                                            add_message("[INFO] Received 'version' message again.".to_string());
                                        }
                                        "verack" => {
                                            add_message("[INFO] Received 'verack' message.".to_string());
                                        }
                                        "ping" => {
                                            add_message("[INFO] Received 'ping' message. Sending 'pong'.".to_string());
                                            if let Ok(nonce) = payload.try_into().map_err(|_| "Invalid ping nonce size") {
                                                match build_pong_message(nonce) {
                                                    Ok(pong_message) => {
                                                        if let Err(e) = current_stream.write_all(&pong_message) {
                                                            add_message(format!("[ERROR] Failed to send pong: {}", e));
                                                        } else {
                                                            add_message("[SENT] 'pong' message.".to_string());
                                                        }
                                                    },
                                                    Err(e) => add_message(format!("[ERROR] Failed to build pong message: {}", e)),
                                                }
                                            } else {
                                                add_message("[ERROR] Invalid ping nonce size.".to_string());
                                            }
                                        }
                                        "pong" => {
                                            add_message("[INFO] Received 'pong' message.".to_string());
                                        }
                                        "mempool" => {
                                            add_message("[INFO] Received 'mempool' response (or another mempool request).".to_string());
                                        }
                                        "inv" => {
                                            add_message("[INFO] Received 'inv' message (inventory).".to_string());
                                        }
                                        "tx" => {
                                            add_message("[INFO] Received 'tx' message (transaction).".to_string());
                                        }
                                        "block" => {
                                            add_message("[INFO] Received 'block' message.".to_string());
                                        }
                                        "headers" => {
                                            add_message("[INFO] Received 'headers' message.".to_string());
                                        }
                                        "getheaders" => {
                                            add_message("[INFO] Received 'getheaders' message.".to_string());
                                        }
                                        "getdata" => {
                                            add_message("[INFO] Received 'getdata' message.".to_string());
                                        }
                                        "addr" => {
                                            add_message("[INFO] Received 'addr' message.".to_string());
                                        }
                                        _ => {
                                            add_message(format!("[INFO] Received unhandled command: '{}'", command));
                                        }
                                    }
                                },
                                Err(e) => {
                                    add_message(format!("[ERROR] Failed to parse command from header: {}", e));
                                    break; // Exit loop on command parsing error
                                }
                            }
                        }
                        Err(e) => {
                            if let Some(io_error) = e.downcast_ref::<std::io::Error>() {
                                if io_error.kind() == std::io::ErrorKind::TimedOut {
                                    add_message("[INFO] Read timeout. No data received for 60 seconds. Sending ping...".to_string());
                                    let nonce_u64 = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                                    let mut nonce_bytes = [0u8; 8];
                                    nonce_bytes.copy_from_slice(&nonce_u64.to_le_bytes());
                                    match build_ping_message(nonce_bytes) {
                                        Ok(ping_message) => {
                                            if let Err(e) = current_stream.write_all(&ping_message) {
                                                add_message(format!("[ERROR] Failed to send ping: {}", e));
                                            } else {
                                                add_message("[SENT] 'ping' message with nonce.".to_string());
                                            }
                                        },
                                        Err(e) => add_message(format!("[ERROR] Failed to build ping message: {}", e)),
                                    }
                                    continue; // Continue the loop to wait for pong
                                }
                            }
                            add_message(format!("[ERROR] Failed to read message: {}", e));
                            break; // Exit loop on other read errors
                        }
                    }
                }
                add_message("Exiting node life cycle loop.".to_string());
            }
            Err(e) => {
                add_message(format!("[ERROR] Failed to connect and handshake: {}", e));
            }
        }
        // Ensure stream_result is handled if it was an error
        if let Err(e) = stream_result {
             add_message(format!("[FATAL] Network thread error: {}", e));
        }

        // Signal TUI to exit by dropping the sender.
        // This will cause `tui_rx.recv()` in `App::run` to error out.
        add_message("Network thread finished. Press 'q' to exit TUI.".to_string());
    });

    // 3. Initialize TUI
    let mut terminal = init_tui()?;

    // 4. Create App instance
    let mut app = App::new(Arc::clone(&messages));

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
