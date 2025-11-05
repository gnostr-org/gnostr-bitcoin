pub mod ui;

use std::{
    fs::{self, File},
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use log::{LevelFilter, debug, error, info, warn};
use sha2::{Digest, Sha256};
use simplelog::{CombinedLogger, Config, WriteLogger};

// --- Constants ---

/// Magic bytes for the Bitcoin P2P network (Mainnet).
/// These bytes prefix every valid Bitcoin P2P message.
pub const MAGIC_BYTES: [u8; 4] = [0xF9, 0xBE, 0xB4, 0xD9]; // Mainnet

/// Default port for the Bitcoin P2P network.
pub const DEFAULT_PORT: u16 = 8333;

/// Protocol version supported by this client.
pub const PROTOCOL_VERSION: i32 = 70016;

/// Services supported by this client.
/// Currently set to 1, indicating a node that supports basic services.
pub const SERVICES: u64 = 1;
use anyhow::Result;

/// List of DNS seeds for discovering initial Bitcoin peers.
/// These domains are used to get a list of active nodes on the Bitcoin network.
pub const DNS_SEEDS: &[&str] = &[
    "dnsseed.bluematt.me",
    "dnsseed.bitcoin.dashjr-list-of-p2p-nodes.us",
    "seed.bitcoinstats.com",
    "seed.btc.petertodd.net",
    "seed.bitcoin.sprovoost.nl",
    "dnsseed.emzy.de",
    "seed.bitcoin.wiz.biz",
    "seed.bitcoin.sipa.be",
    "seed.bitcoin.jonasschnelli.ch",
    "seed.mainnet.achownodes.xyz",
];

/// Initializes the logging system.
/// Creates a log directory if it doesn't exist and sets up a logger that writes
/// to a file.
pub fn init_logger() -> Result<()> {
    let home_dir =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    let gnostr_dir = home_dir.join(".gnostr");
    let bitcoin_dir = gnostr_dir.join("bitcoin");
    let log_file_path = bitcoin_dir.join("gnostr-bitcoin.log");

    fs::create_dir_all(&bitcoin_dir)?;

    CombinedLogger::init(vec![WriteLogger::new(
        LevelFilter::Info,
        Config::default(),
        File::create(log_file_path)?,
    )])
    .map_err(anyhow::Error::from)
}

// ----------------------------------------------------------------------
// --- Bitcoin Data Parsing Trait (Fixed E0599/E0608) ---
// ----------------------------------------------------------------------

/// Trait for reading Bitcoin's variable-length integer (VarInt) encoding.
/// VarInts are used in Bitcoin protocols to represent lengths and counts
/// efficiently.
pub trait VarIntReader {
    /// Decodes a Bitcoin VarInt from a byte slice at a given offset.
    /// Returns a tuple containing the decoded `u64` value and the number of
    /// bytes read.
    fn read_varint_and_advance(&self, offset: usize) -> Result<(u64, usize)>;
}

impl VarIntReader for [u8] {
    /// Implements `VarIntReader` for byte slices.
    /// Parses a VarInt according to Bitcoin's specification:
    /// - 0xfd: next 2 bytes are u16 (little-endian)
    /// - 0xfe: next 4 bytes are u32 (little-endian)
    /// - 0xff: next 8 bytes are u64 (little-endian)
    ///   Otherwise, the first byte is the value.
    fn read_varint_and_advance(&self, offset: usize) -> Result<(u64, usize)> {
        if offset >= self.len() {
            return Err(anyhow::anyhow!("VarInt read failed: Offset out of bounds."));
        }
        let first_byte = self[offset];
        match first_byte {
            0x00..=0xfc => Ok((first_byte as u64, 1)),
            0xfd => {
                if self.len() < offset + 3 {
                    return Err(anyhow::anyhow!("Incomplete 2-byte varint."));
                }
                let bytes: [u8; 2] = self[offset + 1..offset + 3].try_into().unwrap();
                Ok((u16::from_le_bytes(bytes) as u64, 3))
            }
            0xfe => {
                if self.len() < offset + 5 {
                    return Err(anyhow::anyhow!("Incomplete 4-byte varint."));
                }
                let bytes: [u8; 4] = self[offset + 1..offset + 5].try_into().unwrap();
                Ok((u32::from_le_bytes(bytes) as u64, 5))
            }
            0xff => {
                if self.len() < offset + 9 {
                    return Err(anyhow::anyhow!("Incomplete 8-byte varint."));
                }
                let bytes: [u8; 8] = self[offset + 1..offset + 9].try_into().unwrap();
                Ok((u64::from_le_bytes(bytes), 9))
            }
        }
    }
}

/// Establishes a TCP connection to a Bitcoin peer and performs the handshake.
/// It tries to connect to the specified `target_peer_addr` or iterates through
/// `dns_seeds`. The handshake involves sending a `version` message and
/// expecting specific responses. Returns the connected `TcpStream`, the peer's
/// address, and a list of discovered peer addresses.
pub fn connect_and_handshake(
    dns_seeds: &[&str],
    default_port: u16,
    block_height: Arc<Mutex<i32>>,
    running: Arc<AtomicBool>,
    target_peer_addr: Option<String>,
) -> Result<(TcpStream, String, Vec<String>)> {
    info!("[FLOW] Attempting TCP connection and handshake...");

    let mut addresses_to_try: Vec<String> = Vec::new();
    if let Some(target) = target_peer_addr {
        addresses_to_try.push(target);
    }
    for seeder_domain in dns_seeds.iter() {
        addresses_to_try.push(format!("{}:{}", seeder_domain, default_port));
    }

    for addr_to_try in addresses_to_try.iter() {
        if !running.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!(
                "Shutdown signal received, aborting connection attempt."
            ));
        }
        info!("[FLOW] Trying to connect to {}", addr_to_try);

        let socket_addresses = match addr_to_try.to_socket_addrs() {
            Ok(addrs) => addrs,
            Err(e) => {
                info!(
                    "[INFO] Failed to resolve address {}: {}. Trying next...",
                    addr_to_try, e
                );
                continue;
            }
        };

        let mut connected_stream: Option<TcpStream> = None;
        for socket_addr in socket_addresses {
            info!(
                "[FLOW] Attempting to connect to resolved address: {}",
                socket_addr
            );
            match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(5)) {
                Ok(s) => {
                    s.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
                    info!("[INFO] Connected successfully to {}.", socket_addr);
                    connected_stream = Some(s);
                    break; // Connected to one, no need to try others for this addr_to_try
                }
                Err(e) => {
                    info!(
                        "[INFO] Failed to connect to {}: {}. Trying next resolved address...",
                        socket_addr, e
                    );
                    continue;
                }
            }
        }

        let mut stream = match connected_stream {
            Some(s) => s,
            None => {
                info!(
                    "[INFO] Failed to connect to any resolved address for {}. Trying next seed...",
                    addr_to_try
                );
                continue;
            }
        };
        let peer_addr = stream.peer_addr()?.to_string();
        let peer_addr_for_closure = peer_addr.clone();

        // Attempt handshake
        let current_block_height = block_height.clone();
        let _current_running_flag = running.clone();
        let handshake_result = (move || -> Result<(TcpStream, String, Vec<String>)> {
            info!("[FLOW] Performing Handshake (sending 'version').");
            let (version_message, _) = build_version_message()?;
            info!(
                "[SEND] 'version' message (total size: {})",
                version_message.len()
            );
            stream.write_all(&version_message)?;
            info!("[FLOW] Waiting for peer's 'version' response.");
            let (header, payload) = match read_message(&mut stream) {
                Ok(msg) => msg,
                Err(e) => {
                    error!("[ERROR] Failed to read peer\'s version message: {}", e);
                    return Err(e);
                }
            };
            let command = std::str::from_utf8(&header[4..16])?.trim_end_matches('\0');
            info!("[RECEIVED] Command: '{}'", command);

            if command == "version" {
                let mut offset: usize = 80;
                info!("[TRACE] Parsing 'version' payload (offset {}).", offset);
                let (user_agent_len, bytes_read) = payload.read_varint_and_advance(offset)?;
                offset += bytes_read + user_agent_len as usize;
                info!(
                    "[TRACE] User Agent length: {} bytes. New offset: {}",
                    user_agent_len, offset
                );

                let block_height_bytes: [u8; 4] = payload[offset..offset + 4].try_into().unwrap();
                let current_height = i32::from_le_bytes(block_height_bytes);
                info!("Current Block Height: {}", current_height);
                *current_block_height.lock().unwrap() = current_height;
            }

            let verack_message = build_verack_message()?;
            info!("[SEND] 'verack' message (size: {})", verack_message.len());
            stream.write_all(&verack_message)?;

            info!("[FLOW] Waiting for peer's 'verack' or 'addr' response.");
            match read_message(&mut stream) {
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "[ERROR] Failed to read peer\'s verack or addr message: {}",
                        e
                    );
                    return Err(e);
                }
            };

            // Request peer addresses
            let getaddr_message = build_getaddr_message()?;
            info!("[SEND] 'getaddr' message (size: {})", getaddr_message.len());
            stream.write_all(&getaddr_message)?;

            info!("[FLOW] Waiting for peer's 'addr' response.");
            let (header, payload) = match read_message(&mut stream) {
                Ok(msg) => msg,
                Err(e) => {
                    error!("[ERROR] Failed to read peer\'s addr message: {}", e);
                    return Err(e);
                }
            };
            let command = std::str::from_utf8(&header[4..16])?.trim_end_matches('\0');
            info!("[RECEIVED] Command: '{}'", command);

            let mut discovered_peers: Vec<String> = Vec::new();
            if command == "addr" {
                info!("[INFO] Received 'addr' message. Parsing payload.");
                // Parse the 'addr' payload to extract peer addresses
                let mut offset = 0;
                let (count, bytes_read) = payload.read_varint_and_advance(offset)?;
                offset += bytes_read;
                info!(
                    "[TRACE] Number of addresses in 'addr' message: {}. New offset: {}",
                    count, offset
                );

                for _ in 0..count {
                    // Each address entry has a timestamp (4 bytes), services (8 bytes), IP address
                    // (16 bytes), and port (2 bytes).
                    if payload.len() < offset + 30 {
                        warn!("[WARN] Incomplete address entry in 'addr' message.");
                        break;
                    }
                    // Skip timestamp (4 bytes)
                    offset += 4;
                    // Skip services (8 bytes)
                    offset += 8;
                    // IPv6 address (16 bytes) - Bitcoin uses IPv6 format for addresses, with
                    // IPv4-mapped IPv6 addresses representing IPv4.
                    let ip_bytes = &payload[offset..offset + 16];
                    offset += 16;
                    let port_bytes: [u8; 2] = payload[offset..offset + 2].try_into().unwrap();
                    offset += 2;

                    let ip_addr = if ip_bytes[0..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff] {
                        // IPv4-mapped IPv6 address: extract the IPv4 part.
                        format!(
                            "{}.{}.{}.{}",
                            ip_bytes[12], ip_bytes[13], ip_bytes[14], ip_bytes[15]
                        )
                    } else {
                        // IPv6 address (simplified representation for now).
                        // For a full implementation, you'd parse the IPv6 bytes properly.
                        format!("{:?}", ip_bytes)
                    };
                    let port = u16::from_be_bytes(port_bytes);
                    let peer_address = format!("{}:{}", ip_addr, port);
                    discovered_peers.push(peer_address);
                }
                info!(
                    "[INFO] Discovered {} peers from 'addr' message.",
                    discovered_peers.len()
                );
            }
            Ok((stream, peer_addr_for_closure, discovered_peers))
        })(); // Call the closure immediately

        match handshake_result {
            Err(e) => {
                error!(
                    "[ERROR] Handshake failed with {}: {}. Trying next seeder...",
                    peer_addr, e
                );
                continue;
            }
            Ok((s, peer_addr, peers)) => {
                info!(
                    "[INFO] Handshake successful with {}. Discovered {} peers.",
                    peer_addr,
                    peers.len()
                );
                return Ok((s, peer_addr, peers));
            }
        }
    }

    Err(anyhow::anyhow!(
        "Failed to connect and handshake with any known Bitcoin seeders."
    ))
}

