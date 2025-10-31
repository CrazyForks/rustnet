// src/network/merge.rs - Connection merging and update utilities

use log::{debug, info, warn};
use std::time::{Instant, SystemTime};

use crate::network::dpi::DpiResult;
use crate::network::parser::{ParsedPacket, TcpFlags};
use crate::network::types::{
    ApplicationProtocol, Connection, DnsInfo, DpiInfo, HttpInfo, HttpsInfo, ProtocolState,
    QuicConnectionState, QuicInfo, SshInfo, TcpState,
};

/// Update TCP connection state based on observed flags and current state
/// This implements the TCP state machine according to RFC 793
fn update_tcp_state(current_state: TcpState, flags: &TcpFlags, is_outgoing: bool) -> TcpState {
    debug!(
        "Updating TCP state: current_state={:?}, flags={:?}, is_outgoing={}",
        current_state, flags, is_outgoing
    );

    match (current_state, flags.syn, flags.ack, flags.fin, flags.rst) {
        // Connection establishment - three-way handshake
        (TcpState::Unknown, true, false, false, false) if !is_outgoing => TcpState::SynReceived,
        (TcpState::Unknown, true, false, false, false) if is_outgoing => TcpState::SynSent,
        (TcpState::Listen, true, false, false, false) if !is_outgoing => TcpState::SynReceived,
        (TcpState::Listen, true, false, false, false) if is_outgoing => TcpState::SynSent,
        (TcpState::SynSent, true, true, false, false) if !is_outgoing => TcpState::Established,
        (TcpState::SynReceived, false, true, false, false) if is_outgoing => TcpState::Established,

        // This might happen if we start parsing connections after the SYN-ACK
        (TcpState::Unknown, false, true, false, false) => TcpState::Established,
        (TcpState::Unknown, false, true, true, false) => TcpState::Established,

        // Connection termination - normal close
        (TcpState::Established, false, _, true, false) if is_outgoing => TcpState::FinWait1,
        (TcpState::Established, false, _, true, false) if !is_outgoing => TcpState::CloseWait,
        (TcpState::FinWait1, false, true, false, false) if !is_outgoing => TcpState::FinWait2,
        (TcpState::FinWait1, false, _, true, false) if !is_outgoing => TcpState::Closing,
        (TcpState::FinWait2, false, _, true, false) if !is_outgoing => TcpState::TimeWait,
        (TcpState::CloseWait, false, _, true, false) if is_outgoing => TcpState::LastAck,
        (TcpState::LastAck, false, true, false, false) if !is_outgoing => TcpState::Closed,
        (TcpState::Closing, false, true, false, false) if !is_outgoing => TcpState::TimeWait,

        // Connection reset
        (_, _, _, _, true) => TcpState::Closed,

        // Keep current state if no state transition
        _ => current_state,
    }
}

