pub mod ui;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{SystemTime, UNIX_EPOCH};
use sha2::{Digest, Sha256};
use log::{info, error, warn, LevelFilter};
use simplelog::{CombinedLogger, WriteLogger, Config};
use std::fs::{self, File};
use std::path::PathBuf;

// --- Constants ---
pub const MAGIC_BYTES: [u8; 4] = [0xF9, 0xBE, 0xB4, 0xD9]; // Mainnet
pub const DEFAULT_PORT: u16 = 8333;
pub const PROTOCOL_VERSION: i32 = 70016;
pub const SERVICES: u64 = 1;

pub const DNS_SEEDS: &[&str] = &[
    "seed.bitcoin.sipa.be", "dnsseed.bluematt.me", "dnsseed.bitcoin.dashjr.org",
    "seed.btc.petertodd.org", "dnsseed.emzy.de", "seed.bitcoin.wiz.biz",
];

pub fn init_logger() -> Result<(), Box<dyn std::error::Error>> {
    let home_dir = dirs::home_dir().ok_or("Could not find home directory")?;
    let gnostr_dir = home_dir.join(".gnostr");
    let bitcoin_dir = gnostr_dir.join("bitcoin");
    let log_file_path = bitcoin_dir.join("gnostr-bitcoin.log");

    fs::create_dir_all(&bitcoin_dir)?;

    CombinedLogger::init(
        vec![
            WriteLogger::new(
                LevelFilter::Info,
                Config::default(),
                File::create(log_file_path)?
            ),
        ]
    ).map_err(|e| e.into())
}

// ----------------------------------------------------------------------
// --- Bitcoin Data Parsing Trait (Fixed E0599/E0608) ---
// ----------------------------------------------------------------------

pub trait VarIntReader {
    /// Decodes a Bitcoin VarInt from a byte slice at a given offset.
    /// Returns (value, bytes_read).
    fn read_varint_and_advance(&self, offset: usize) -> Result<(u64, usize), Box<dyn std::error::Error>>;
}

impl VarIntReader for [u8] {
    fn read_varint_and_advance(&self, offset: usize) -> Result<(u64, usize), Box<dyn std::error::Error>> {
        if offset >= self.len() { return Err("VarInt read failed: Offset out of bounds.".into()); }
        let first_byte = self[offset];
        match first_byte {
            0x00..=0xfc => Ok((first_byte as u64, 1)),
            0xfd => {
                if self.len() < offset + 3 { return Err("Incomplete 2-byte varint.".into()); }
                let bytes: [u8; 2] = self[offset + 1..offset + 3].try_into().unwrap();
                Ok((u16::from_le_bytes(bytes) as u64, 3))
            }
            0xfe => {
                if self.len() < offset + 5 { return Err("Incomplete 4-byte varint.".into()); }
                let bytes: [u8; 4] = self[offset + 1..offset + 5].try_into().unwrap();
                Ok((u32::from_le_bytes(bytes) as u64, 5))
            }
            0xff => {
                if self.len() < offset + 9 { return Err("Incomplete 8-byte varint.".into()); }
                let bytes: [u8; 8] = self[offset + 1..offset + 9].try_into().unwrap();
                Ok((u64::from_le_bytes(bytes), 9))
            }
        }
    }
}

pub fn connect_and_handshake(
    dns_seeds: &[&str],
    default_port: u16,
) -> Result<TcpStream, Box<dyn std::error::Error>> {
    info!("[FLOW] Attempting TCP connection and handshake...");

    for seeder_domain in dns_seeds.iter() {
        info!("[FLOW] Trying to connect to {}: {}", seeder_domain, default_port);
        let mut stream = match TcpStream::connect(format!("{}:{}", seeder_domain, default_port)) {
            Ok(s) => {
                s.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
                info!("[INFO] Connected successfully to {}", seeder_domain);
                s
            }
            Err(e) => {
                info!("[INFO] Failed to connect to {}: {}. Trying next...", seeder_domain, e);
                continue;
            }
        };

        // Attempt handshake
        let handshake_result = (|| -> Result<(), Box<dyn std::error::Error>> {
            info!("[FLOW] Performing Handshake (sending 'version').");
            let (version_message, _) = build_version_message()?;
            info!("[SEND] 'version' message (total size: {})", version_message.len());
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
                info!("[TRACE] User Agent length: {} bytes. New offset: {}", user_agent_len, offset);

                let block_height_bytes: [u8; 4] = payload[offset..offset + 4].try_into().unwrap();
                info!("Current Block Height: {}", i32::from_le_bytes(block_height_bytes));
            }

            let verack_message = build_verack_message()?;
            info!("[SEND] 'verack' message (size: {})", verack_message.len());
            stream.write_all(&verack_message)?;

            info!("[FLOW] Waiting for peer's 'verack' or 'addr' response.");
            match read_message(&mut stream) {
                Ok(_) => {{}},
                Err(e) => {
                    error!("[ERROR] Failed to read peer\'s verack or addr message: {}", e);
                    return Err(e);
                }
            };
            Ok(())
        })(); // Call the closure immediately

        match handshake_result {
            Err(e) => {
                error!("[ERROR] Handshake failed with {}: {}. Trying next seeder...", seeder_domain, e);
                continue;
            }
            Ok(_) => {
                info!("[INFO] Handshake successful with {}.", seeder_domain);
                return Ok(stream);
            }
        }
    }

    Err("Failed to connect and handshake with any known Bitcoin seeders.".into())
}

