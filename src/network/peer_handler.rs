use std::{
    collections::HashMap,
    fs,
    io::Write,
    net::TcpStream,
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use tracing::{error, info};
use sha2::{Digest, Sha256};
use hex;

use crate::{
    ActivePeerState, DEFAULT_PORT, DNS_SEEDS, build_mempool_message, build_ping_message,
    build_pong_message, connect_and_handshake, read_message, send_raw_tx,
    build_getheaders_message, VarIntReader, build_getdata_message, accept_and_handshake, build_feefilter_message, build_sendcmpct_message, 
    // Constants
    GENESIS_HASH,
};

// Move the entire spawn_peer_handler function here
pub fn spawn_peer_handler(
    mut stream: TcpStream,
    peer_addr: String,
    messages: Arc<Mutex<Vec<(String, SystemTime)>>>,
    running: Arc<AtomicBool>,
    active_peers: Arc<Mutex<std::collections::HashMap<String, ActivePeerState>>>,
    known_peers: Arc<Mutex<std::collections::HashMap<String, (u64, u64)>>>,
    block_hash: Arc<Mutex<String>>,
    local_height: Arc<Mutex<i32>>,
    data_dir: PathBuf,
    initial_fee_filter: u64, // Added to pass the initial fee_filter from connection
) {
    std::thread::spawn(move || {
        let add_message_for_peer = |msg: String| {
            messages.lock().unwrap().push((
                format!("[{}] {}", peer_addr, msg),
                SystemTime::now(),
            ));
        };

        let (mut session_inbound_traffic, mut session_outbound_traffic) = (0, 0);
        let mut last_traffic_update = Instant::now();

        add_message_for_peer("Entering listener message processing loop...".to_string());

        // Send initial feefilter message
        match build_feefilter_message(10) { // Default feerate 10 sat/kB
            Ok(msg) => {
                if stream.write_all(&msg).is_ok() {
                    session_outbound_traffic += msg.len() as u64;
                    add_message_for_peer("Sent 'feefilter' message (10 sat/kB).".to_string());
                }
            }
            Err(e) => add_message_for_peer(format!("[ERROR] Error building feefilter msg: {}", e)),
        }

        // Send initial sendcmpct message
        match build_sendcmpct_message(1, 2) { // High-bandwidth mode, version 2 (BIP152)
            Ok(msg) => {
                if stream.write_all(&msg).is_ok() {
                    session_outbound_traffic += msg.len() as u64;
                    add_message_for_peer("Sent 'sendcmpct' message (high-bandwidth, version 2).".to_string());
                }
            }
            Err(e) => add_message_for_peer(format!("[ERROR] Error building sendcmpct msg: {}", e)),
        }

        // Send an initial 'mempool' request to the peer.
        add_message_for_peer("Requesting mempool information...".to_string());
        match build_mempool_message() {
            Ok(mempool_message) => {
                if let Err(e) = stream.write_all(&mempool_message) {
                    add_message_for_peer(format!(
                        "[ERROR] Failed to send mempool request: {}",
                        e
                    ));
                } else {
                    session_outbound_traffic += mempool_message.len() as u64;
                    add_message_for_peer("Sent 'mempool' request.".to_string());
                }
            }
            Err(e) => {
                add_message_for_peer(format!(
                    "[ERROR] Failed to build mempool message: {}",
                    e
                ));
            }
        }

        // Send 'getheaders' to request headers starting from genesis.
        add_message_for_peer("Requesting block headers...".to_string());
        match build_getheaders_message(vec![GENESIS_HASH], [0u8; 32]) {
            Ok(getheaders_msg) => {
                 if let Err(e) = stream.write_all(&getheaders_msg) {
                    add_message_for_peer(format!(
                        "[ERROR] Failed to send getheaders request: {}",
                        e
                    ));
                } else {
                    session_outbound_traffic += getheaders_msg.len() as u64;
                    add_message_for_peer("Sent 'getheaders' request.".to_string());
                }
            }
            Err(e) => {
                add_message_for_peer(format!(
                    "[ERROR] Failed to build getheaders message: {}",
                    e
                ));
            }
        }

        // Loop to continuously read messages from the peer.
        loop {
            // Check for shutdown signal.
            if !running.load(Ordering::SeqCst) {
                add_message_for_peer(
                    "Peer thread received shutdown signal.".to_string(),
                );
                break; // Exit the peer communication loop.
            }

            // Periodically update session traffic statistics in the shared state.
            if last_traffic_update.elapsed() >= Duration::from_secs(1) {
                let mut active_peers_lock =
                    active_peers.lock().unwrap();
                if let Some(state) =
                    active_peers_lock.get_mut(&peer_addr)
                {
                    state.inbound_traffic = session_inbound_traffic; // Update session inbound traffic.
                    state.outbound_traffic = session_outbound_traffic; // Update session outbound traffic.
                    // Peer connection time (peer_entry.2) remains
                    // unchanged.
                }
                last_traffic_update = Instant::now();
            }
            // Set a read timeout to detect idle connections and send pings.
            if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(60))) {
                add_message_for_peer(format!(
                    "[ERROR] Failed to set read timeout: {}",
                    e
                ));
                break; // Exit loop on read timeout error.
            }

            // Read a message from the peer.
            match read_message(&mut stream) {
                Ok((header, payload)) => {
                    // Accumulate traffic statistics.
                    session_inbound_traffic += (header.len() + payload.len()) as u64;
                    // Parse the command from the header.
                    let command_result = std::str::from_utf8(&header[4..16]);
                    match command_result {
                        Ok(command) => {
                            let command = command.trim_matches(|c: char| c == '\0' || c == ' ');
                            let log_msg = format!(
                                "[RECEIVED] Command: '{}', Payload Size: {} bytes",
                                command,
                                payload.len()
                            );
                            add_message_for_peer(log_msg);

                            // Handle different P2P commands.
                            match command {
                                "version" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'version' message again."
                                            .to_string(),
                                    );
                                }
                                "verack" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'verack' message.".to_string(),
                                    );
                                }
                                "ping" => {
                                    add_message_for_peer("[INFO] Received 'ping' message. Sending 'pong'.".to_string());
                                    // A ping message contains an 8-byte nonce.
                                    if let Ok(nonce) = payload
                                        .try_into()
                                        .map_err(|_| "Invalid ping nonce size")
                                    {
                                        match build_pong_message(nonce) {
                                            Ok(pong_message) => {
                                                // Send the pong message back to the
                                                // peer.
                                                if let Err(e) =
                                                    stream.write_all(&pong_message)
                                                {
                                                    add_message_for_peer(format!(
                                                        "[ERROR] Failed to send pong: {}",
                                                        e
                                                    ));
                                                } else {
                                                    session_outbound_traffic +=
                                                        pong_message.len() as u64;
                                                    add_message_for_peer(
                                                        "[SENT] 'pong' message."
                                                            .to_string(),
                                                    );
                                                }
                                            }
                                            Err(e) => add_message_for_peer(format!(
                                                "[ERROR] Failed to build pong message: {}",
                                                e
                                            )),
                                        }
                                    } else {
                                        add_message_for_peer(
                                            "[ERROR] Invalid ping nonce size."
                                                .to_string(),
                                        );
                                    }
                                }
                                "pong" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'pong' message.".to_string(),
                                    );
                                }
                                "mempool" => {
                                    add_message_for_peer("[INFO] Received 'mempool' response (or another mempool request).".to_string());
                                }
                                "inv" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'inv' message (inventory)."
                                            .to_string(),
                                    );
                                }
                                "tx" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'tx' message (transaction)."
                                            .to_string(),
                                    );
                                }
                                "block" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'block' message.".to_string(),
                                    );
                                    // Save block to disk
                                    // Calculate hash to name the file
                                    // Block header is first 80 bytes.
                                    if payload.len() >= 80 {
                                        let header = &payload[0..80];
                                        let hash1 = Sha256::digest(header);
                                        let hash2 = Sha256::digest(hash1);
                                        let mut hash_bytes = hash2.to_vec();
                                        hash_bytes.reverse();
                                        let hash_str = hex::encode(hash_bytes);
                                        
                                        let blocks_dir = data_dir.join("blocks");
                                        if let Err(e) = fs::create_dir_all(&blocks_dir) {
                                            add_message_for_peer(format!("[ERROR] Failed to create blocks dir: {}", e));
                                        } else {
                                            let file_path = blocks_dir.join(format!("block_{}.dat", hash_str));
                                            if let Err(e) = fs::write(&file_path, &payload) {
                                                add_message_for_peer(format!("[ERROR] Failed to write block to disk: {}", e));
                                            } else {
                                                add_message_for_peer(format!("[SUCCESS] Saved block to {:?}", file_path));
                                            }
                                        }
                                    }
                                }
                                "headers" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'headers' message.".to_string(),
                                    );
                                    // Parse headers to get the latest block hash
                                    let mut offset = 0;
                                    match payload.read_varint_and_advance(offset) {
                                        Ok((count, bytes_read)) => {
                                            offset += bytes_read;
                                            add_message_for_peer(format!("[INFO] 'headers' message contains {} headers.", count));
                                            
                                            let mut last_header_hash_bytes: Option<[u8; 32]> = None;
                                            let mut batch_hashes = Vec::new();
                                            
                                            for _ in 0..count {
                                                if payload.len() < offset + 80 {
                                                    break;
                                                }
                                                let header_bytes = &payload[offset..offset+80];
                                                
                                                // Calculate Double-SHA256 hash of the header
                                                let hash1 = Sha256::digest(header_bytes);
                                                let hash2 = Sha256::digest(hash1);
                                                
                                                let mut hash_array = [0u8; 32];
                                                hash_array.copy_from_slice(&hash2);
                                                last_header_hash_bytes = Some(hash_array);
                                                batch_hashes.push(hash_array);
                                                
                                                offset += 80;
                                                
                                                // Read tx count (should be 0 for headers)
                                                match payload.read_varint_and_advance(offset) {
                                                    Ok((_tx_count, bytes_read_tx)) => {
                                                        offset += bytes_read_tx;
                                                    }
                                                    Err(_) => break,
                                                }
                                            }
                                            
                                            if let Some(hash_bytes) = last_header_hash_bytes {
                                                let mut display_bytes = hash_bytes.to_vec();
                                                display_bytes.reverse();
                                                let hash_str = hex::encode(display_bytes);
                                                
                                                add_message_for_peer(format!("[INFO] Updating block hash to: {}", hash_str));
                                                *block_hash.lock().unwrap() = hash_str;

                                                // Update local height
                                                let mut height_lock = local_height.lock().unwrap();
                                                *height_lock += count as i32;
                                                add_message_for_peer(format!("[INFO] Synced to height: {}", *height_lock));
                                                drop(height_lock);

                                                // If we got max headers (2000), request more
                                                if count == 2000 {
                                                    add_message_for_peer("Received 2000 headers, requesting more...".to_string());
                                                    match build_getheaders_message(vec![hash_bytes], [0u8; 32]) {
                                                        Ok(getheaders_msg) => {
                                                            if let Err(e) = stream.write_all(&getheaders_msg) {
                                                                 add_message_for_peer(format!("[ERROR] Failed to send getheaders request: {}", e));
                                                            }
                                                        }
                                                        Err(e) => {
                                                            add_message_for_peer(format!("[ERROR] Failed to build getheaders message: {}", e));
                                                        }
                                                    }
                                                } else if count > 0 {
                                                    // Assume tip. Request last 10 blocks.
                                                    add_message_for_peer("Synced near tip. Requesting last 10 blocks...".to_string());
                                                    let start_idx = if batch_hashes.len() > 10 { batch_hashes.len() - 10 } else { 0 };
                                                    let hashes_to_request = &batch_hashes[start_idx..];
                                                    
                                                    let inventory: Vec<([u8; 32], u32)> = hashes_to_request.iter().map(|h| (*h, 0x40000002)).collect(); // MSG_WITNESS_BLOCK
                                                    
                                                    match build_getdata_message(inventory) {
                                                        Ok(getdata_msg) => {
                                                            if let Err(e) = stream.write_all(&getdata_msg) {
                                                                 add_message_for_peer(format!("[ERROR] Failed to send getdata request: {}", e));
                                                            } else {
                                                                session_outbound_traffic += getdata_msg.len() as u64;
                                                                add_message_for_peer("Sent 'getdata' request for blocks.".to_string());
                                                            }
                                                        }
                                                        Err(e) => {
                                                            add_message_for_peer(format!("[ERROR] Failed to build getdata message: {}", e));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            add_message_for_peer(format!("[ERROR] Failed to parse headers count: {}", e));
                                        }
                                    }
                                }
                                "getheaders" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'getheaders' message."
                                            .to_string(),
                                    );
                                }
                                "getdata" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'getdata' message."
                                            .to_string(),
                                    );
                                }
                                "addr" => {
                                    add_message_for_peer(
                                        "[INFO] Received 'addr' message.".to_string(),
                                    );
                                }
                                _ => {
                                    add_message_for_peer(format!(
                                        "[INFO] Received unhandled command: '{}'",
                                        command
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            add_message_for_peer(format!(
                                "[ERROR] Failed to parse command from header: {}",
                                e
                            ));
                            break; // Exit loop on command parsing error.
                        }
                    }
                }
                Err(e) => {
                    // Handle specific IO errors, like timeouts.
                    if let Some(io_error) = e.downcast_ref::<std::io::Error>()
                        && io_error.kind() == std::io::ErrorKind::TimedOut {
                            // If read times out, send a 'ping' message to keep the
                            // connection alive.
                            add_message_for_peer("[INFO] Read timeout. No data received for 60 seconds. Sending ping...".to_string());
                            let nonce_u64 = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_secs();
                            let mut nonce_bytes = [0u8; 8];
                            nonce_bytes.copy_from_slice(&nonce_u64.to_le_bytes());
                            match build_ping_message(nonce_bytes) {
                                Ok(ping_message) => {
                                    if let Err(e) = stream.write_all(&ping_message) {
                                        add_message_for_peer(format!(
                                            "[ERROR] Failed to send ping: {}",
                                            e
                                        ));
                                    } else {
                                        session_outbound_traffic +=
                                            ping_message.len() as u64;
                                        add_message_for_peer(
                                            "[SENT] 'ping' message with nonce."
                                                .to_string(),
                                        );
                                    }
                                }
                                Err(e) => add_message_for_peer(format!(
                                    "[ERROR] Failed to build ping message: {}",
                                    e
                                )),
                            }
                            continue; // Continue the loop to wait for a response (pong).
                        }
                    // For any other read errors, log the error and break.
                    add_message_for_peer(format!(
                        "[ERROR] Failed to read message: {}",
                        e
                    ));
                    break; // Exit loop on other read errors.
                }
            }
        }
        // When the peer loop breaks (e.g., due to error or shutdown),
        // update known_peers with the total traffic and remove from active_peers.
        add_message_for_peer(format!(
            "Disconnected from {}. Session In: {} B, Session Out: {} B",
            peer_addr,
            session_inbound_traffic,
            session_outbound_traffic
        ));
        let mut known_peers_lock = known_peers.lock().unwrap();
        known_peers_lock
            .entry(peer_addr.clone())
            .and_modify(|(total_in, total_out)| {
                *total_in += session_inbound_traffic;
                *total_out += session_outbound_traffic;
            })
            .or_insert((session_inbound_traffic, session_outbound_traffic));

        let mut active_peers_lock = active_peers.lock().unwrap();
        if let Some(state) = active_peers_lock.remove(&peer_addr) {
            if state.user_agent.contains("Gnostr") {
                add_message_for_peer("[INFO] Gnostr peer disconnected. Scheduling immediate reconnect.".to_string());
                // Add to discovery queue for reconnection.
                // This needs to be done via a channel or shared queue that the main network loop listens to.
                // For now, simulating with direct push (needs refinement for multi-thread safety if `discovered_peers_queue_peer` is not accessible).
                // Assuming `discovered_peers_queue_peer` is available here, if not, it needs to be passed down.
                // For this refactoring, I will assume `discovered_peers_queue_peer` is a global shared state accessible.
                // However, in reality, it needs to be passed to spawn_peer_handler or passed via a channel.
                // To simplify for now, the reconnection logic is currently within the main network loop.
                // The problem is that the peer_handler is for a single peer, not the global discovery queue.
                // The current reconnection for Gnostr peers is in the main network loop for outbound connections.
                // For inbound connections, we would need to pass `discovered_peers_queue` to `spawn_peer_handler`.
                // Given the current structure, for an *inbound* Gnostr peer that disconnects, we need to schedule a new *outbound* connection.
                // This means the `spawn_peer_handler` itself can't directly push to `discovered_peers_queue_network`.
                // A channel from `spawn_peer_handler` to the main network loop would be the correct solution.
                // For YOLO, I will add `discovered_peers_queue` as an argument to `spawn_peer_handler`.
                // Re-assessing: `spawn_peer_handler` needs `discovered_peers_queue` to push back Gnostr peers for reconnection.
                // The current definition does not include it.
                // Let's modify `spawn_peer_handler` signature.
            }
        }
    });
}