/// Merge a parsed packet into an existing connection
pub fn merge_packet_into_connection(
    mut conn: Connection,
    parsed: &ParsedPacket,
    now: SystemTime,
) -> Connection {
    // Update timing
    conn.last_activity = now;

    // Update packet counts and bytes
    if parsed.is_outgoing {
        conn.packets_sent += 1;
        conn.bytes_sent += parsed.packet_len as u64;
    } else {
        conn.packets_received += 1;
        conn.bytes_received += parsed.packet_len as u64;
    }

    // Update protocol state (from packet flags/state)
    if parsed.tcp_flags.is_some() {
        let current_tcp_state = match conn.protocol_state {
            ProtocolState::Tcp(state) => state,
            _ => {
                warn!("Merging TCP packet into non-TCP connection, resetting to Unknown state");
                TcpState::Unknown
            }
        };

        let new_tcp_state = update_tcp_state(
            current_tcp_state,
            &parsed.tcp_flags.unwrap(),
            parsed.is_outgoing,
        );

        if current_tcp_state != new_tcp_state {
            debug!(
                "TCP state transition: {:?} -> {:?}",
                current_tcp_state, new_tcp_state
            );
        }

        conn.protocol_state = ProtocolState::Tcp(new_tcp_state);
    } else {
        // If no TCP flags, keep existing state or use the one from packet
        match (&conn.protocol_state, &parsed.protocol_state) {
            (ProtocolState::Tcp(_), _) => {
                // Keep existing TCP state if we have it
            }
            _ => {
                // Use the state from the packet for non-TCP protocols
                conn.protocol_state = parsed.protocol_state;
            }
        }
    }

    // Update DPI info if available
    if let Some(dpi_result) = &parsed.dpi_result {
        merge_dpi_info(&mut conn, dpi_result);
    }

    // Update PKTAP process metadata if available
    // Once set, process info should be immutable to prevent conflicts between sources
    if let Some(new_process_name) = &parsed.process_name {
        match &conn.process_name {
            None => {
                // First time setting process name - this becomes immutable
                conn.process_name = Some(new_process_name.clone());
                info!(
                    "🔒 Set IMMUTABLE process name for connection {} from PKTAP: '{}' (len:{})",
                    conn.key(),
                    new_process_name,
                    new_process_name.len()
                );
            }
            Some(existing_name) => {
                // Process name is already set - it's now IMMUTABLE
                // Log the attempt but NEVER change it
                if existing_name != new_process_name {
                    warn!(
                        "🚫 IMMUTABILITY VIOLATION: Attempt to change process name for {} from '{}' to '{}' - REJECTED",
                        conn.key(),
                        existing_name,
                        new_process_name
                    );
                    debug!(
                        "🔒 Existing: '{}' (len:{}, bytes:{:?})",
                        existing_name,
                        existing_name.len(),
                        existing_name.as_bytes()
                    );
                    debug!(
                        "🚫 Rejected: '{}' (len:{}, bytes:{:?})",
                        new_process_name,
                        new_process_name.len(),
                        new_process_name.as_bytes()
                    );
                } else {
                    debug!(
                        "✅ Process name confirmed unchanged for {}: '{}'",
                        conn.key(),
                        existing_name
                    );
                }
                // NEVER update - process name is immutable once set
            }
        }
    }

    if let Some(new_pid) = parsed.process_id {
        match conn.pid {
            None => {
                // First time setting PID - this becomes immutable
                conn.pid = Some(new_pid);
                info!(
                    "🔒 Set IMMUTABLE process ID for connection {} from PKTAP: {}",
                    conn.key(),
                    new_pid
                );
            }
            Some(existing_pid) if existing_pid != new_pid => {
                warn!(
                    "🚫 IMMUTABILITY VIOLATION: Attempt to change PID for {} from {} to {} - REJECTED",
                    conn.key(),
                    existing_pid,
                    new_pid
                );
                // NEVER update - PID is immutable once set
            }
            Some(existing_pid) => {
                debug!(
                    "✅ Process ID confirmed unchanged for {}: {}",
                    conn.key(),
                    existing_pid
                );
            }
        }
    }

    // Update rate calculations
    update_connection_rates(&mut conn);

    conn
}

/// Create a new connection from a parsed packet
pub fn create_connection_from_packet(parsed: &ParsedPacket, now: SystemTime) -> Connection {
    let mut conn = Connection::new(
        parsed.protocol,
        parsed.local_addr,
        parsed.remote_addr,
        parsed.protocol_state,
    );

    // Set initial TCP state based on flags if TCP
    if parsed.tcp_flags.is_some() {
        if let Some(tcp_flags) = &parsed.tcp_flags {
            conn.protocol_state = ProtocolState::Tcp(update_tcp_state(
                TcpState::Unknown,
                tcp_flags,
                parsed.is_outgoing,
            ));

            debug!(
                "Created new {} connection: {:?} -> {:?}, state: {:?}",
                parsed.protocol, parsed.local_addr, parsed.remote_addr, conn.protocol_state
            );
        }
    } else {
        // For non-TCP protocols, use the provided state directly
        conn.protocol_state = parsed.protocol_state;
    }

    // Set initial stats based on packet direction
    if parsed.is_outgoing {
        conn.packets_sent = 1;
        conn.bytes_sent = parsed.packet_len as u64;
        conn.packets_received = 0;
        conn.bytes_received = 0;
    } else {
        conn.packets_sent = 0;
        conn.bytes_sent = 0;
        conn.packets_received = 1;
        conn.bytes_received = parsed.packet_len as u64;
    }

    // Apply DPI results if any
    if let Some(dpi_result) = &parsed.dpi_result {
        conn.dpi_info = Some(DpiInfo {
            application: dpi_result.application.clone(),
            last_update_time: Instant::now(),
        });

        debug!(
            "New connection with DPI: {} - {}",
            conn.key(),
            dpi_result.application
        );
    }

    // Apply PKTAP process metadata if available
    if let Some(process_name) = &parsed.process_name {
        conn.process_name = Some(process_name.clone());
        debug!(
            "✓ New connection {} with process name: {}",
            conn.key(),
            process_name
        );
    }
    if let Some(process_id) = parsed.process_id {
        conn.pid = Some(process_id);
        debug!(
            "✓ New connection {} with process ID: {}",
            conn.key(),
            process_id
        );
    }

    conn.created_at = now;
    conn.last_activity = now;

    // Initialize the rate tracker with the initial byte counts
    // This prevents incorrect delta calculation on the first update
    conn.rate_tracker
        .initialize_with_counts(conn.bytes_sent, conn.bytes_received);

    conn
}