// ----------------------------------------------------------------------
// --- Protocol Helper Functions (Tracing) ---
// ----------------------------------------------------------------------

pub fn read_message<R: Read>(stream: &mut R) -> Result<([u8; 24], Vec<u8>), Box<dyn std::error::Error>> {
    info!("[FUNC] read_message: Attempting to read 24-byte header.");
    let mut header_bytes = [0u8; 24];
    stream.read_exact(&mut header_bytes)?;
    
    let payload_len = u32::from_le_bytes(header_bytes[16..20].try_into().unwrap());
    let command = std::str::from_utf8(&header_bytes[4..16])?.trim_end_matches('\0');
    info!("[TRACE] Header read. Command: '{}', Payload Length: {} bytes.", command, payload_len);

    let mut payload = vec![0u8; payload_len as usize];
    if payload_len > 0 {
        stream.read_exact(&mut payload)?;
        let expected_checksum: [u8; 4] = header_bytes[20..24].try_into().unwrap();
        let actual_checksum = calculate_checksum(&payload);
        
        if expected_checksum != actual_checksum {
            warn!("[WARN] Checksum mismatch! Expected: {:?}, Actual: {:?}", expected_checksum, actual_checksum);
        }
    }
    
    info!("[FUNC] read_message: Finished reading message.");
    Ok((header_bytes, payload))
}


pub fn build_version_message() -> Result<(Vec<u8>, usize), Box<dyn std::error::Error>> {
    info!("[FUNC] build_version_message: Assembling payload.");
    let mut payload = Vec::new();
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    
    payload.write_all(&PROTOCOL_VERSION.to_le_bytes())?;
    payload.write_all(&SERVICES.to_le_bytes())?;
    payload.write_all(&now.to_le_bytes())?;
    payload.write_all(&[0u8; 26])?; // addr_recv
    payload.write_all(&[0u8; 26])?; // addr_from
    payload.write_all(&0u64.to_le_bytes())?; // nonce
    let user_agent = "/rust-sha2-p2p-client/";
    payload.write_all(&[user_agent.len() as u8])?; 
    payload.write_all(user_agent.as_bytes())?;
    payload.write_all(&0i32.to_le_bytes())?; // start_height
    payload.write_all(&[1])?; // relay
    
    let payload_len = payload.len();
    let checksum = calculate_checksum(&payload);
    
    let mut raw_message = Vec::new();
    raw_message.write_all(&MAGIC_BYTES)?;
    let mut command_bytes = [0u8; 12];
    command_bytes[0..7].copy_from_slice(b"version");
    raw_message.write_all(&command_bytes)?;
    raw_message.write_all(&(payload_len as u32).to_le_bytes())?;
    raw_message.write_all(&checksum)?;
    raw_message.write_all(&payload)?;
    
    println!("[FUNC] build_version_message: Done. Payload size: {}", payload_len);
    Ok((raw_message, payload_len))
}

pub fn build_verack_message() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    println!("[FUNC] build_verack_message: Assembling message (Zero payload).");
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
    
    println!("[FUNC] build_verack_message: Done.");
    Ok(raw_message)
}

pub fn build_mempool_message() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    println!("[FUNC] build_mempool_message: Assembling message (Zero payload).");
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
    
    println!("[FUNC] build_mempool_message: Done.");
    Ok(raw_message)
}

