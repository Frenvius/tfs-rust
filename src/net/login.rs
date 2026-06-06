use crate::config::{g_config, BooleanConfig, StringConfig};
use crate::db::login::{loginserver_authentication, LoginResult};
use crate::game::{g_game, GameState};
use crate::net::connection::ConnectionHandle;
use crate::net::message::NetworkMessage;
use crate::net::output_message::OutputMessage;
use crate::net::protocol::{rsa_decrypt, ProtocolCrypto};

const CLIENT_VERSION_MIN: u16 = 860;
const CLIENT_VERSION_MAX: u16 = 860;

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
        msg.skip_bytes(2); // OS version
        let version = msg.get_u16();
        msg.skip_bytes(12); // dat/spr/pic signatures

        if version <= 760 {
            self.disconnect_client(conn, "Only clients with protocol 8.60 allowed!");
            return;
        }

        if !rsa_decrypt(msg) {
            conn.disconnect();
            return;
        }

        let key: [u32; 4] = [msg.get_u32(), msg.get_u32(), msg.get_u32(), msg.get_u32()];
        self.crypto.enable_xtea(&key);

        if version < CLIENT_VERSION_MIN || version > CLIENT_VERSION_MAX {
            self.disconnect_client(
                conn,
                &format!(
                    "Only clients with protocol {}.{} allowed!",
                    CLIENT_VERSION_MIN / 100,
                    CLIENT_VERSION_MIN % 100
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

        let checksummed = self.crypto.checksum_enabled;
        let round_keys = self.crypto.round_keys.as_ref().map(|rk| **rk);
        let conn = conn.clone();

        tokio::spawn(async move {
            match loginserver_authentication(&account_name, &password).await {
                LoginResult::Success(account) => {
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

                    // C++ bundles MOTD (0x14) + character list (0x64) in one OutputMessage.
                    let mut output = OutputMessage::new();
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
                    crypto.finalize_output(&mut output);
                    conn.send_bytes(output.get_output_buffer().to_vec());
                    conn.disconnect();
                }
                LoginResult::WrongPassword => {
                    send_error(&conn, "Account name or password is not correct.", checksummed, round_keys);
                }
                LoginResult::NotFound => {
                    send_error(&conn, "Account name or password is not correct.", checksummed, round_keys);
                }
                LoginResult::Error(e) => {
                    send_error(&conn, &format!("An error occurred: {e}"), checksummed, round_keys);
                }
            }
        });
    }

    fn disconnect_client(&self, conn: &ConnectionHandle, message: &str) {
        let mut output = OutputMessage::new();
        output.add_byte(0x0A); // disconnect/error opcode
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
    output.add_byte(0x0A);
    output.add_string(message.as_bytes());
    crypto.finalize_output(&mut output);
    conn.send_bytes(output.get_output_buffer().to_vec());
    conn.disconnect();
}