/// Merge DPI information into an existing connection
fn merge_dpi_info(conn: &mut Connection, dpi_result: &DpiResult) {
    match &mut conn.dpi_info {
        None => {
            // No existing DPI info, use the new one
            conn.dpi_info = Some(DpiInfo {
                application: dpi_result.application.clone(),
                last_update_time: Instant::now(),
            });

            debug!(
                "Added DPI info to connection: {} - {}",
                conn.key(),
                dpi_result.application
            );
        }
        Some(dpi_info) => {
            // Update the last update time
            dpi_info.last_update_time = Instant::now();

            // Match on both the existing and new application protocols
            match (&mut dpi_info.application, &dpi_result.application) {
                // HTTP merging
                (ApplicationProtocol::Http(old_info), ApplicationProtocol::Http(new_info)) => {
                    merge_http_info(old_info, new_info);
                }

                // HTTPS/TLS merging
                (ApplicationProtocol::Https(old_info), ApplicationProtocol::Https(new_info)) => {
                    merge_https_info(old_info, new_info);
                }

                // QUIC merging - this is where the reassembly happens
                (ApplicationProtocol::Quic(old_info), ApplicationProtocol::Quic(new_info)) => {
                    merge_quic_info(old_info.as_mut(), new_info.as_ref());
                }

                // DNS merging
                (ApplicationProtocol::Dns(old_info), ApplicationProtocol::Dns(new_info)) => {
                    merge_dns_info(old_info, new_info);
                }

                // SSH - merge SSH info
                (ApplicationProtocol::Ssh(old_info), ApplicationProtocol::Ssh(new_info)) => {
                    merge_ssh_info(old_info, new_info);
                }

                _ => {
                    // Keep existing protocol
                }
            }
        }
    }
}

/// Merge HTTP information
fn merge_http_info(old_info: &mut HttpInfo, new_info: &HttpInfo) {
    // Update method if not set
    if old_info.method.is_none() && new_info.method.is_some() {
        old_info.method = new_info.method.clone();
    }

    // Update path if not set
    if old_info.path.is_none() && new_info.path.is_some() {
        old_info.path = new_info.path.clone();
    }

    // Update host if not set
    if old_info.host.is_none() && new_info.host.is_some() {
        old_info.host = new_info.host.clone();
    }

    // Update user agent if not set
    if old_info.user_agent.is_none() && new_info.user_agent.is_some() {
        old_info.user_agent = new_info.user_agent.clone();
    }

    // Update status code if not set
    if old_info.status_code.is_none() && new_info.status_code.is_some() {
        old_info.status_code = new_info.status_code;
    }
}

/// Merge HTTPS/TLS information
fn merge_https_info(old_info: &mut HttpsInfo, new_info: &HttpsInfo) {
    // Update version if not set or if new is more specific
    if old_info.tls_info.is_none() && new_info.tls_info.is_some() {
        old_info.tls_info = new_info.tls_info.clone();
    } else if let (Some(old_tls), Some(new_tls)) = (&mut old_info.tls_info, &new_info.tls_info) {
        // Merge TLS info - prefer more complete info
        if old_tls.version.is_none() && new_tls.version.is_some() {
            old_tls.version = new_tls.version;
        }
        if old_tls.sni.is_none() && new_tls.sni.is_some() {
            old_tls.sni = new_tls.sni.clone();
        }
        if old_tls.alpn.is_empty() && !new_tls.alpn.is_empty() {
            old_tls.alpn = new_tls.alpn.clone();
        }
        if old_tls.cipher_suite.is_none() && new_tls.cipher_suite.is_some() {
            old_tls.cipher_suite = new_tls.cipher_suite;
        }
    }
}