pub fn build_ping_message(nonce: [u8; 8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    println!("[FUNC] build_ping_message: Assembling payload.");
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
    
    println!("[FUNC] build_ping_message: Done.");
    Ok(raw_message)
}

pub fn build_pong_message(nonce: [u8; 8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    println!("[FUNC] build_pong_message: Assembling payload.");
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
    
    println!("[FUNC] build_pong_message: Done.");
    Ok(raw_message)
}

fn calculate_checksum(payload: &[u8]) -> [u8; 4] {
    println!("[FUNC] calculate_checksum: Hashing {} bytes.", payload.len());
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
    use super::*;
    use std::io::Cursor;
    use std::str;

    // FIX: Simplified helper to only perform the one essential read.
    fn perform_test_handshake<S: Read>(mut stream: S) -> Result<i32, Box<dyn std::error::Error>> {
        // 1. Client reads peer's 'version' message 
        let (header, payload) = read_message(&mut stream)?;
        let command = std::str::from_utf8(&header[4..16])?.trim_end_matches('\0');
        
        if command != "version" { 
            return Err(format!("Expected 'version', got '{}'.", command).into()); 
        }

        // Extract height (core test value)
        let mut offset: usize = 80; 
        let (user_agent_len, bytes_read) = payload.read_varint_and_advance(offset)?;
        offset += bytes_read + user_agent_len as usize; 
        let block_height_bytes: [u8; 4] = payload[offset..offset + 4].try_into().unwrap();
        let height = i32::from_le_bytes(block_height_bytes);
        println!("[DEBUG] Final extracted height: {}", height);
        
        // 2. Client reads peer's 'verack' message
        let (header, _) = read_message(&mut stream)?;
        let command = std::str::from_utf8(&header[4..16])?.trim_end_matches('\0');
        if command != "verack" { 
            return Err(format!("Expected 'verack', got '{}'.", command).into()); 
        }
        
        Ok(i32::from_le_bytes(block_height_bytes))
    }
    
    // --- Mock Data ---
    const MOCK_HEIGHT_LE: [u8; 4] = [0x60, 0x5c, 0x0c, 0x00]; // 810000
    const MOCK_USER_AGENT: &[u8] = b"/mock-test-client/"; 
    const MOCK_RELAY: [u8; 1] = [0x00]; 

    fn create_mock_version_payload_prefix() -> Vec<u8> {
        let mut payload_prefix = Vec::new();
        let now: i64 = 0; // Mock timestamp
        payload_prefix.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        payload_prefix.extend_from_slice(&SERVICES.to_le_bytes());
        payload_prefix.extend_from_slice(&now.to_le_bytes());
        payload_prefix.extend_from_slice(&[0u8; 26]); // addr_recv
        payload_prefix.extend_from_slice(&[0u8; 26]); // addr_from
        payload_prefix.extend_from_slice(&0u64.to_le_bytes()); // nonce
        payload_prefix
    }

    fn create_mock_verack_response() -> Vec<u8> {
        let payload = Vec::new();
        let mut msg = Vec::new();
        msg.extend_from_slice(&MAGIC_BYTES);
        let mut cmd = [0u8; 12];
        cmd[0..6].copy_from_slice(b"verack");
        msg.extend_from_slice(&cmd);
        msg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        msg.extend_from_slice(&calculate_checksum(&payload));
        println!("[DEBUG] create_mock_verack_response: message length = {}", msg.len());
        msg
    }

    fn create_mock_version_response() -> Vec<u8> {
        let mut payload = create_mock_version_payload_prefix();
        payload.push(MOCK_USER_AGENT.len() as u8);
        payload.extend_from_slice(MOCK_USER_AGENT);
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
        println!("[DEBUG] create_mock_version_response: message length = {}", msg.len());
        msg
    }

    #[test]
    fn test_mock_seeder_handshake_and_height_check() -> Result<(), Box<dyn std::error::Error>> {
        let mock_version = create_mock_version_response();
        let mock_verack = create_mock_verack_response();
        
        let mut mock_response = Vec::new();
        mock_response.extend_from_slice(&mock_version);
        mock_response.extend_from_slice(&mock_verack);
        println!("[DEBUG] test_mock_seeder_handshake_and_height_check: mock_response total length = {}", mock_response.len());

        let mock_stream = Cursor::new(mock_response);

        let height = perform_test_handshake(mock_stream)?;

        assert_eq!(height, 810080);
        Ok(())
    }

    #[test]
    fn test_varint_decoding_robust() -> Result<(), Box<dyn std::error::Error>> {
        let payload_2b: [u8; 3] = [0xfd, 0x00, 0x01];
        assert_eq!(payload_2b.read_varint_and_advance(0)?, (256, 3));
        let payload_4b: [u8; 5] = [0xfe, 0x00, 0x00, 0x01, 0x00];
        assert_eq!(payload_4b.read_varint_and_advance(0)?, (65536, 5));
        Ok(())
    }

    #[test]
    fn test_mempool_message_encoding() -> Result<(), Box<dyn std::error::Error>> {
        let mempool_msg = build_mempool_message()?;
        // Check command name
        assert_eq!(str::from_utf8(&mempool_msg[4..11])?, "mempool");
        // Check payload length (should be 0)
        assert_eq!(u32::from_le_bytes(mempool_msg[16..20].try_into().unwrap()), 0);
        // Check total length (24 header)
        assert_eq!(mempool_msg.len(), 24);
        Ok(())
    }
}
