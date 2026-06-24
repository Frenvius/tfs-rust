use crate::config::{g_config, BooleanConfig, IntegerConfig, StringConfig};
use crate::db::login::{loginserver_authentication, LoginResult};
use crate::game::{g_game, GameState};
use crate::net::connection::ConnectionHandle;
use crate::net::message::NetworkMessage;
use crate::net::output_message::OutputMessage;
use crate::net::protocol::{rsa_decrypt, ProtocolCrypto};
use crate::net::protocol_version::client_version;

pub struct ProtocolLogin {
    pub crypto: ProtocolCrypto,
}

impl ProtocolLogin {
    pub fn new(checksummed: bool) -> Self {
        Self {
            crypto: ProtocolCrypto::new(checksummed),
        }
    }

    pub fn on_recv_first_message(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let is_1098 = client_version().is_1098();
        let version_min = client_version().min_version();
        let version_max = client_version().max_version();

        msg.skip_bytes(2); // OS version
        let version = msg.get_u16();
        tracing::info!(version, is_1098, "login: got version");

        if is_1098 {
            // clientVersion(4) + contentRevision(2) + 0(2) + sprSig(4) + picSig(4) + previewState(1) = 17
            msg.skip_bytes(17);
        } else {
            msg.skip_bytes(12); // dat/spr/pic signatures
        }

        if version <= 760 {
            tracing::info!("login: version too old");
            self.disconnect_client(conn, "Only clients with protocol 8.60 allowed!");
            return;
        }

        if !rsa_decrypt(msg) {
            tracing::info!("login: RSA decrypt failed");
            conn.disconnect();
            return;
        }
        tracing::info!("login: RSA ok");

        let key: [u32; 4] = [msg.get_u32(), msg.get_u32(), msg.get_u32(), msg.get_u32()];
        self.crypto.enable_xtea(&key);

        if version < version_min || version > version_max {
            self.disconnect_client(
                conn,
                &format!(
                    "Only clients with protocol {}.{} allowed!",
                    version_min / 100,
                    version_min % 100
                ),
            );
            return;
        }

        {
            let game_state = g_game().lock().unwrap().get_game_state();
            if game_state == GameState::Startup {
                self.disconnect_client(conn, "Gameworld is starting up. Please wait.");
                return;
            }
            if game_state == GameState::Maintain {
                self.disconnect_client(conn, "Gameworld is under maintenance. Please re-connect in a while.");
                return;
            }
            if game_state == GameState::Closed {
                self.disconnect_client(conn, "Server is currently closed.");
                return;
            }
        }

        let account_name_bytes = msg.get_string(None);
        let account_name = String::from_utf8_lossy(&account_name_bytes).into_owned();
        let password_bytes = msg.get_string(None);
        let password = String::from_utf8_lossy(&password_bytes).into_owned();
        tracing::info!(account = %account_name, "login: credentials read");

        let checksummed = self.crypto.checksum_enabled;
        let round_keys = self.crypto.round_keys.as_ref().map(|rk| **rk);
        let conn = conn.clone();

        tokio::spawn(async move {
            match loginserver_authentication(&account_name, &password).await {
                LoginResult::Success(account) => {
                    tracing::info!(chars = account.characters.len(), "login: auth success");
                    let config = g_config();
                    let server_name = config.get_string(StringConfig::ServerName).to_owned();
                    let motd = config.get_string(StringConfig::Motd).to_owned();
                    let free_premium = config.get_boolean(BooleanConfig::FreePremium);
                    let motd_num = g_game().lock().unwrap().get_motd_num();

                    let mut crypto = ProtocolCrypto::new(checksummed);
                    if let Some(rk) = round_keys {
                        crypto.round_keys = Some(Box::new(rk));
                        crypto.encryption_enabled = true;
                    }

                    let mut output = OutputMessage::new();

                    if is_1098 {
                        // Session key packet (0x28)
                        output.add_byte(0x28);
                        let session_key = format!("{account_name}\n{password}");
                        output.add_string(session_key.as_bytes());

                        // MOTD (0x14) — same as 8.60
                        if !motd.is_empty() {
                            output.add_byte(0x14);
                            let motd_text = format!("{motd_num}\n{motd}");
                            output.add_string(motd_text.as_bytes());
                        }

                        // Character list (0x64) — 10.98 format: worlds first, then chars
                        output.add_byte(0x64);

                        // World list (1 world)
                        let game_port = config.get_number(IntegerConfig::GamePort) as u16;
                        let ip_str = config.get_string(StringConfig::IpString).to_owned();
                        output.add_byte(1); // world count
                        output.add_byte(0); // world id
                        output.add_string(server_name.as_bytes());
                        output.add_string(ip_str.as_bytes());
                        output.add_u16(game_port);
                        output.add_byte(0); // preview world = false

                        // Character entries
                        output.add_byte(account.characters.len() as u8);
                        for ch in &account.characters {
                            output.add_byte(0); // world id
                            output.add_string(ch.name.as_bytes());
                        }

                        // Premium: u8 status + u8 subStatus + u32 premiumTimeStamp
                        let has_premium = free_premium || account.premium_days > 0;
                        output.add_byte(0); // account status (0 = OK)
                        output.add_byte(if has_premium { 1 } else { 0 }); // sub status (1=premium, 0=free)
                        let premium_until = if free_premium {
                            0u32
                        } else if account.premium_days > 0 {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as u32;
                            now + (account.premium_days as u32) * 86400
                        } else {
                            0u32
                        };
                        output.add_u32(premium_until);
                    } else {
                        // 8.60 format
                        if !motd.is_empty() {
                            output.add_byte(0x14);
                            let motd_text = format!("{motd_num}\n{motd}");
                            output.add_string(motd_text.as_bytes());
                        }
                        output.add_byte(0x64);
                        output.add_byte(account.characters.len() as u8);
                        for ch in &account.characters {
                            output.add_string(ch.name.as_bytes());
                            output.add_string(server_name.as_bytes());
                            output.add_u32(ch.world_ip);
                            output.add_u16(ch.world_port);
                        }
                        let premium_days = if free_premium { 0xFFFF } else { account.premium_days };
                        output.add_u16(premium_days);
                    }

                    crypto.finalize_output(&mut output);
                    conn.send_bytes(output.get_output_buffer().to_vec());
                    conn.disconnect();
                }
                LoginResult::WrongPassword => {
                    tracing::info!("login: wrong password");
                    send_error(&conn, "Account name or password is not correct.", checksummed, round_keys);
                }
                LoginResult::NotFound => {
                    tracing::info!("login: account not found");
                    send_error(&conn, "Account name or password is not correct.", checksummed, round_keys);
                }
                LoginResult::Error(e) => {
                    tracing::info!(%e, "login: DB error");
                    send_error(&conn, &format!("An error occurred: {e}"), checksummed, round_keys);
                }
            }
        });
    }

    fn disconnect_client(&self, conn: &ConnectionHandle, message: &str) {
        let mut output = OutputMessage::new();
        let opcode = if client_version().is_1098() { 0x0B } else { 0x0A };
        output.add_byte(opcode);
        output.add_string(message.as_bytes());
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
        conn.disconnect();
    }
}

fn send_error(
    conn: &ConnectionHandle,
    message: &str,
    checksummed: bool,
    round_keys: Option<[u32; 64]>,
) {
    let mut crypto = ProtocolCrypto::new(checksummed);
    if let Some(rk) = round_keys {
        crypto.round_keys = Some(Box::new(rk));
        crypto.encryption_enabled = true;
    }
    let mut output = OutputMessage::new();
    let opcode = if client_version().is_1098() { 0x0B } else { 0x0A };
    output.add_byte(opcode);
    output.add_string(message.as_bytes());
    crypto.finalize_output(&mut output);
    conn.send_bytes(output.get_output_buffer().to_vec());
    conn.disconnect();
}