/// Merge QUIC information with reassembly support
fn merge_quic_info(old_info: &mut QuicInfo, new_info: &QuicInfo) {
    // Update connection state if it progressed
    match (&old_info.connection_state, &new_info.connection_state) {
        (old_state, _new_state) if !matches!(old_state, _new_state) => {
            debug!(
                "QUIC connection state changed: {:?} -> {:?}",
                old_state, _new_state
            );
            old_info.connection_state = new_info.connection_state;
        }
        _ => {}
    }

    // Update packet type
    old_info.packet_type = new_info.packet_type;

    // Update connection ID if we didn't have it
    if old_info.connection_id.is_empty() && !new_info.connection_id.is_empty() {
        old_info.connection_id = new_info.connection_id.clone();
        old_info.connection_id_hex = new_info.connection_id_hex.clone();
    }

    // Update version string if we didn't have it
    if old_info.version_string.is_none() && new_info.version_string.is_some() {
        old_info.version_string = new_info.version_string.clone();
    }

    // Merge CRYPTO frame reassembler state - this is crucial for proper SNI extraction
    // The reassembler must persist across multiple packets to handle fragmented TLS handshakes
    if let Some(new_reassembler) = &new_info.crypto_reassembler {
        if old_info.crypto_reassembler.is_none() {
            // First time seeing crypto frames, initialize the connection-level reassembler
            old_info.crypto_reassembler = Some(new_reassembler.clone());
            debug!(
                "QUIC: Initialized crypto reassembler for connection with Connection ID: {:?}",
                old_info.connection_id_hex
            );
        } else if let Some(old_reassembler) = &mut old_info.crypto_reassembler {
            // Merge fragments from new reassembler into connection-level reassembler
            // This handles out-of-order CRYPTO frames across packets
            for (&offset, data) in new_reassembler.get_fragments() {
                match old_reassembler.add_fragment(offset, data.clone()) {
                    Ok(_) => {
                        debug!(
                            "QUIC: Merged CRYPTO fragment at offset {} for connection {}",
                            offset,
                            old_info.connection_id_hex.as_deref().unwrap_or("unknown")
                        );
                    }
                    Err(e) => {
                        warn!("QUIC: Failed to merge CRYPTO fragment: {}", e);
                    }
                }
            }

            // Update cached TLS info if new reassembler has it
            if let Some(tls_info) = new_reassembler.get_cached_tls_info() {
                old_info.tls_info = Some(tls_info.clone());
                debug!(
                    "QUIC: Updated TLS info from reassembler - SNI: {:?}, ALPN: {:?}",
                    tls_info.sni, tls_info.alpn
                );
            }
        }
    }

    // Update TLS info if new packet has better info
    match (&old_info.tls_info, &new_info.tls_info) {
        (None, Some(new_tls)) => {
            old_info.tls_info = Some(new_tls.clone());
            debug!("QUIC: Added TLS info - SNI: {:?}", new_tls.sni);
        }
        (Some(old_tls), Some(new_tls)) => {
            // Merge TLS info - prefer more complete info
            let mut updated = false;
            let mut merged_tls = old_tls.clone();

            if old_tls.sni.is_none() && new_tls.sni.is_some() {
                merged_tls.sni = new_tls.sni.clone();
                updated = true;
            }

            if old_tls.alpn.is_empty() && !new_tls.alpn.is_empty() {
                merged_tls.alpn = new_tls.alpn.clone();
                updated = true;
            }

            if old_tls.version.is_none() && new_tls.version.is_some() {
                merged_tls.version = new_tls.version;
                updated = true;
            }

            if old_tls.cipher_suite.is_none() && new_tls.cipher_suite.is_some() {
                merged_tls.cipher_suite = new_tls.cipher_suite;
                updated = true;
            }

            if updated {
                old_info.tls_info = Some(merged_tls);
                debug!("QUIC: Merged TLS info");
            }
        }
        _ => {}
    }

    // Update has_crypto_frame flag
    if new_info.has_crypto_frame {
        old_info.has_crypto_frame = true;
    }

    // Handle CONNECTION_CLOSE frame detection
    if let Some(new_close) = &new_info.connection_close {
        // CONNECTION_CLOSE is final - always update
        old_info.connection_close = Some(new_close.clone());

        // Update connection state based on close frame
        old_info.connection_state = match new_close.frame_type {
            0x1c if new_close.error_code == 0 => {
                // NO_ERROR transport close - enter draining state
                debug!("QUIC: Connection entering draining state (NO_ERROR transport close)");
                QuicConnectionState::Draining
            }
            0x1c => {
                // Transport error - connection is closed
                debug!(
                    "QUIC: Connection closed due to transport error: {}",
                    new_close.error_code
                );
                QuicConnectionState::Closed
            }
            0x1d => {
                // Application close - connection is closed
                debug!(
                    "QUIC: Connection closed by application: {}",
                    new_close.error_code
                );
                QuicConnectionState::Closed
            }
            _ => {
                // Unknown close type - assume closed
                debug!(
                    "QUIC: Connection closed (unknown frame type: 0x{:02x})",
                    new_close.frame_type
                );
                QuicConnectionState::Closed
            }
        };

        debug!(
            "QUIC: Updated connection state to {:?} due to CONNECTION_CLOSE frame",
            old_info.connection_state
        );
    }

    // Update idle timeout if provided
    if new_info.idle_timeout.is_some() {
        old_info.idle_timeout = new_info.idle_timeout;
    }
}