// ----------------------------------------------------------------------
// --- Protocol Helper Functions (Tracing) ---
// ----------------------------------------------------------------------

/// Reads a complete Bitcoin P2P message from a given reader.
/// A message consists of a 24-byte header and a variable-length payload.
/// It returns the header bytes and the payload bytes.
/// Performs basic validation like command parsing and checksum checking.
pub fn read_message<R: Read>(stream: &mut R) -> Result<([u8; 24], Vec<u8>)> {
    info!("[FUNC] read_message: Attempting to read 24-byte header.");
    let mut header_bytes = [0u8; 24];
    match stream.read_exact(&mut header_bytes) {
        Ok(_) => debug!("[TRACE] Read 24 bytes for header."),
        Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            warn!("[WARN] Connection closed prematurely by peer during header read.");
            return Err(anyhow::anyhow!(
                "Connection closed prematurely during header read."
            ));
        }
        Err(e) => return Err(e.into()),
    }

    // Parse payload length from the header (bytes 16-19, little-endian).
    let payload_len = u32::from_le_bytes(header_bytes[16..20].try_into().unwrap());
    // Extract command from the header (bytes 4-15), removing null padding.
    let command = std::str::from_utf8(&header_bytes[4..16])?.trim_end_matches('\0');
    info!(
        "[TRACE] Header read. Command: '{}', Payload Length: {} bytes.",
        command, payload_len
    );

    let mut payload = vec![0u8; payload_len as usize];
    if payload_len > 0 {
        match stream.read_exact(&mut payload) {
            Ok(_) => debug!("[TRACE] Read {} bytes for payload.", payload_len),
            Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                warn!(
                    "[WARN] Connection closed prematurely by peer during payload read. Expected {} bytes.",
                    payload_len
                );
                return Err(anyhow::anyhow!(
                    "Connection closed prematurely during payload read. Expected {} bytes.",
                    payload_len
                ));
            }
            Err(e) => return Err(e.into()),
        }
        // Validate the checksum.
        let expected_checksum: [u8; 4] = header_bytes[20..24].try_into().unwrap();
        let actual_checksum = calculate_checksum(&payload);

        if expected_checksum != actual_checksum {
            warn!(
                "[WARN] Checksum mismatch! Expected: {:?}, Actual: {:?}",
                expected_checksum, actual_checksum
            );
        }
    }

    info!("[FUNC] read_message: Finished reading message.");
    Ok((header_bytes, payload))
}

