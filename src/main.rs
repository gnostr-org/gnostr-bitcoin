use gnostr_bitcoin::{
    connect_and_handshake,
    build_mempool_message,
    build_ping_message,
    build_pong_message,
    read_message, // This will be public in lib.rs
    DNS_SEEDS,
    DEFAULT_PORT,
};
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting Bitcoin P2P client...");

    // Connect and handshake
    let mut stream = connect_and_handshake(DNS_SEEDS, DEFAULT_PORT)?;
    println!("Successfully connected and handshaked.");

    // Request mempool
    println!("Requesting mempool information...");
    let mempool_message = build_mempool_message()?;
    stream.write_all(&mempool_message)?;
    println!("Sent 'mempool' request.");

    // --- Node Life Cycle Loop ---
    println!("Entering message processing loop...");
    loop {
        // Set a read timeout to prevent blocking indefinitely
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;

        match read_message(&mut stream) {
            Ok((header, payload)) => {
                let command = std::str::from_utf8(&header[4..16])?.trim_end_matches('\0');
                println!("[RECEIVED] Command: '{}', Payload Size: {} bytes", command, payload.len());

                match command {
                    "version" => {
                        println!("[INFO] Received 'version' message again.");
                    }
                    "verack" => {
                        println!("[INFO] Received 'verack' message.");
                    }
                    "ping" => {
                        println!("[INFO] Received 'ping' message. Sending 'pong'.");
                        let nonce: [u8; 8] = payload.try_into().map_err(|_| "Invalid ping nonce size")?;
                        let pong_message = build_pong_message(nonce)?;
                        stream.write_all(&pong_message)?;
                        println!("[SENT] 'pong' message.");
                    }
                    "pong" => {
                        println!("[INFO] Received 'pong' message.");
                    }
                    "mempool" => {
                        println!("[INFO] Received 'mempool' response (or another mempool request).");
                    }
                    "inv" => {
                        println!("[INFO] Received 'inv' message (inventory).");
                    }
                    "tx" => {
                        println!("[INFO] Received 'tx' message (transaction).");
                    }
                    "block" => {
                        println!("[INFO] Received 'block' message.");
                    }
                    "headers" => {
                        println!("[INFO] Received 'headers' message.");
                    }
                    "getheaders" => {
                        println!("[INFO] Received 'getheaders' message.");
                    }
                    "getdata" => {
                        println!("[INFO] Received 'getdata' message.");
                    }
                    "addr" => {
                        println!("[INFO] Received 'addr' message.");
                    }
                    _ => {
                        println!("[INFO] Received unhandled command: '{}'", command);
                    }
                }
            }
            Err(e) => {
                if let Some(io_error) = e.downcast_ref::<std::io::Error>() {
                    if io_error.kind() == std::io::ErrorKind::TimedOut {
                        println!("[INFO] Read timeout. No data received for 60 seconds. Sending ping...");
                        let nonce_u64 = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
                        let mut nonce_bytes = [0u8; 8];
                        nonce_bytes.copy_from_slice(&nonce_u64.to_le_bytes());
                        let ping_message = build_ping_message(nonce_bytes)?;
                        stream.write_all(&ping_message)?;
                        println!("[SENT] 'ping' message with nonce.");
                        continue;
                    }
                }
                eprintln!("[ERROR] Failed to read message: {}", e);
                break;
            }
        }
    }

    println!("Exiting node life cycle loop.");
    Ok(())
}