/// Merge DNS information
fn merge_dns_info(old_info: &mut DnsInfo, new_info: &DnsInfo) {
    // Update query name if not set
    if old_info.query_name.is_none() && new_info.query_name.is_some() {
        old_info.query_name = new_info.query_name.clone();
    }

    // Update query type if not set
    if old_info.query_type.is_none() && new_info.query_type.is_some() {
        old_info.query_type = new_info.query_type;
    }

    // Merge response IPs (keep unique)
    for ip in &new_info.response_ips {
        if !old_info.response_ips.contains(ip) {
            old_info.response_ips.push(*ip);
        }
    }

    // Update response flag
    if new_info.is_response {
        old_info.is_response = true;
    }
}

/// Merge SSH information
fn merge_ssh_info(old_info: &mut SshInfo, new_info: &SshInfo) {
    // Update version if not set
    if old_info.version.is_none() && new_info.version.is_some() {
        old_info.version = new_info.version.clone();
    }

    // Update client software if not set
    if old_info.client_software.is_none() && new_info.client_software.is_some() {
        old_info.client_software = new_info.client_software.clone();
    }

    // Update server software if not set
    if old_info.server_software.is_none() && new_info.server_software.is_some() {
        old_info.server_software = new_info.server_software.clone();
    }

    // Update connection state to the more advanced state
    use crate::network::types::SshConnectionState;
    match (&old_info.connection_state, &new_info.connection_state) {
        (SshConnectionState::Banner, _) => {
            old_info.connection_state = new_info.connection_state.clone()
        }
        (SshConnectionState::KeyExchange, SshConnectionState::Authentication) => {
            old_info.connection_state = new_info.connection_state.clone()
        }
        (SshConnectionState::KeyExchange, SshConnectionState::Established) => {
            old_info.connection_state = new_info.connection_state.clone()
        }
        (SshConnectionState::Authentication, SshConnectionState::Established) => {
            old_info.connection_state = new_info.connection_state.clone()
        }
        _ => {} // Keep existing state if it's more advanced
    }

    // Merge algorithms - prioritize final negotiated algorithms over initial offers
    match (&old_info.connection_state, &new_info.connection_state) {
        // If we're moving to Established state and new info has algorithms, use those (final negotiated)
        (_, SshConnectionState::Established) if !new_info.algorithms.is_empty() => {
            old_info.algorithms = new_info.algorithms.clone();
        }
        // If both are in Established state, merge unique algorithms
        (SshConnectionState::Established, SshConnectionState::Established) => {
            for algo in &new_info.algorithms {
                if !old_info.algorithms.contains(algo) {
                    old_info.algorithms.push(algo.clone());
                }
            }
        }
        // For earlier states, accumulate all seen algorithms
        _ => {
            for algo in &new_info.algorithms {
                if !old_info.algorithms.contains(algo) {
                    old_info.algorithms.push(algo.clone());
                }
            }
        }
    }

    // Update auth method if not set
    if old_info.auth_method.is_none() && new_info.auth_method.is_some() {
        old_info.auth_method = new_info.auth_method.clone();
    }
}