/// Constructs a Bitcoin P2P `version` message.
/// This message is sent when establishing a connection to a peer.
/// It includes protocol version, services, timestamp, peer addresses, nonce,
/// user agent, and start height.
pub fn build_version_message() -> Result<(Vec<u8>, usize)> {
    info!("[FUNC] build_version_message: Assembling payload.");
    let mut payload = Vec::new();
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

    // Protocol version (little-endian).
    payload.write_all(&PROTOCOL_VERSION.to_le_bytes())?;
    // Services supported by the sender (little-endian).
    payload.write_all(&SERVICES.to_le_bytes())?;
    // Current timestamp (little-endian).
    payload.write_all(&now.to_le_bytes())?;
    // Receiver's address (26 bytes: Services (8) + IPv6 Address (16) + Port (2)).
    // Filled with zeros.
    payload.write_all(&[0u8; 26])?; // addr_recv
    // Sender's network address (26 bytes). Filled with zeros as we don't specify it
    // here.
    payload.write_all(&[0u8; 26])?; // addr_from
    // Nonce to detect loops (8 bytes).
    payload.write_all(&0u64.to_le_bytes())?; // nonce
    // User agent of the client (Length prefixed string).
    let user_agent = "/rust-sha2-p2p-client/";
    payload.write_all(&[user_agent.len() as u8])?;
    payload.write_all(user_agent.as_bytes())?;
    // Last block height processed by the sender (little-endian).
    payload.write_all(&0i32.to_le_bytes())?; // start_height
    // Whether relay options are enabled (1 byte boolean).
    payload.write_all(&[1])?; // relay

    let payload_len = payload.len();
    let checksum = calculate_checksum(&payload);

    // Construct the full message: MAGIC bytes + Command (12 bytes) + Payload Length
    // (4 bytes) + Checksum (4 bytes) + Payload.
    let mut raw_message = Vec::new();
    raw_message.write_all(&MAGIC_BYTES)?;
    let mut command_bytes = [0u8; 12];
    command_bytes[0..7].copy_from_slice(b"version");
    raw_message.write_all(&command_bytes)?;
    raw_message.write_all(&(payload_len as u32).to_le_bytes())?;
    raw_message.write_all(&checksum)?;
    raw_message.write_all(&payload)?;

    info!(
        "[FUNC] build_version_message: Done. Payload size: {}",
        payload_len
    );
    Ok((raw_message, payload_len))
}

