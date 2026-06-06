use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{g_config, BooleanConfig, IntegerConfig, StringConfig};
use crate::game::g_game;
use crate::net::connection::ConnectionHandle;
use crate::net::message::NetworkMessage;
use crate::net::output_message::OutputMessage;
use crate::net::protocol::ProtocolCrypto;

pub struct ProtocolStatus {
    crypto: ProtocolCrypto,
}

impl ProtocolStatus {
    pub fn new() -> Self {
        Self {
            crypto: ProtocolCrypto::new(false),
        }
    }

    pub fn on_recv_first_message(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let request_type = msg.get_byte();
        match request_type {
            0xFF => self.send_status_string(conn),
            0x01 => {
                let requested_info = msg.get_u32();
                let char_name = msg.get_string(None);
                let char_name = String::from_utf8_lossy(&char_name).into_owned();
                self.send_info(conn, requested_info, &char_name);
            }
            _ => {}
        }
        conn.disconnect();
    }

    fn send_status_string(&mut self, conn: &ConnectionHandle) {
        let config = g_config();
        let game = g_game().lock().unwrap();

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let server_ip = config.get_number(IntegerConfig::Ip) as u32;
        let ip_bytes = server_ip.to_le_bytes();
        let ip_str = format!("{}.{}.{}.{}", ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]);

        let players_online = game.get_player_count();
        let monsters_total = game.get_monster_count();
        let npcs_total = game.get_npc_count();
        let xml = format!(
            r#"<?xml version="1.0"?><tsqp serverinfo="0" version="1.0" timestamp="{ts}"><serverinfo servername="{name}" serverip="{ip}" port="{port}" location="{loc}" url="{url}" server="The Forgotten Server" version="1.5 (Nekiro&apos;s 8.60 downgrade)" client="860"/><owner name="{owner}" email="{email}"/><players online="{online}" max="{max_players}" peak="{peak}"/><monsters total="{monsters}"/><npcs total="{npcs}"/><rates experience="{rate_exp}" skill="{rate_skill}" loot="{rate_loot}" magic="{rate_magic}" spawn="{rate_spawn}"/><map name="{map_name}" author="{map_author}" width="0" height="0"/></tsqp>"#,
            ts = ts,
            name = xml_escape(config.get_string(StringConfig::ServerName)),
            ip = ip_str,
            port = config.get_number(IntegerConfig::LoginPort),
            loc = xml_escape(config.get_string(StringConfig::Location)),
            url = xml_escape(config.get_string(StringConfig::Url)),
            owner = xml_escape(config.get_string(StringConfig::OwnerName)),
            email = xml_escape(config.get_string(StringConfig::OwnerEmail)),
            online = players_online,
            max_players = config.get_number(IntegerConfig::MaxPlayers),
            peak = game.get_players_record(),
            monsters = monsters_total,
            npcs = npcs_total,
            rate_exp = config.get_number(IntegerConfig::RateExperience),
            rate_skill = config.get_number(IntegerConfig::RateSkill),
            rate_loot = config.get_number(IntegerConfig::RateLoot),
            rate_magic = config.get_number(IntegerConfig::RateMagic),
            rate_spawn = config.get_number(IntegerConfig::RateSpawn),
            map_name = xml_escape(config.get_string(StringConfig::MapName)),
            map_author = xml_escape(config.get_string(StringConfig::MapAuthor)),
        );
        drop(game);

        self.crypto.raw_messages = true;
        let mut output = OutputMessage::new();
        output.add_bytes(xml.as_bytes());
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn send_info(&mut self, conn: &ConnectionHandle, requested_info: u32, _char_name: &str) {
        const REQUEST_BASIC_SERVER_INFO: u32 = 1 << 0;
        const REQUEST_OWNER_SERVER_INFO: u32 = 1 << 1;
        const REQUEST_MISC_SERVER_INFO: u32 = 1 << 2;
        const REQUEST_PLAYERS_INFO: u32 = 1 << 5;
        const REQUEST_SERVER_SOFTWARE_INFO: u32 = 1 << 7;

        let config = g_config();
        let mut output = OutputMessage::new();

        if requested_info & REQUEST_BASIC_SERVER_INFO != 0 {
            output.add_byte(0x10);
            output.add_string(config.get_string(StringConfig::ServerName).as_bytes());
            let server_ip = config.get_number(IntegerConfig::Ip) as u32;
            let ip_bytes = server_ip.to_le_bytes();
            let ip_str = format!("{}.{}.{}.{}", ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]);
            output.add_string(ip_str.as_bytes());
            output.add_u16(config.get_number(IntegerConfig::LoginPort) as u16);
        }

        if requested_info & REQUEST_OWNER_SERVER_INFO != 0 {
            output.add_byte(0x11);
            output.add_string(config.get_string(StringConfig::OwnerName).as_bytes());
            output.add_string(config.get_string(StringConfig::OwnerEmail).as_bytes());
        }

        if requested_info & REQUEST_MISC_SERVER_INFO != 0 {
            output.add_byte(0x12);
            output.add_string(config.get_string(StringConfig::Motd).as_bytes());
            output.add_string(config.get_string(StringConfig::Location).as_bytes());
            output.add_string(config.get_string(StringConfig::Url).as_bytes());
            let uptime: u64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            output.add_u64(uptime);
        }

        if requested_info & REQUEST_PLAYERS_INFO != 0 {
            let game = g_game().lock().unwrap();
            output.add_byte(0x20);
            output.add_u16(0); // online count placeholder
            output.add_u16(config.get_number(IntegerConfig::MaxPlayers) as u16);
            output.add_u32(game.get_players_record());
            drop(game);
        }

        if requested_info & REQUEST_SERVER_SOFTWARE_INFO != 0 {
            output.add_byte(0x23);
            output.add_string(b"The Forgotten Server");
            output.add_string(b"1.5 (Nekiro's 8.60 downgrade)");
            output.add_string(b"8.60");
        }

        let free_premium = config.get_boolean(BooleanConfig::FreePremium);
        if free_premium {
            output.add_byte(0x01);
        }

        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }
}

impl Default for ProtocolStatus {
    fn default() -> Self {
        Self::new()
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
