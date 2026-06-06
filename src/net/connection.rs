use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;
use tracing::debug;

use crate::net::game_protocol::ProtocolGame;
use crate::net::login::ProtocolLogin;
use crate::net::message::{NetworkMessage, NETWORK_MESSAGE_MAXSIZE};
use crate::net::protocol::xtea_decrypt_incoming;
use crate::net::server::{ProtocolKind, ServiceInfo};
use crate::net::status::ProtocolStatus;
use crate::util::adler_checksum;

const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PACKETS_PER_SECOND: u32 = 25;

#[derive(Clone)]
pub struct ConnectionHandle {
    write_tx: UnboundedSender<Vec<u8>>,
    close: Arc<Notify>,
    pub peer_addr: SocketAddr,
}

impl ConnectionHandle {
    pub fn send_bytes(&self, bytes: Vec<u8>) {
        let _ = self.write_tx.send(bytes);
    }

    pub fn disconnect(&self) {
        self.close.notify_waiters();
        let _ = self.write_tx.send(vec![]);
    }

    pub fn peer_addr_u32(&self) -> u32 {
        match self.peer_addr.ip() {
            std::net::IpAddr::V4(ip) => u32::from_le_bytes(ip.octets()),
            _ => 0,
        }
    }
}

enum AnyProtocol {
    Status(ProtocolStatus),
    Login(ProtocolLogin),
    Game(ProtocolGame),
}

pub async fn run_connection(stream: TcpStream, services: Arc<Vec<ServiceInfo>>) {
    let peer_addr = match stream.peer_addr() {
        Ok(a) => a,
        Err(_) => return,
    };

    let (read_half, write_half) = stream.into_split();
    let (write_tx, write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let close = Arc::new(Notify::new());

    let conn = ConnectionHandle {
        write_tx,
        close: Arc::clone(&close),
        peer_addr,
    };

    tokio::spawn(write_task(write_half, write_rx));

    connection_loop(read_half, conn, services, close).await;
}

async fn write_task(mut write_half: OwnedWriteHalf, mut rx: UnboundedReceiver<Vec<u8>>) {
    while let Some(bytes) = rx.recv().await {
        if bytes.is_empty() {
            break;
        }
        let result =
            tokio::time::timeout(WRITE_TIMEOUT, write_half.write_all(&bytes)).await;
        if result.is_err() || result.unwrap().is_err() {
            break;
        }
    }
}

async fn connection_loop(
    mut read_half: OwnedReadHalf,
    conn: ConnectionHandle,
    services: Arc<Vec<ServiceInfo>>,
    close: Arc<Notify>,
) {
    let mut protocol: Option<AnyProtocol> = None;
    let mut first_message = true;
    let mut packet_count = 0u32;
    let mut window_start = Instant::now();

    // For server_sends_first: create and initialise the game protocol before reading.
    for svc in services.iter() {
        if svc.server_sends_first {
            let mut game = ProtocolGame::new(svc.checksummed);
            game.on_connect(&conn);
            protocol = Some(AnyProtocol::Game(game));
            break;
        }
    }

    loop {
        let mut header = [0u8; 2];
        let read_result = tokio::select! {
            _ = close.notified() => break,
            r = tokio::time::timeout(READ_TIMEOUT, read_half.read_exact(&mut header)) => r,
        };
        match read_result {
            Err(_) | Ok(Err(_)) => break,
            Ok(Ok(_)) => {}
        }

        let packet_len = u16::from_le_bytes(header) as usize;
        if packet_len == 0 || packet_len > NETWORK_MESSAGE_MAXSIZE - 2 {
            break;
        }

        // Rate limit
        packet_count += 1;
        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            packet_count = 1;
            window_start = Instant::now();
        } else if packet_count > MAX_PACKETS_PER_SECOND {
            debug!("rate limit exceeded for {}", conn.peer_addr);
            break;
        }

        let mut msg = NetworkMessage::new();
        {
            let body = msg.body_buffer_mut();
            let read_result = tokio::select! {
                _ = close.notified() => break,
                r = tokio::time::timeout(READ_TIMEOUT, read_half.read_exact(&mut body[..packet_len])) => r,
            };
            match read_result {
                Err(_) | Ok(Err(_)) => break,
                Ok(Ok(_)) => {}
            }
        }
        msg.set_length(packet_len as u16);

        if first_message {
            first_message = false;

            let any_checksummed = services.iter().any(|s| s.checksummed);
            let mut checksummed = false;
            if any_checksummed {
                let recv_checksum = msg.get_u32();
                let pos = msg.get_buffer_position() as usize;
                let end = 2 + packet_len;
                if pos <= end {
                    let computed = adler_checksum(&msg.buffer()[pos..end]);
                    if recv_checksum == computed {
                        checksummed = true;
                    } else {
                        msg.skip_bytes(-4);
                    }
                } else {
                    msg.skip_bytes(-4);
                }
            }

            // If protocol pre-assigned (server_sends_first), skip the placeholder protocol ID byte.
            if protocol.is_none() {
                let protocol_id = msg.get_byte();
                let matched = services.iter().find(|s| {
                    s.protocol_id == protocol_id && (!s.checksummed || checksummed)
                });
                let Some(svc) = matched else { break };
                protocol = Some(match svc.kind {
                    ProtocolKind::Status => AnyProtocol::Status(ProtocolStatus::new()),
                    ProtocolKind::Login => {
                        AnyProtocol::Login(ProtocolLogin::new(svc.checksummed))
                    }
                    ProtocolKind::Game => {
                        let mut g = ProtocolGame::new(svc.checksummed);
                        g.on_connect(&conn);
                        AnyProtocol::Game(g)
                    }
                });
            } else {
                // Skip the protocol ID placeholder byte (e.g. 0x00 for game protocol).
                msg.skip_bytes(1);
            }

            match protocol.as_mut() {
                Some(AnyProtocol::Status(p)) => p.on_recv_first_message(&mut msg, &conn),
                Some(AnyProtocol::Login(p)) => p.on_recv_first_message(&mut msg, &conn),
                Some(AnyProtocol::Game(p)) => p.on_recv_first_message(&mut msg, &conn).await,
                None => break,
            }
        } else {
            match protocol.as_mut() {
                Some(AnyProtocol::Game(p)) => {
                    // Verify checksum on subsequent packets.
                    if p.checksummed {
                        let recv_checksum = msg.get_u32();
                        let pos = msg.get_buffer_position() as usize;
                        let end = 2 + packet_len;
                        if pos <= end {
                            let computed = adler_checksum(&msg.buffer()[pos..end]);
                            if recv_checksum != computed {
                                msg.skip_bytes(-4);
                            }
                        } else {
                            msg.skip_bytes(-4);
                        }
                    }
                    if p.crypto.encryption_enabled {
                        if let Some(ref rk) = p.crypto.round_keys {
                            let rk = **rk;
                            if !xtea_decrypt_incoming(&mut msg, &rk) {
                                break;
                            }
                        }
                    }
                    p.on_recv_message(&mut msg, &conn).await;
                }
                _ => break,
            }
        }
    }
}