/// Constructs a Bitcoin P2P `verack` message.
/// This message is sent after receiving a `version` message and successfully
/// negotiating the connection. It signifies that the peer has acknowledged the
/// version information and is ready to proceed.
pub fn build_verack_message() -> Result<Vec<u8>> {
    info!("[FUNC] build_verack_message: Assembling message (Zero payload).");
    let payload = Vec::new();
    let payload_len = 0;
    let checksum = calculate_checksum(&payload);

    let mut raw_message = Vec::new();
    raw_message.write_all(&MAGIC_BYTES)?;
    let mut command_bytes = [0u8; 12];
    command_bytes[0..6].copy_from_slice(b"verack");
    raw_message.write_all(&command_bytes)?;
    raw_message.write_all(&(payload_len as u32).to_le_bytes())?;
    raw_message.write_all(&checksum)?;

    info!("[FUNC] build_verack_message: Done.");
    Ok(raw_message)
}

/// Constructs a Bitcoin P2P `mempool` message.
/// This message requests information about all unconfirmed transactions in the
/// mempool.
pub fn build_mempool_message() -> Result<Vec<u8>> {
    info!("[FUNC] build_mempool_message: Assembling message (Zero payload).");
    let payload = Vec::new();
    let payload_len = 0;
    let checksum = calculate_checksum(&payload);

    let mut raw_message = Vec::new();
    raw_message.write_all(&MAGIC_BYTES)?;
    let mut command_bytes = [0u8; 12];
    command_bytes[0..7].copy_from_slice(b"mempool");
    raw_message.write_all(&command_bytes)?;
    raw_message.write_all(&(payload_len as u32).to_le_bytes())?;
    raw_message.write_all(&checksum)?;

    info!("[FUNC] build_mempool_message: Done.");
    Ok(raw_message)
}

/// Constructs a Bitcoin P2P `ping` message.
/// This message is used to check if a peer is still responsive.
/// It includes a nonce that should be echoed in the `pong` message.
pub fn build_ping_message(nonce: [u8; 8]) -> Result<Vec<u8>> {
    info!("[FUNC] build_ping_message: Assembling payload.");
    let mut payload = Vec::new();
    payload.write_all(&nonce)?;
    let payload_len = payload.len();
    let checksum = calculate_checksum(&payload);

    let mut raw_message = Vec::new();
    raw_message.write_all(&MAGIC_BYTES)?;
    let mut command_bytes = [0u8; 12];
    command_bytes[0..4].copy_from_slice(b"ping");
    raw_message.write_all(&command_bytes)?;
    raw_message.write_all(&(payload_len as u32).to_le_bytes())?;
    raw_message.write_all(&checksum)?;
    raw_message.write_all(&payload)?;

    info!("[FUNC] build_ping_message: Done.");
    Ok(raw_message)
}