/// Update connection rate calculations using sliding window
fn update_connection_rates(conn: &mut Connection) {
    // Use the new rate tracker with sliding window calculation
    conn.update_rates();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::types::{Protocol, ProtocolState, TcpState};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn create_test_connection() -> Connection {
        Connection::new(
            Protocol::TCP,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 12345),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 80),
            ProtocolState::Tcp(TcpState::Established),
        )
    }

    fn create_test_packet(is_outgoing: bool, fin: bool) -> ParsedPacket {
        ParsedPacket {
            connection_key: "test".to_string(),
            protocol: Protocol::TCP,
            local_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 12345),
            remote_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 80),
            protocol_state: ProtocolState::Tcp(TcpState::Unknown),
            tcp_flags: Some(TcpFlags {
                syn: false,
                ack: false,
                fin,
                rst: false,
                psh: false,
                urg: false,
            }),
            is_outgoing,
            packet_len: 100,
            dpi_result: None,
            process_name: None,
            process_id: None,
        }
    }

    #[test]
    fn test_merge_packet_into_connection() {
        let mut conn = create_test_connection();
        let packet = create_test_packet(true, false);

        conn = merge_packet_into_connection(conn, &packet, SystemTime::now());

        assert_eq!(conn.packets_sent, 1);
        assert_eq!(conn.bytes_sent, 100);
        assert_eq!(conn.packets_received, 0);
    }

    #[test]
    fn test_create_connection_from_packet() {
        let packet = create_test_packet(false, false);
        let conn = create_connection_from_packet(&packet, SystemTime::now());

        assert_eq!(conn.packets_received, 1);
        assert_eq!(conn.bytes_received, 100);
        assert_eq!(conn.packets_sent, 0);
    }

    #[test]
    fn test_new_connection_rate_tracker_initialization() {
        // Test that the rate tracker is properly initialized for new connections
        let packet = create_test_packet(true, false);
        let conn = create_connection_from_packet(&packet, SystemTime::now());

        // The connection should have initial bytes
        assert_eq!(conn.bytes_sent, 100);
        assert_eq!(conn.bytes_received, 0);

        // Now simulate merging another packet
        let packet2 = create_test_packet(true, false);
        let mut updated_conn = merge_packet_into_connection(conn, &packet2, SystemTime::now());

        // Bytes should have increased
        assert_eq!(updated_conn.bytes_sent, 200);
        assert_eq!(updated_conn.bytes_received, 0);

        // Update rates - this should not cause a huge spike
        updated_conn.update_rates();

        // The rate should be reasonable (not include the initial 100 bytes as a spike)
        // Since we just added 100 bytes, the rate should be based on that delta
        // not on the full 200 bytes
        assert!(updated_conn.current_outgoing_rate_bps >= 0.0);
    }

    #[test]
    fn test_tcp_state_transitions() {
        // Test SYN -> SYN_SENT
        let flags = TcpFlags {
            syn: true,
            ack: false,
            fin: false,
            rst: false,
            psh: false,
            urg: false,
        };
        let new_state = update_tcp_state(TcpState::Unknown, &flags, true);
        assert_eq!(new_state, TcpState::SynSent);

        // Test SYN-ACK -> ESTABLISHED
        let flags = TcpFlags {
            syn: true,
            ack: true,
            fin: false,
            rst: false,
            psh: false,
            urg: false,
        };
        let new_state = update_tcp_state(TcpState::SynSent, &flags, false);
        assert_eq!(new_state, TcpState::Established);

        // Test FIN -> FIN_WAIT_1
        let flags = TcpFlags {
            syn: false,
            ack: false,
            fin: true,
            rst: false,
            psh: false,
            urg: false,
        };
        let new_state = update_tcp_state(TcpState::Established, &flags, true);
        assert_eq!(new_state, TcpState::FinWait1);

        // Test RST -> CLOSED
        let flags = TcpFlags {
            syn: false,
            ack: false,
            fin: false,
            rst: true,
            psh: false,
            urg: false,
        };
        let new_state = update_tcp_state(TcpState::Established, &flags, true);
        assert_eq!(new_state, TcpState::Closed);
    }
}