/// Constructs a Bitcoin P2P `pong` message.
/// This message is a response to a `ping` message, echoing the nonce received.
pub fn build_pong_message(nonce: [u8; 8]) -> Result<Vec<u8>> {
    info!("[FUNC] build_pong_message: Assembling payload.");
    let mut payload = Vec::new();
    payload.write_all(&nonce)?;
    let payload_len = payload.len();
    let checksum = calculate_checksum(&payload);

    let mut raw_message = Vec::new();
    raw_message.write_all(&MAGIC_BYTES)?;
    let mut command_bytes = [0u8; 12];
    command_bytes[0..4].copy_from_slice(b"pong");
    raw_message.write_all(&command_bytes)?;
    raw_message.write_all(&(payload_len as u32).to_le_bytes())?;
    raw_message.write_all(&checksum)?;
    raw_message.write_all(&payload)?;

    info!("[FUNC] build_pong_message: Done.");
    Ok(raw_message)
}

/// Constructs a Bitcoin P2P `getaddr` message.
/// This message requests a list of known active peers from the connected node.
pub fn build_getaddr_message() -> Result<Vec<u8>> {
    info!("[FUNC] build_getaddr_message: Assembling message (Zero payload).");
    let payload = Vec::new();
    let payload_len = 0;
    let checksum = calculate_checksum(&payload);

    let mut raw_message = Vec::new();
    raw_message.write_all(&MAGIC_BYTES)?;
    let mut command_bytes = [0u8; 12];
    command_bytes[0..7].copy_from_slice(b"getaddr");
    raw_message.write_all(&command_bytes)?;
    raw_message.write_all(&(payload_len as u32).to_le_bytes())?;
    raw_message.write_all(&checksum)?;

    info!("[FUNC] build_getaddr_message: Done.");
    Ok(raw_message)
}

/// Helper function for encoding a `u64` value into Bitcoin's VarInt format.
/// VarInt is used to encode integers of variable length, saving space for
/// smaller numbers.
fn encode_varint(value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    if value <= 0xfc {
        bytes.push(value as u8);
    } else if value <= 0xffff {
        bytes.push(0xfd);
        bytes.extend_from_slice(&(value as u16).to_le_bytes());
    } else if value <= 0xffffffff {
        bytes.push(0xfe);
        bytes.extend_from_slice(&(value as u32).to_le_bytes());
    } else {
        bytes.push(0xff);
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Constructs a Bitcoin P2P `getheaders` message.
/// This message is used to request block headers from a specific block hash up
/// to a stop hash. It requires a list of block locator hashes and a stop hash.
pub fn build_getheaders_message(
    locator_hashes: Vec<[u8; 32]>,
    stop_hash: [u8; 32],
) -> Result<Vec<u8>> {
    info!("[FUNC] build_getheaders_message: Assembling message.");
    let mut payload = Vec::new();

    // Protocol version (little-endian).
    payload.write_all(&PROTOCOL_VERSION.to_le_bytes())?;

    // Hash count and Block locator hashes.
    let hash_count = locator_hashes.len() as u64;
    payload.write_all(&encode_varint(hash_count))?; // Use helper
    for hash in locator_hashes {
        payload.write_all(&hash)?;
    }

    // Hash stop: specifies the hash of the block that should be the last one
    // returned.
    payload.write_all(&stop_hash)?;

    let payload_len = payload.len();
    let checksum = calculate_checksum(&payload);

    // Construct the full message.
    let mut raw_message = Vec::new();
    raw_message.write_all(&MAGIC_BYTES)?;
    let mut command_bytes = [0u8; 12];
    command_bytes[0..10].copy_from_slice(b"getheaders");
    raw_message.write_all(&command_bytes)?;
    raw_message.write_all(&(payload_len as u32).to_le_bytes())?;
    raw_message.write_all(&checksum)?;
    raw_message.write_all(&payload)?;

    info!(
        "[FUNC] build_getheaders_message: Done. Payload size: {}",
        payload_len
    );
    Ok(raw_message)
}

fn calculate_checksum(payload: &[u8]) -> [u8; 4] {
    debug!(
        "[FUNC] calculate_checksum: Hashing {} bytes.",
        payload.len()
    );
    let hash1 = Sha256::digest(payload);
    let hash2 = Sha256::digest(hash1);
    let mut checksum = [0u8; 4];
    checksum.copy_from_slice(&hash2[0..4]);
    checksum
}

// ----------------------------------------------------------------------
// --- Testing Module (FINAL FIXED STRUCTURE) ---
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{io::Cursor, str};

    use super::*;

    /// Helper function to simulate and test the handshake process.
    /// It reads mock `version` and `verack` messages from a stream
    /// and verifies the extracted block height.
    fn perform_test_handshake<S: Read>(mut stream: S) -> Result<i32> {
        // 1. Client reads peer's 'version' message
        let (header, payload) = read_message(&mut stream)?;
        let command = std::str::from_utf8(&header[4..16])?.trim_end_matches('\0');

        if command != "version" {
            return Err(anyhow::anyhow!("Expected 'version', got '{}'.", command));
        }

        // Extract height (core test value)
        let mut offset: usize = 80; // Offset to the block height field in the version payload
        let (_user_agent_len, bytes_read) = payload.read_varint_and_advance(offset)?;
        offset += bytes_read + _user_agent_len as usize;
        let block_height_bytes: [u8; 4] = payload[offset..offset + 4].try_into().unwrap();
        let height = i32::from_le_bytes(block_height_bytes);
        debug!("[DEBUG] Final extracted height: {}", height);

        // 2. Client reads peer's 'verack' message
        let (header, _) = read_message(&mut stream)?;
        let command = std::str::from_utf8(&header[4..16])?.trim_end_matches('\0');
        if command != "verack" {
            return Err(anyhow::anyhow!("Expected 'verack', got '{}'.", command));
        }

        Ok(i32::from_le_bytes(block_height_bytes))
    }

    // --- Mock Data ---
    /// Mock block height in little-endian format (e.g., 810000).
    const MOCK_HEIGHT_LE: [u8; 4] = [0x60, 0x5c, 0x0c, 0x00]; // 810000
    /// Mock user agent string for testing.
    const MOCK_USER_AGENT: &[u8] = b"/mock-test-client/";
    /// Mock relay flag (0 for false).
    const MOCK_RELAY: [u8; 1] = [0x00];

    /// Creates a prefix for the mock `version` payload, containing fields
    /// before the user agent and block height.
    fn create_mock_version_payload_prefix() -> Vec<u8> {
        let mut payload_prefix = Vec::new();
        let now: i64 = 0; // Mock timestamp for consistency
        payload_prefix.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        payload_prefix.extend_from_slice(&SERVICES.to_le_bytes());
        payload_prefix.extend_from_slice(&now.to_le_bytes());
        payload_prefix.extend_from_slice(&[0u8; 26]); // addr_recv (placeholder)
        payload_prefix.extend_from_slice(&[0u8; 26]); // addr_from (placeholder)
        payload_prefix.extend_from_slice(&0u64.to_le_bytes()); // nonce (placeholder)
        payload_prefix
    }

    /// Creates a mock `verack` response message, as expected by the handshake.
    fn create_mock_verack_response() -> Vec<u8> {
        let payload = Vec::new();
        let mut msg = Vec::new();
        msg.extend_from_slice(&MAGIC_BYTES);
        let mut cmd = [0u8; 12];
        cmd[0..6].copy_from_slice(b"verack");
        msg.extend_from_slice(&cmd);
        msg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        msg.extend_from_slice(&calculate_checksum(&payload));
        debug!(
            "[DEBUG] create_mock_verack_response: message length = {}",
            msg.len()
        );
        msg
    }

    /// Creates a mock `version` response message, including mock data for user
    /// agent and block height.
    fn create_mock_version_response() -> Vec<u8> {
        let mut payload = create_mock_version_payload_prefix();
        // User agent length and string.
        payload.push(MOCK_USER_AGENT.len() as u8);
        payload.extend_from_slice(MOCK_USER_AGENT);
        // Block height and relay flag.
        payload.extend_from_slice(&MOCK_HEIGHT_LE);
        payload.extend_from_slice(&MOCK_RELAY);

        let mut msg = Vec::new();
        msg.extend_from_slice(&MAGIC_BYTES);
        let mut cmd = [0u8; 12];
        cmd[0..7].copy_from_slice(b"version");
        msg.extend_from_slice(&cmd);
        msg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        msg.extend_from_slice(&calculate_checksum(&payload));
        msg.extend_from_slice(&payload);
        debug!(
            "[DEBUG] create_mock_version_response: message length = {}",
            msg.len()
        );
        msg
    }

    #[test]
    fn test_mock_seeder_handshake_and_height_check() -> Result<()> {
        // Tests the simulated handshake process by creating mock version and verack
        // messages.
        let mock_version = create_mock_version_response();
        let mock_verack = create_mock_verack_response();

        let mut mock_response = Vec::new();
        mock_response.extend_from_slice(&mock_version);
        mock_response.extend_from_slice(&mock_verack);
        debug!(
            "[DEBUG] test_mock_seeder_handshake_and_height_check: mock_response total length = {}",
            mock_response.len()
        );

        let mock_stream = Cursor::new(mock_response);

        let height = perform_test_handshake(mock_stream)?;

        assert_eq!(height, 810080, "Block height extraction failed.");
        Ok(())
    }

    #[test]
    fn test_varint_decoding_robust() -> Result<()> {
        // Tests the robust decoding of VarInts for different lengths.
        let payload_2b: [u8; 3] = [0xfd, 0x00, 0x01]; // 0xfd indicates next two bytes are u16
        assert_eq!(payload_2b.read_varint_and_advance(0)?, (256, 3));
        let payload_4b: [u8; 5] = [0xfe, 0x00, 0x00, 0x01, 0x00]; // 0xfe indicates next four bytes are u32
        assert_eq!(payload_4b.read_varint_and_advance(0)?, (65536, 5));
        Ok(())
    }

    #[test]
    fn test_varint_decoding_single_byte() -> Result<()> {
        // Test decoding a single-byte varint (value <= 0xfc)
        let payload = [0x7b]; // Value 123
        assert_eq!(payload.read_varint_and_advance(0)?, (123, 1));
        Ok(())
    }

    #[test]
    fn test_varint_decoding_max_single_byte() -> Result<()> {
        // Test decoding the maximum single-byte varint (0xfc)
        let payload = [0xfc]; // Value 252
        assert_eq!(payload.read_varint_and_advance(0)?, (252, 1));
        Ok(())
    }

    #[test]
    fn test_varint_decoding_max_2_byte() -> Result<()> {
        // Test decoding the maximum 2-byte varint (0xffff)
        let payload = [0xfd, 0xff, 0xff]; // Value 65535
        assert_eq!(payload.read_varint_and_advance(0)?, (65535, 3));
        Ok(())
    }

    #[test]
    fn test_varint_decoding_max_4_byte() -> Result<()> {
        // Test decoding the maximum 4-byte varint (0xffffffff)
        let payload = [0xfe, 0xff, 0xff, 0xff, 0xff]; // Value 4294967295
        assert_eq!(payload.read_varint_and_advance(0)?, (4294967295, 5));
        Ok(())
    }

    #[test]
    fn test_varint_decoding_max_8_byte() -> Result<()> {
        // Test decoding the maximum 8-byte varint (0xffffffffffffffff)
        let payload = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]; // Value 18446744073709551615
        assert_eq!(
            payload.read_varint_and_advance(0)?,
            (18446744073709551615, 9)
        );
        Ok(())
    }

    #[test]
    fn test_varint_decoding_incomplete_2_byte() -> Result<()> {
        // Test error for incomplete 2-byte varint
        let payload = [0xfd, 0x00];
        let err = payload.read_varint_and_advance(0).unwrap_err();
        assert_eq!(err.to_string(), "Incomplete 2-byte varint.");
        Ok(())
    }

    #[test]
    fn test_varint_decoding_incomplete_4_byte() -> Result<()> {
        // Test error for incomplete 4-byte varint
        let payload = [0xfe, 0x00, 0x00, 0x01];
        let err = payload.read_varint_and_advance(0).unwrap_err();
        assert_eq!(err.to_string(), "Incomplete 4-byte varint.");
        Ok(())
    }

    #[test]
    fn test_varint_decoding_incomplete_8_byte() -> Result<()> {
        // Test error for incomplete 8-byte varint
        let payload = [0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
        let err = payload.read_varint_and_advance(0).unwrap_err();
        assert_eq!(err.to_string(), "Incomplete 8-byte varint.");
        Ok(())
    }

    #[test]
    fn test_varint_decoding_offset_out_of_bounds() -> Result<()> {
        // Test error for offset out of bounds
        let payload = [0x01];
        let err = payload.read_varint_and_advance(1).unwrap_err();
        assert_eq!(err.to_string(), "VarInt read failed: Offset out of bounds.");
        Ok(())
    }

    #[test]
    fn test_encode_varint_single_byte() {
        // Test encoding a single-byte varint
        assert_eq!(encode_varint(123), vec![0x7b]);
        assert_eq!(encode_varint(0xfc), vec![0xfc]);
    }

    #[test]
    fn test_encode_varint_2_bytes() {
        // Test encoding a 2-byte varint
        assert_eq!(encode_varint(253), vec![0xfd, 0xfd, 0x00]);
        assert_eq!(encode_varint(0xffff), vec![0xfd, 0xff, 0xff]);
    }

    #[test]
    fn test_encode_varint_4_bytes() {
        // Test encoding a 4-byte varint
        assert_eq!(encode_varint(0x10000), vec![0xfe, 0x00, 0x00, 0x01, 0x00]);
        assert_eq!(
            encode_varint(0xffffffff),
            vec![0xfe, 0xff, 0xff, 0xff, 0xff]
        );
    }

    #[test]
    fn test_encode_varint_8_bytes() {
        // Test encoding an 8-byte varint
        assert_eq!(
            encode_varint(0x100000000),
            vec![0xff, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            encode_varint(0xffffffffffffffff),
            vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
    }

    #[test]
    fn test_mempool_message_encoding() -> Result<()> {
        // Verifies that the `mempool` message is correctly encoded with a zero payload.
        let mempool_msg = build_mempool_message()?;
        // Check command name is correct.
        assert_eq!(str::from_utf8(&mempool_msg[4..11])?, "mempool");
        // Check payload length (should be 0).
        assert_eq!(
            u32::from_le_bytes(mempool_msg[16..20].try_into().unwrap()),
            0
        );
        // Check total message length (24 bytes for header).
        assert_eq!(mempool_msg.len(), 24);
        Ok(())
    }

    #[test]
    fn test_build_version_message() -> Result<()> {
        let (version_msg, payload_len) = build_version_message()?;

        assert_eq!(&version_msg[0..4], MAGIC_BYTES);
        assert_eq!(str::from_utf8(&version_msg[4..11])?, "version");
        assert_eq!(
            u32::from_le_bytes(version_msg[16..20].try_into().unwrap()),
            payload_len as u32
        );
        assert_eq!(version_msg.len(), 24 + payload_len);

        // Basic check for some payload content (e.g., protocol version)
        assert_eq!(
            u32::from_le_bytes(version_msg[24..28].try_into().unwrap()),
            PROTOCOL_VERSION as u32
        );
        Ok(())
    }

    #[test]
    fn test_build_verack_message() -> Result<()> {
        let verack_msg = build_verack_message()?;

        assert_eq!(&verack_msg[0..4], MAGIC_BYTES);
        assert_eq!(str::from_utf8(&verack_msg[4..10])?, "verack");
        assert_eq!(
            u32::from_le_bytes(verack_msg[16..20].try_into().unwrap()),
            0
        );
        assert_eq!(verack_msg.len(), 24);
        Ok(())
    }

    #[test]
    fn test_build_ping_message() -> Result<()> {
        let nonce = [1, 2, 3, 4, 5, 6, 7, 8];
        let ping_msg = build_ping_message(nonce)?;

        assert_eq!(&ping_msg[0..4], MAGIC_BYTES);
        assert_eq!(str::from_utf8(&ping_msg[4..8])?, "ping");
        assert_eq!(
            u32::from_le_bytes(ping_msg[16..20].try_into().unwrap()),
            8
        );
        assert_eq!(ping_msg.len(), 24 + 8);
        assert_eq!(&ping_msg[24..32], nonce);
        Ok(())
    }

    #[test]
    fn test_build_pong_message() -> Result<()> {
        let nonce = [8, 7, 6, 5, 4, 3, 2, 1];
        let pong_msg = build_pong_message(nonce)?;

        assert_eq!(&pong_msg[0..4], MAGIC_BYTES);
        assert_eq!(str::from_utf8(&pong_msg[4..8])?, "pong");
        assert_eq!(
            u32::from_le_bytes(pong_msg[16..20].try_into().unwrap()),
            8
        );
        assert_eq!(pong_msg.len(), 24 + 8);
        assert_eq!(&pong_msg[24..32], nonce);
        Ok(())
    }

    #[test]
    fn test_build_getaddr_message() -> Result<()> {
        let getaddr_msg = build_getaddr_message()?;

        assert_eq!(&getaddr_msg[0..4], MAGIC_BYTES);
        assert_eq!(str::from_utf8(&getaddr_msg[4..11])?, "getaddr");
        assert_eq!(
            u32::from_le_bytes(getaddr_msg[16..20].try_into().unwrap()),
            0
        );
        assert_eq!(getaddr_msg.len(), 24);
        Ok(())
    }

    #[test]
    fn test_build_getheaders_message() -> Result<()> {
        let locator_hashes = vec![[0u8; 32], [1u8; 32]];
        let stop_hash = [2u8; 32];
        let getheaders_msg = build_getheaders_message(locator_hashes.clone(), stop_hash)?;

        assert_eq!(&getheaders_msg[0..4], MAGIC_BYTES);
        assert_eq!(str::from_utf8(&getheaders_msg[4..14])?, "getheaders");

        // Calculate expected payload length:
        // PROTOCOL_VERSION (4 bytes)
        // hash_count (varint, 1 byte for 2 hashes)
        // locator_hashes (2 * 32 bytes)
        // stop_hash (32 bytes)
        let expected_payload_len = 4 + 1 + (2 * 32) + 32;
        assert_eq!(
            u32::from_le_bytes(getheaders_msg[16..20].try_into().unwrap()),
            expected_payload_len as u32
        );
        assert_eq!(getheaders_msg.len(), 24 + expected_payload_len);

        // Verify parts of the payload
        let mut offset = 24; // Start of payload
        assert_eq!(
            u32::from_le_bytes(getheaders_msg[offset..offset + 4].try_into().unwrap()),
            PROTOCOL_VERSION as u32
        );
        offset += 4;

        assert_eq!(getheaders_msg[offset], 2); // hash_count varint
        offset += 1;

        assert_eq!(&getheaders_msg[offset..offset + 32], &locator_hashes[0]);
        offset += 32;
        assert_eq!(&getheaders_msg[offset..offset + 32], &locator_hashes[1]);
        offset += 32;

        assert_eq!(&getheaders_msg[offset..offset + 32], &stop_hash);

        Ok(())
    }

    #[test]
    fn test_calculate_checksum() {
        // Test with an empty payload
        let empty_payload = vec![];
        let expected_checksum_empty = [0x5d, 0xf6, 0xe0, 0xe2]; // Double SHA256 of empty string
        assert_eq!(calculate_checksum(&empty_payload), expected_checksum_empty);

        // Test with a known payload (e.g., "hello world")
        let hello_world_payload = b"hello world".to_vec();
        let expected_checksum_hello_world = [0xbc, 0x62, 0xd4, 0xb8]; // Corrected pre-calculated double SHA256 of "hello world"
        assert_eq!(
            calculate_checksum(&hello_world_payload),
            expected_checksum_hello_world
        );

        // Test with a longer payload
        let long_payload = vec![0; 100];
        let expected_checksum_long = [0x71, 0x81, 0x6d, 0xf1]; // Corrected pre-calculated double SHA256 of 100 null bytes
        assert_eq!(calculate_checksum(&long_payload), expected_checksum_long);
    }

}
