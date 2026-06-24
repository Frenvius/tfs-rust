use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::debug;

use crate::config::{g_config, BooleanConfig};
use crate::creatures::player::{
    AccountType, Player, CONST_SLOT_FIRST, CONST_SLOT_LAST, SKILL_COUNT,
};
use crate::creatures::{CreatureId, Direction, Outfit, Skull};
use crate::crypto::xtea::RoundKeys;
use crate::db::login::{gameworld_authentication, load_player_by_name};
use crate::game::{g_game, GameState};
use crate::items::{ItemGroup, Items};
use crate::map::{Map, Position, MAX_VIEWPORT_X, MAX_VIEWPORT_Y};
use crate::combat::condition::{ICON_PZ, ICON_PZBLOCK, ICON_SWORDS};
use crate::map::tile::{Tile, TILESTATE_BLOCKSOLID};
use crate::map::TILESTATE_PROTECTIONZONE;
use crate::net::connection::ConnectionHandle;
use crate::net::message::NetworkMessage;
use crate::net::output_message::OutputMessage;
use crate::net::protocol::{rsa_decrypt, ProtocolCrypto};
use crate::net::protocol_version::{client_version, translate_message_class_to_client, translate_speak_class_to_client};
use crate::util::adler_checksum;

struct PlayerSession {
    conn: ConnectionHandle,
    round_keys: Arc<RoundKeys>,
    checksum_enabled: bool,
    known_creatures: Mutex<HashSet<u32>>,
}

fn dispatch(f: impl FnOnce() + Send + 'static) {
    crate::runtime::g_dispatcher().add_task(
        crate::runtime::dispatcher::Task::new(f),
    );
}

static PLAYER_SESSIONS: OnceLock<Mutex<HashMap<CreatureId, PlayerSession>>> = OnceLock::new();

fn player_sessions() -> &'static Mutex<HashMap<CreatureId, PlayerSession>> {
    PLAYER_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_player_connection(creature_id: CreatureId, conn: ConnectionHandle, round_keys: Arc<RoundKeys>, checksum_enabled: bool, known_creatures: HashSet<u32>) {
    player_sessions().lock().unwrap().insert(creature_id, PlayerSession {
        conn,
        round_keys,
        checksum_enabled,
        known_creatures: Mutex::new(known_creatures),
    });
}

pub fn unregister_player_connection(creature_id: CreatureId) {
    player_sessions().lock().unwrap().remove(&creature_id);
}

thread_local! {
    /// When set, packets destined for this creature id are appended into one
    /// OutputMessage instead of being sent as separate frames — used to bundle
    /// the login init + onLogin welcome messages into a single XTEA frame, as
    /// C++ does (it flushes the player's output once per dispatcher cycle).
    static PLAYER_BUNDLE: std::cell::RefCell<Option<(CreatureId, OutputMessage)>> =
        const { std::cell::RefCell::new(None) };
}

/// Begin accumulating packets for `creature_id` into `output` (which already
/// holds the init body). Must be paired with `end_player_bundle`. No `.await`
/// may occur between the two (the bundle is thread-local).
fn begin_player_bundle(creature_id: CreatureId, output: OutputMessage) {
    PLAYER_BUNDLE.with(|b| *b.borrow_mut() = Some((creature_id, output)));
}

/// Finish bundling and return the accumulated OutputMessage.
fn end_player_bundle() -> Option<OutputMessage> {
    PLAYER_BUNDLE.with(|b| b.borrow_mut().take().map(|(_, out)| out))
}

/// Open an empty per-player bundle so a synchronous burst of packets for one
/// player (e.g. a spell cast's effect + stats) is flushed as a single XTEA
/// frame. Pair with `flush_player_bundle`. No `.await` may occur between them.
pub(crate) fn begin_player_bundle_empty(creature_id: CreatureId) {
    begin_player_bundle(creature_id, OutputMessage::new());
}

/// Close the active bundle and send the accumulated frame to the player.
/// No-op if the bundle is empty or the session is gone.
pub(crate) fn flush_player_bundle(creature_id: CreatureId) {
    let Some(mut output) = end_player_bundle() else { return };
    if output.get_length() == 0 {
        return;
    }
    let sessions = player_sessions().lock().unwrap();
    let Some(session) = sessions.get(&creature_id) else { return };
    finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
}

pub fn send_packet_to_player(creature_id: CreatureId, build_fn: impl FnOnce(&mut OutputMessage)) {
    // If a bundle is active for this exact creature, append into it (one frame).
    let mut build_fn = Some(build_fn);
    let appended = PLAYER_BUNDLE.with(|b| {
        let mut b = b.borrow_mut();
        if let Some((bcid, out)) = b.as_mut() {
            if *bcid == creature_id {
                (build_fn.take().unwrap())(out);
                return true;
            }
        }
        false
    });
    if appended {
        return;
    }
    let build_fn = build_fn.unwrap();
    let sessions = player_sessions().lock().unwrap();
    let Some(session) = sessions.get(&creature_id) else { return };
    let mut output = OutputMessage::new();
    build_fn(&mut output);
    finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
}

pub fn broadcast_creature_teleport_pub(creature_id: CreatureId, old_pos: Position, old_stackpos: u8, new_pos: Position) {
    broadcast_creature_teleport(creature_id, old_pos, old_stackpos, new_pos);
}

/// Send the full teleport packet sequence to a player being teleported:
/// remove from old tile + full 0x64 map description at the new position.
/// Mirrors C++ `ProtocolGame::sendMoveCreature(..., teleport=true)` for the
/// player == creature case.
pub fn send_teleport_map_to_player(creature_id: CreatureId, old_pos: Position, old_stackpos: u8, new_pos: Position) {
    let game = crate::game::g_game().lock().unwrap();
    let sessions = player_sessions().lock().unwrap();
    let Some(session) = sessions.get(&creature_id) else { return };
    let mut known = session.known_creatures.lock().unwrap();

    // Frame 1: remove creature from old tile
    let mut output = OutputMessage::new();
    write_remove_tile_creature(&mut output, old_pos, old_stackpos, creature_id);
    finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);

    // Frame 2: full map description at new position (separate frame to avoid overflow)
    output = OutputMessage::new();
    if let Some(player) = game.get_player(creature_id) {
        write_map_description(
            &mut output,
            &game,
            game.get_items(),
            new_pos,
            &mut known,
            creature_id,
            player,
        );
    }

    finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
}

/// Send a 0xB4 MESSAGE_STATUS_SMALL text line to a player's session.
pub fn send_status_message_to_player(creature_id: CreatureId, text: &str) {
    let bytes = text.as_bytes().to_vec();
    let msg_type = crate::net::protocol_version::message_status_small();
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0xB4);
        output.add_byte(msg_type);
        output.add_string(&bytes);
    });
}

fn format_date_short(timestamp: u32) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let dt = UNIX_EPOCH + Duration::from_secs(timestamp as u64);
    let secs = dt.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = (secs / 86400) as i64;
    let (year, month, day) = {
        let mut y = 1970i64;
        let mut remaining = days;
        loop {
            let days_in_year = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
            if remaining < days_in_year { break; }
            remaining -= days_in_year;
            y += 1;
        }
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut m = 0usize;
        while m < 12 && remaining >= month_days[m] {
            remaining -= month_days[m];
            m += 1;
        }
        (y, m, remaining + 1)
    };
    let month_names = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{:02} {} {}", day, month_names[month], year)
}

#[allow(clippy::too_many_arguments)]
fn send_text_window_to_player(
    creature_id: CreatureId,
    window_text_id: u32,
    client_id: u16,
    item_count_byte: Option<u8>,
    text: &str,
    writer: &str,
    date: u32,
    can_write: bool,
    max_text_len: u16,
) {
    let text_bytes = text.as_bytes().to_vec();
    let writer_bytes = writer.as_bytes().to_vec();
    let date_str = if date != 0 {
        format_date_short(date).into_bytes()
    } else {
        Vec::new()
    };
    let text_len = text_bytes.len();
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0x96);
        output.add_u32(window_text_id);
        output.add_u16(client_id);
        if let Some(count) = item_count_byte {
            output.add_byte(count);
        }
        if can_write {
            output.add_u16(max_text_len);
        } else {
            output.add_u16(text_len as u16);
        }
        output.add_string(&text_bytes);
        if writer_bytes.is_empty() {
            output.add_u16(0);
        } else {
            output.add_string(&writer_bytes);
        }
        if date_str.is_empty() {
            output.add_u16(0);
        } else {
            output.add_string(&date_str);
        }
    });
}

pub fn send_house_window_to_player(creature_id: CreatureId, window_text_id: u32, text: &str) {
    let text_bytes = text.as_bytes().to_vec();
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0x97);
        output.add_byte(0x00);
        output.add_u32(window_text_id);
        output.add_string(&text_bytes);
    });
}

pub fn send_stats_to_player(creature_id: CreatureId) {
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    // Bundle-aware: append to the active per-player bundle if one is open for
    // this creature, otherwise send as its own frame.
    let appended = PLAYER_BUNDLE.with(|b| {
        let mut b = b.borrow_mut();
        if let Some((bcid, out)) = b.as_mut() {
            if *bcid == creature_id {
                write_player_stats(out, player);
                return true;
            }
        }
        false
    });
    if appended {
        return;
    }
    let mut output = OutputMessage::new();
    write_player_stats(&mut output, player);
    let sessions = player_sessions().lock().unwrap();
    let Some(session) = sessions.get(&creature_id) else { return };
    finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
}

pub fn send_skills_to_player(creature_id: CreatureId) {
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    let mut output = OutputMessage::new();
    write_player_skills(&mut output, player);
    let sessions = player_sessions().lock().unwrap();
    let Some(session) = sessions.get(&creature_id) else { return };
    finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
}

pub fn broadcast_creature_health(creature_id: CreatureId, pos: Position, health: i32, health_max: i32, hidden: bool) {
    let health_percent = if hidden || health_max <= 0 {
        0u8
    } else {
        ((health as f64 / health_max as f64) * 100.0).ceil() as u8
    };
    broadcast_effect_to_spectators(pos, |output: &mut OutputMessage| {
        output.add_byte(0x8C);
        output.add_u32(creature_id);
        output.add_byte(health_percent);
    });
}

pub fn broadcast_magic_effect(pos: Position, effect_type: u8) {
    broadcast_effect_to_spectators(pos, |output: &mut OutputMessage| {
        output.add_byte(0x83);
        let p = pos;
        output.add_u16(p.x);
        output.add_u16(p.y);
        output.add_byte(p.z);
        output.add_byte(effect_type);
    });
}

pub fn broadcast_distance_effect(from: Position, to: Position, effect_type: u8) {
    let spectator_ids: Vec<CreatureId>;
    {
        let mut game = g_game().lock().unwrap();
        // Spectators of both source and destination.
        let mut ids: std::collections::HashSet<CreatureId> = game
            .map.get_spectators(from, false, true, 0, 0, 0, 0)
            .into_iter().collect();
        ids.extend(game.map.get_spectators(to, false, true, 0, 0, 0, 0));
        spectator_ids = ids.into_iter().collect();
    }
    let sessions = player_sessions().lock().unwrap();
    for spec_id in spectator_ids {
        let Some(session) = sessions.get(&spec_id) else { continue };
        let mut output = OutputMessage::new();
        output.add_byte(0x85);
        output.add_u16(from.x); output.add_u16(from.y); output.add_byte(from.z);
        output.add_u16(to.x);   output.add_u16(to.y);   output.add_byte(to.z);
        output.add_byte(effect_type);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

pub fn broadcast_change_speed(creature_id: CreatureId, pos: Position, speed: u32) {
    broadcast_effect_to_spectators(pos, |output: &mut OutputMessage| {
        output.add_byte(0x8F);
        output.add_u32(creature_id);
        output.add_u16(speed.min(0xFFFF) as u16);
    });
}

pub fn broadcast_creature_light(creature_id: CreatureId, pos: Position, light: crate::creatures::LightInfo) {
    let spectator_ids: Vec<CreatureId>;
    {
        let mut game = g_game().lock().unwrap();
        spectator_ids = game.map.get_spectators(pos, true, true, 0, 0, 0, 0);
    }
    for spec_id in spectator_ids {
        send_packet_to_player(spec_id, |output: &mut OutputMessage| {
            output.add_byte(0x8D);
            output.add_u32(creature_id);
            output.add_byte(light.level);
            output.add_byte(light.color);
        });
    }
}

pub fn broadcast_creature_outfit(creature_id: CreatureId, pos: Position, outfit: crate::creatures::Outfit) {
    let spectator_ids: Vec<CreatureId>;
    {
        let mut game = g_game().lock().unwrap();
        spectator_ids = game.map.get_spectators(pos, true, true, 0, 0, 0, 0);
    }
    for spec_id in spectator_ids {
        send_packet_to_player(spec_id, |output: &mut OutputMessage| {
            output.add_byte(0x8E);
            output.add_u32(creature_id);
            if outfit.look_type != 0 {
                output.add_u16(outfit.look_type);
                output.add_byte(outfit.look_head);
                output.add_byte(outfit.look_body);
                output.add_byte(outfit.look_legs);
                output.add_byte(outfit.look_feet);
                output.add_byte(outfit.look_addons);
            } else {
                output.add_u16(0);
                output.add_u16(outfit.look_type_ex);
            }
        });
    }
}

pub fn broadcast_creature_visible(creature_id: CreatureId, pos: Position, visible: bool, is_player_creature: bool, outfit: crate::creatures::Outfit) {
    let spectator_ids: Vec<CreatureId>;
    {
        let mut game = g_game().lock().unwrap();
        spectator_ids = game.map.get_spectators(pos, true, true, 0, 0, 0, 0);
    }
    for spec_id in spectator_ids {
        if is_player_creature {
            let send_outfit = if visible { outfit } else { crate::creatures::Outfit::default() };
            send_packet_to_player(spec_id, |output: &mut OutputMessage| {
                output.add_byte(0x8E);
                output.add_u32(creature_id);
                if send_outfit.look_type != 0 {
                    output.add_u16(send_outfit.look_type);
                    output.add_byte(send_outfit.look_head);
                    output.add_byte(send_outfit.look_body);
                    output.add_byte(send_outfit.look_legs);
                    output.add_byte(send_outfit.look_feet);
                    output.add_byte(send_outfit.look_addons);
                } else {
                    output.add_u16(0);
                    output.add_u16(send_outfit.look_type_ex);
                }
            });
        } else {
            send_packet_to_player(spec_id, |output: &mut OutputMessage| {
                output.add_byte(0x8E);
                output.add_u32(creature_id);
                if visible {
                    if outfit.look_type != 0 {
                        output.add_u16(outfit.look_type);
                        output.add_byte(outfit.look_head);
                        output.add_byte(outfit.look_body);
                        output.add_byte(outfit.look_legs);
                        output.add_byte(outfit.look_feet);
                        output.add_byte(outfit.look_addons);
                    } else {
                        output.add_u16(0);
                        output.add_u16(outfit.look_type_ex);
                    }
                } else {
                    output.add_u16(0);
                    output.add_u16(0);
                }
            });
        }
    }
}

pub fn send_icons_to_player(creature_id: CreatureId) {
    let icons = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let pos = player.base.position;
        let mut v: u32 = player.base.conditions.iter()
            .map(|c| c.get_icons())
            .fold(0u32, |a, b| a | b);
        if player.pz_locked { v |= crate::combat::condition::ICON_PZBLOCK; }
        if let Some(tile) = game.map.get_tile(pos) {
            if tile.has_flag(crate::map::TILESTATE_PROTECTIONZONE) {
                v |= crate::combat::condition::ICON_PZ;
                v &= !crate::combat::condition::ICON_SWORDS;
            }
        }
        v as u16
    };
    send_packet_to_player(creature_id, |output: &mut OutputMessage| {
        output.add_byte(0xA2);
        output.add_u16(icons);
    });
}

pub fn broadcast_effect_to_spectators(pos: Position, build_fn: impl Fn(&mut OutputMessage)) {
    let spectator_ids: Vec<CreatureId>;
    {
        let mut game = g_game().lock().unwrap();
        spectator_ids = game.map.get_spectators(pos, false, true, 0, 0, 0, 0);
    }

    // Route through send_packet_to_player so that, if a per-player bundle is
    // active (e.g. during a spell cast), the caster's effect is appended to the
    // same XTEA frame as the rest of the cast; other spectators get their own
    // frame (a separate connection — same as C++).
    for spec_id in spectator_ids {
        send_packet_to_player(spec_id, |output: &mut OutputMessage| build_fn(output));
    }
}

const MAP_MAX_LAYERS: i32 = 16;
const USE_TELEPORT_UP_IDS: &[u16] = &[1386, 3678, 5543];
const USE_TELEPORT_DOWN_IDS: &[u16] = &[430, 1369];
const ROPE_ITEM_IDS: &[u16] = &[2120, 7731, 10511, 10513, 10515];
const ROPE_SPOT_IDS: &[u16] = &[384, 418, 8278, 8592];

pub struct ProtocolGame {
    pub crypto: ProtocolCrypto,
    pub checksummed: bool,
    challenge_timestamp: u32,
    challenge_random: u8,
    accept_packets: bool,
    creature_id: u32,
    known_creatures: HashSet<u32>,
}

impl ProtocolGame {
    pub fn new(checksummed: bool) -> Self {
        Self {
            crypto: ProtocolCrypto::new(checksummed),
            checksummed,
            challenge_timestamp: 0,
            challenge_random: 0,
            accept_packets: false,
            creature_id: 0,
            known_creatures: HashSet::new(),
        }
    }

    pub fn on_connect(&mut self, conn: &ConnectionHandle) {
        self.challenge_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;
        self.challenge_random = rand::random::<u8>();

        let mut output = OutputMessage::new();

        output.skip_bytes(4);
        output.add_u16(0x0006);
        output.add_byte(0x1F);
        output.add_u32(self.challenge_timestamp);
        output.add_byte(self.challenge_random);

        output.skip_bytes(-12);
        let checksum = adler_checksum(&output.get_raw_buffer()[12..20]);
        output.add_u32(checksum);

        output.write_message_length();

        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    pub async fn on_recv_first_message(
        &mut self,
        msg: &mut NetworkMessage,
        conn: &ConnectionHandle,
    ) {
        let is_1098 = client_version().is_1098();
        let version_min = client_version().min_version();
        let version_max = client_version().max_version();

        let _os = msg.get_u16();
        let version = msg.get_u16();

        if is_1098 {
            // 10.98: clientVersion(u32) + clientType(u8) + datRevision(u16) = 7 bytes
            msg.skip_bytes(7);
        }

        tracing::info!(version, is_1098, "game login: RSA decrypt");
        if !rsa_decrypt(msg) {
            tracing::warn!("game login: RSA decrypt failed");
            conn.disconnect();
            return;
        }

        let key: [u32; 4] = [msg.get_u32(), msg.get_u32(), msg.get_u32(), msg.get_u32()];
        self.crypto.enable_xtea(&key);

        msg.skip_bytes(1); // gamemaster flag

        let (account_name, char_name, password) = if is_1098 {
            // 10.98: session key string "accountName\npassword\ntoken\ntokenTime"
            let session_bytes = msg.get_string(None);
            let session = String::from_utf8_lossy(&session_bytes).into_owned();
            let parts: Vec<&str> = session.splitn(4, '\n').collect();
            let acc = parts.first().copied().unwrap_or("").to_string();
            let pwd = parts.get(1).copied().unwrap_or("").to_string();
            let char_bytes = msg.get_string(None);
            let chr = String::from_utf8_lossy(&char_bytes).into_owned();
            (acc, chr, pwd)
        } else {
            // 8.60: separate account/char/password fields
            let account_name_bytes = msg.get_string(None);
            let account_name = String::from_utf8_lossy(&account_name_bytes).into_owned();
            let char_name_bytes = msg.get_string(None);
            let char_name = String::from_utf8_lossy(&char_name_bytes).into_owned();
            let password_bytes = msg.get_string(None);
            let password = String::from_utf8_lossy(&password_bytes).into_owned();
            (account_name, char_name, password)
        };

        if account_name.is_empty() {
            self.disconnect_client(conn, "You must enter your account name.");
            return;
        }

        let recv_timestamp = msg.get_u32();
        let recv_random = msg.get_byte();
        if recv_timestamp != self.challenge_timestamp || recv_random != self.challenge_random {
            conn.disconnect();
            return;
        }

        if version < version_min || version > version_max {
            self.disconnect_client(conn, &format!(
                "Only clients with protocol {}.{} allowed!",
                version_min / 100,
                version_min % 100
            ));
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
        }

        tracing::info!(account = %account_name, char_name = %char_name, "game login: authenticating");
        if !gameworld_authentication(&account_name, &password, &char_name).await {
            tracing::warn!(account = %account_name, "game login: auth failed");
            self.disconnect_client(conn, "Account name or password is not correct.");
            return;
        }
        tracing::info!(char_name = %char_name, "game login: loading player");

        let player = match load_player_by_name(&char_name).await {
            Some(p) => p,
            None => {
                self.disconnect_client(conn, "Your character could not be loaded.");
                return;
            }
        };
        // C++ ProtocolGame::login checks getPlayerByName first and kicks
        // the old session before placing the new one.  Without this, a
        // lingering old connection (e.g. unclean disconnect) leaves the
        // player in the world and the new login either double-inserts or
        // gets rejected.
        {
            let existing_id = g_game().lock().unwrap().get_player_id_by_name(&char_name);
            if let Some(old_id) = existing_id {
                tracing::info!(char_name = %char_name, old_id, "game login: kicking stale session");
                kick_player_by_id(old_id);
            }
        }

        // Assign creature ID and track player in the game.
        let (creature_id, player_guid) = {
            let mut game = g_game().lock().unwrap();
            let id = game.next_creature_id();
            let guid = player.guid;
            game.add_player(id, player);
            game.add_creature_check(id);
            (id, guid)
        };
        self.creature_id = creature_id;

        // Notify VIP observers that this player is now online.
        broadcast_vip_status(player_guid, true);

        // Insert into players_online table (prevents duplicate logins).
        tokio::spawn(async move {
            crate::db::login::update_online_status(player_guid, true).await;
        });

        // Resolve login position: if (0,0,0) use town temple position.
        let login_pos = {
            let game = g_game().lock().unwrap();
            let player = game.get_player(creature_id).unwrap();
            let pos = player.login_position;
            if pos.x == 0 && pos.y == 0 {
                game.map
                    .towns
                    .get(&player.town_id)
                    .map(|t| t.temple_pos)
                    .unwrap_or(pos)
            } else {
                pos
            }
        };

        // Update player position to login position and record login timestamp.
        {
            let now = crate::util::get_milliseconds_time();
            let mut game = g_game().lock().unwrap();
            if let Some(p) = game.get_player_mut(creature_id) {
                p.base.position = login_pos;
                let now_sec = now / 1000;
                p.last_login_saved = now_sec.max(p.last_login_saved + 1);
                p.last_ping = now;
                p.last_pong = now;
            }
        }

        // Pull known_creatures out so we can pass it mutably to write_map_description
        // while also holding a mutable borrow on self.crypto below.
        let mut known = std::mem::take(&mut self.known_creatures);

        // C++ accumulates all init writes into one output buffer before sending,
        // so the client receives a single bundled XTEA frame at login. Build the
        // entire init sequence into one OutputMessage to match.
        let mut output = OutputMessage::new();

        {
            let game = g_game().lock().unwrap();
            let player = game.get_player(creature_id).unwrap();
            let pos = player.base.position;
            // C++ uses isAccessPlayer() (group->access flag) for the light
            // overrides, NOT account type — god group is not an access group.
            let is_access = player.is_access_player();
            let can_report_bugs = player.account_type >= AccountType::Tutor;
            let (light_level, light_color) = game.get_world_light_info();
            let c_light = player.base.internal_light;

            if is_1098 {
                // 10.98 login sequence (Player::onCreatureAppear, creature==this):
                // C++ uses writeToOutputBuffer which auto-splits. We send each
                // major section as a separate XTEA frame since the 10.98 map +
                // mark bytes exceed a single OutputMessage.

                let rk = Arc::new(**self.crypto.round_keys.as_ref().expect("xtea keys set"));

                // Frame 1: 0x17 login success — must come first so client knows own creature ID
                fn add_double_10(out: &mut OutputMessage, value: f64) {
                    out.add_byte(3);
                    let scaled = (value * 1000.0) + f64::from(i32::MAX);
                    out.add_u32(scaled as u32);
                }
                output.add_byte(0x17);
                output.add_u32(creature_id);
                output.add_u16(0x32); // beat duration (50)
                add_double_10(&mut output, 857.36);
                add_double_10(&mut output, 261.29);
                add_double_10(&mut output, -4795.01);
                output.add_byte(u8::from(can_report_bugs));
                output.add_byte(0x00); // pvp frame option
                output.add_byte(0x00); // expert mode
                output.add_string(b""); // store URL (empty)
                output.add_u16(25); // coin package size
                finalize_and_send(&mut output, &rk, self.checksummed, conn);

                // Frame 2: pending + enter world
                output = OutputMessage::new();
                output.add_byte(0x0A); // sendPendingStateEntered
                output.add_byte(0x0F); // sendEnterWorld
                finalize_and_send(&mut output, &rk, self.checksummed, conn);

                // Frame 3: map description
                output = OutputMessage::new();
                write_map_description(&mut output, &game, game.get_items(), pos,
                    &mut known, creature_id, player);
                finalize_and_send(&mut output, &rk, self.checksummed, conn);

                // Frame: inventory + stats + skills + lights + basic data
                output = OutputMessage::new();
                for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
                    if let Some(server_id) = player.inventory[slot] {
                        output.add_byte(0x78);
                        output.add_byte(slot as u8);
                        write_item(&mut output, game.get_items(), server_id, 1);
                    } else {
                        output.add_byte(0x79);
                        output.add_byte(slot as u8);
                    }
                }

                write_player_stats(&mut output, player);
                write_player_skills(&mut output, player);

                output.add_byte(0x82); // world light
                output.add_byte(if is_access { 0xFF } else { light_level });
                output.add_byte(light_color);

                output.add_byte(0x8D); // creature light
                output.add_u32(creature_id);
                output.add_byte(if is_access { 0xFF } else { c_light.level });
                output.add_byte(c_light.color);

                write_basic_data(&mut output, player);

                {
                    let buf = output.get_output_buffer();
                    let hex: String = buf.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                    std::fs::write("init_frame3.hex", &hex).ok();
                    tracing::info!(len = buf.len(), "INIT_FRAME3 written");
                }
                // Icons handled below (shared with 8.60)
            } else {
                // 0x0A: 8.60 player self-init
                output.add_byte(0x0A);
                output.add_u32(creature_id);
                output.add_u16(0x32);
                output.add_byte(u8::from(can_report_bugs));

                // 0x64: map description
                write_map_description(&mut output, &game, game.get_items(), pos,
                    &mut known, creature_id, player);

                // 0x78/0x79: inventory slots
                for slot in CONST_SLOT_FIRST..=CONST_SLOT_LAST {
                    if let Some(server_id) = player.inventory[slot] {
                        output.add_byte(0x78);
                        output.add_byte(slot as u8);
                        write_item(&mut output, game.get_items(), server_id, 1);
                    } else {
                        output.add_byte(0x79);
                        output.add_byte(slot as u8);
                    }
                }

                // 0xA0: player stats
                write_player_stats(&mut output, player);

                // 0xA1: player skills
                write_player_skills(&mut output, player);

                // 0x82: world light
                output.add_byte(0x82);
                output.add_byte(if is_access { 0xFF } else { light_level });
                output.add_byte(light_color);

                // 0x8D: creature light
                output.add_byte(0x8D);
                output.add_u32(creature_id);
                output.add_byte(if is_access { 0xFF } else { c_light.level });
                output.add_byte(c_light.color);
            }

            // 0xA2: player icons (condition icons + PZ/pzblock state)
            {
                let mut v: u32 = player.base.conditions.iter()
                    .map(|c| c.get_icons())
                    .fold(0u32, |a, b| a | b);
                if player.pz_locked {
                    v |= ICON_PZBLOCK;
                }
                if let Some(tile) = game.map.get_tile(pos) {
                    if tile.has_flag(TILESTATE_PROTECTIONZONE) {
                        v |= ICON_PZ;
                        v &= !ICON_SWORDS;
                    }
                }
                write_icons(&mut output, v as u16);
            }
        }

        // Restore known_creatures
        self.known_creatures = known;

        self.accept_packets = true;
        let rk = Arc::new(**self.crypto.round_keys.as_ref().expect("xtea keys set"));
        register_player_connection(creature_id, conn.clone(), rk.clone(), self.checksummed, self.known_creatures.clone());

        // Bundle the onLogin creaturescript's messages (welcome / last-visit)
        // into the SAME frame as the init, matching C++ which flushes the login
        // dispatcher's output once. No `.await` occurs in this window.
        begin_player_bundle(creature_id, output);
        crate::events::dispatch::execute_creature_event_login(creature_id);
        let mut output = end_player_bundle().expect("bundle was set");
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());

        // Send VIP list entries.
        {
            let game = g_game().lock().unwrap();
            if let Some(player) = game.get_player(creature_id) {
                let account_id = player.account_number;
                let vip_guids: Vec<u32> = player.vip_list.iter().copied().collect();
                drop(game);

                // For each VIP guid, look up name and online status.
                for guid in vip_guids {
                    let is_online = {
                        let game = g_game().lock().unwrap();
                        game.get_player_by_guid(guid)
                            .map(|p| !p.is_ghost_mode)
                            .unwrap_or(false)
                    };
                    let name = {
                        let game = g_game().lock().unwrap();
                        game.get_player_by_guid(guid).map(|p| p.name.clone())
                    };
                    let name = match name {
                        Some(n) => n,
                        None => {
                            // Offline — fetch name from DB async after login.
                            let cid = creature_id;
                            let _ = account_id;
                            tokio::spawn(async move {
                                use crate::db::DatabaseEngine;
                                let db = crate::db::g_database();
                                let query = format!(
                                    "SELECT `name` FROM `players` WHERE `id` = {guid}"
                                );
                                if let Ok(Some(result)) = db.store_query(&query).await {
                                    if let Some(n) = result.get_string("name") {
                                        let sessions = player_sessions().lock().unwrap();
                                        if let Some(session) = sessions.get(&cid) {
                                            let mut output = OutputMessage::new();
                                            write_vip(&mut output, guid, &n, false);
                                            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
                                        }
                                    }
                                }
                            });
                            continue;
                        }
                    };
                    let sessions = player_sessions().lock().unwrap();
                    if let Some(session) = sessions.get(&creature_id) {
                        let mut output = OutputMessage::new();
                        write_vip(&mut output, guid, &name, is_online);
                        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
                    }
                }
            }
        }

        broadcast_creature_appear(creature_id, login_pos);

        // Wake the player if they logged out while sleeping in a bed.
        {
            let bed_pos = {
                let game = g_game().lock().unwrap();
                game.get_player(creature_id)
                    .and_then(|p| game.get_bed_by_sleeper(p.guid))
            };
            if let Some(bed_pos) = bed_pos {
                let server_id = {
                    let game = g_game().lock().unwrap();
                    game.map.get_tile(bed_pos).and_then(|t| {
                        t.items
                            .iter()
                            .find(|it| {
                                game.items.get_item_type(usize::from(it.server_id)).kind
                                    == crate::items::ItemKind::Bed
                            })
                            .map(|it| it.server_id)
                    })
                };
                if let Some(server_id) = server_id {
                    self.bed_wake_up(bed_pos, server_id);
                }
            }
        }

        // onLogin already ran (bundled into the init frame above).
        if g_config().get_boolean(BooleanConfig::PlayerConsoleLogs) {
            println!("> {} has logged in.", char_name);
        }
    }

    pub fn on_recv_message(&mut self, msg: &mut NetworkMessage) {
        if !self.accept_packets {
            return;
        }

        let cid = self.creature_id;
        let opcode = msg.get_byte();
        match opcode {
            0x14 => {
                self.accept_packets = false;
                dispatch(move || game_handle_logout(cid));
            }
            0x1D | 0x1E => dispatch(move || game_handle_ping_back(cid)),
            0x32 => { let _ = msg.get_u32(); let _ = msg.get_string(None); }
            0x64 => {
                let path_len = msg.get_byte();
                let mut dirs = Vec::with_capacity(path_len as usize);
                for _ in 0..path_len {
                    dirs.push(msg.get_byte());
                }
                dispatch(move || game_parse_auto_walk(cid, dirs));
            }
            0x65 => dispatch(move || game_handle_walk(cid, Direction::North)),
            0x66 => dispatch(move || game_handle_walk(cid, Direction::East)),
            0x67 => dispatch(move || game_handle_walk(cid, Direction::South)),
            0x68 => dispatch(move || game_handle_walk(cid, Direction::West)),
            0x69 => dispatch(move || stop_auto_walk(cid)),
            0x6A => dispatch(move || game_handle_walk(cid, Direction::NorthEast)),
            0x6B => dispatch(move || game_handle_walk(cid, Direction::SouthEast)),
            0x6C => dispatch(move || game_handle_walk(cid, Direction::SouthWest)),
            0x6D => dispatch(move || game_handle_walk(cid, Direction::NorthWest)),
            0x73 => { let _pos = msg.get_position(); }
            0x77 => { let _sprite_id = msg.get_u16(); }
            0x6F => dispatch(move || game_handle_turn(cid, Direction::North)),
            0x70 => dispatch(move || game_handle_turn(cid, Direction::East)),
            0x71 => dispatch(move || game_handle_turn(cid, Direction::South)),
            0x72 => dispatch(move || game_handle_turn(cid, Direction::West)),
            0x78 => {
                let from_pos = to_map_position(msg.get_position());
                let sprite_id = msg.get_u16();
                let from_stackpos = msg.get_byte();
                let to_pos = to_map_position(msg.get_position());
                let count = msg.get_byte();
                dispatch(move || game_parse_throw(cid, from_pos, sprite_id, from_stackpos, to_pos, count));
            }
            0x79 => { let _id = msg.get_u16(); let _count = msg.get_byte(); }
            0x7A => { let _id = msg.get_u16(); let _count = msg.get_byte(); let _amount = msg.get_byte(); let _ignorecap = msg.get_byte(); let _inbackpacks = msg.get_byte(); }
            0x7B => { let _id = msg.get_u16(); let _count = msg.get_byte(); let _amount = msg.get_byte(); let _ignoreequipped = msg.get_byte(); }
            0x7C => dispatch(move || game_handle_close_shop(cid)),
            0x7D => {
                let pos = to_map_position(msg.get_position());
                let sprite_id = msg.get_u16();
                let stackpos = msg.get_byte();
                let target_id = msg.get_u32();
                dispatch(move || game_parse_request_trade(cid, pos, sprite_id, stackpos, target_id));
            }
            0x7E => {
                let counter_offer = msg.get_byte() != 0;
                let index = msg.get_byte();
                dispatch(move || game_parse_look_in_trade(cid, counter_offer, index));
            }
            0x7F => dispatch(move || game_handle_accept_trade(cid)),
            0x80 => dispatch(move || game_handle_close_trade(cid)),
            0x82 => {
                let pos = to_map_position(msg.get_position());
                let sprite_id = msg.get_u16();
                let stackpos = msg.get_byte();
                let index = msg.get_byte();
                dispatch(move || game_handle_use_item(cid, pos, sprite_id, stackpos, index));
            }
            0x83 => {
                let from_pos = to_map_position(msg.get_position());
                let from_sprite_id = msg.get_u16();
                let from_stackpos = msg.get_byte();
                let to_pos = to_map_position(msg.get_position());
                let to_sprite_id = msg.get_u16();
                let to_stackpos = msg.get_byte();
                dispatch(move || game_handle_use_item_ex(cid, from_pos, from_sprite_id, from_stackpos, to_pos, to_sprite_id, to_stackpos));
            }
            0x84 => {
                let from_pos = to_map_position(msg.get_position());
                let from_sprite_id = msg.get_u16();
                let from_stackpos = msg.get_byte();
                let target_creature_id = msg.get_u32();
                dispatch(move || game_handle_use_with_creature(cid, from_pos, from_sprite_id, from_stackpos, target_creature_id));
            }
            0x85 => {
                let pos = to_map_position(msg.get_position());
                let sprite_id = msg.get_u16();
                let stackpos = msg.get_byte();
                dispatch(move || game_parse_rotate_item(cid, pos, sprite_id, stackpos));
            }
            0x87 => {
                let container_id = msg.get_byte();
                dispatch(move || game_parse_close_container(cid, container_id));
            }
            0x88 => {
                let container_id = msg.get_byte();
                dispatch(move || game_parse_up_arrow_container(cid, container_id));
            }
            0x89 => {
                let window_text_id = msg.get_u32();
                let text = msg.get_string(None);
                dispatch(move || game_parse_text_window(cid, window_text_id, text));
            }
            0x8A => {
                let _door_id = msg.get_byte();
                let _window_text_id = msg.get_u32();
                let _text = msg.get_string(None);
            }
            0x8C => {
                let pos = to_map_position(msg.get_position());
                let sprite_id = msg.get_u16();
                let stackpos = msg.get_byte();
                dispatch(move || game_parse_look_at(cid, pos, sprite_id, stackpos));
            }
            0x8D => {
                let target_id = msg.get_u32();
                dispatch(move || game_parse_look_in_battle_list(cid, target_id));
            }
            0x8E => {}
            0x96 => {
                let speak_type = msg.get_byte();
                let channel_id = if speak_type == 7 || speak_type == 10 {
                    Some(msg.get_u16())
                } else { None };
                let receiver_name = if speak_type == 6 || speak_type == 11 {
                    Some(msg.get_string(None))
                } else { None };
                let text = msg.get_string(None);
                dispatch(move || game_handle_say(cid, speak_type, channel_id, receiver_name, text));
            }
            0x97 => dispatch(move || game_handle_request_channels(cid)),
            0x98 => {
                let channel_id = msg.get_u16();
                dispatch(move || game_parse_open_channel(cid, channel_id));
            }
            0x99 => {
                let channel_id = msg.get_u16();
                dispatch(move || game_parse_close_channel(cid, channel_id));
            }
            0x9A => {
                let name = msg.get_string(None);
                dispatch(move || game_parse_open_private_channel(cid, name));
            }
            0x9E => dispatch(move || game_handle_close_npc_channel(cid)),
            0xA0 => {
                let fight_mode = msg.get_byte();
                let chase_mode = msg.get_byte();
                let safe_fight = msg.get_byte();
                dispatch(move || game_parse_fight_modes(cid, fight_mode, chase_mode, safe_fight));
            }
            0xA1 => {
                let target_id = msg.get_u32();
                dispatch(move || game_parse_attack(cid, target_id));
            }
            0xA2 => {
                let target_id = msg.get_u32();
                dispatch(move || game_parse_follow(cid, target_id));
            }
            0xA3 => { let tid = msg.get_u32(); dispatch(move || game_parse_invite_to_party(cid, tid)); }
            0xA4 => { let tid = msg.get_u32(); dispatch(move || game_parse_join_party(cid, tid)); }
            0xA5 => { let tid = msg.get_u32(); dispatch(move || game_parse_revoke_party_invite(cid, tid)); }
            0xA6 => { let tid = msg.get_u32(); dispatch(move || game_parse_pass_party_leadership(cid, tid)); }
            0xA7 => dispatch(move || game_handle_leave_party(cid)),
            0xA8 => { let active = msg.get_byte() != 0; dispatch(move || game_parse_enable_shared_party_exp(cid, active)); }
            0xAA => dispatch(move || game_handle_create_private_channel(cid)),
            0xAB => { let name = msg.get_string(None); dispatch(move || game_parse_channel_invite(cid, name)); }
            0xAC => { let name = msg.get_string(None); dispatch(move || game_parse_channel_exclude(cid, name)); }
            0xBE => dispatch(move || game_handle_cancel_attack_and_follow(cid)),
            0xC9 => {}
            0xCA => {
                let container_id = msg.get_byte();
                dispatch(move || game_parse_update_container(cid, container_id));
            }
            0xCB => { msg.skip_bytes(5); }
            0xCC => { msg.skip_bytes(3); }
            0xD2 => dispatch(move || game_handle_request_outfit(cid)),
            0xD3 => {
                let look_type = msg.get_u16();
                let look_head = msg.get_byte();
                let look_body = msg.get_byte();
                let look_legs = msg.get_byte();
                let look_feet = msg.get_byte();
                let look_addons = msg.get_byte();
                dispatch(move || game_parse_set_outfit(cid, look_type, look_head, look_body, look_legs, look_feet, look_addons));
            }
            0xD4 => { msg.skip_bytes(1); }
            0xDC => {
                let name = msg.get_string(None);
                dispatch(move || game_parse_add_vip(cid, name));
            }
            0xDD => { let guid = msg.get_u32(); dispatch(move || game_parse_remove_vip(cid, guid)); }
            0xDE => {
                let _guid = msg.get_u32();
                let _ = msg.get_string(None);
                let _icon = msg.get_u32();
                let _notify = msg.get_byte();
            }
            0xE6 => { let _msg = msg.get_string(None); }
            0xE7 => {}
            0xE8 => {
                let _line = msg.get_string(None);
                let _date = msg.get_string(None);
                let _decription = msg.get_string(None);
                let _comment = msg.get_string(None);
            }
            0xF0 => dispatch(move || game_handle_quest_log(cid)),
            0xF1 => { let quest_id = msg.get_u16(); dispatch(move || game_parse_quest_line(cid, quest_id)); }
            0xF2 => { let _ = msg.get_string(None); let _ = msg.get_string(None); let _ = msg.get_string(None); let _ = msg.get_string(None); }
            0xF3 => {}
            0xF4..=0xF8 => {}
            0xF9 => { let _ = msg.get_u32(); let _ = msg.get_byte(); let _ = msg.get_byte(); }
            _ => debug!(opcode, "unhandled opcode"),
        }
    }

    fn handle_logout(&mut self, conn: &ConnectionHandle) {
        self.accept_packets = false;
        let creature_id = self.creature_id;
        if creature_id != 0 {
            perform_player_logout(creature_id);
        }
        conn.disconnect();
    }

    fn handle_walk(&mut self, conn: &ConnectionHandle, dir: Direction) {
        let creature_id = self.creature_id;
        if creature_id == 0 {
            return;
        }

        let old_pos;
        let new_pos;
        let stackpos: u8;
        let block_msg: Option<&'static [u8]>;
        {
            let game = g_game().lock().unwrap();
            let player = match game.get_player(creature_id) {
                Some(p) => p,
                None => return,
            };
            old_pos = player.base.position;
            new_pos = match game.resolve_walk_destination(old_pos, dir, true) {
                Some(pos) => pos,
                None => {
                    self.send_cancel_walk(conn, dir, b"Sorry, not possible.");
                    return;
                }
            };

            let idx = game.map.get_tile(old_pos)
                .map(|t| t.get_client_index_of_creature(creature_id))
                .unwrap_or(-1);
            stackpos = if idx >= 0 { idx as u8 } else { 0 };

            block_msg = match game.map.get_tile(new_pos) {
                None => Some(b"Sorry, not possible."),
                Some(t) if t.ground.is_none() => Some(b"Sorry, not possible."),
                Some(t) if t.has_flag(TILESTATE_BLOCKSOLID) => Some(b"There is not enough room."),
                _ => None,
            };
        }

        if let Some(msg) = block_msg {
            self.send_cancel_walk(conn, dir, msg);
            return;
        }

        {
            let mut game = g_game().lock().unwrap();
            game.move_creature_position(creature_id, old_pos, new_pos);
            if let Some(player) = game.get_player_mut(creature_id) {
                player.base.direction = dir;
            }
        }

        // Fire step-out on old tile, step-in on new tile.
        crate::events::dispatch::execute_step_event(creature_id, old_pos, new_pos, 1);
        crate::events::dispatch::execute_step_event(creature_id, new_pos, old_pos, 0);

        {
            let game = g_game().lock().unwrap();
            if old_pos.z != new_pos.z {
                // Floor change: remove creature + full map description.
                // Split into separate frames to avoid buffer overflow with 10.98 format.
                let mut output = OutputMessage::new();
                write_remove_tile_creature(&mut output, old_pos, stackpos, creature_id);
                self.crypto.finalize_output(&mut output);
                conn.send_bytes(output.get_output_buffer().to_vec());

                let mut output = OutputMessage::new();
                let known = &mut self.known_creatures;
                if let Some(player) = game.get_player(creature_id) {
                    write_map_description(
                        &mut output,
                        &game,
                        game.get_items(),
                        new_pos,
                        known,
                        creature_id,
                        player,
                    );
                }
                self.crypto.finalize_output(&mut output);
                conn.send_bytes(output.get_output_buffer().to_vec());
            } else {
                let mut output = OutputMessage::new();
                output.add_byte(0x6D);
                write_creature_movement(&mut output, old_pos, new_pos, stackpos, creature_id);
                let known = &mut self.known_creatures;
                append_walk_map_slices(&mut output, &game, game.get_items(), known, old_pos, new_pos);
                self.crypto.finalize_output(&mut output);
                conn.send_bytes(output.get_output_buffer().to_vec());
            }
        }

        sync_known_creatures(creature_id, &self.known_creatures);
        broadcast_creature_move(creature_id, old_pos, new_pos, stackpos);
    }

    async fn handle_use_item(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        DBG_PACKETS.store(true, std::sync::atomic::Ordering::Relaxed);
        let pos = to_map_position(msg.get_position());
        let sprite_id = msg.get_u16();
        let stackpos = msg.get_byte();
        let _index = msg.get_byte();

        let Some((server_id, old_pos, is_pz_locked)) =
            resolve_player_item_for_use(self.creature_id, pos, stackpos, sprite_id, false)
        else {
            send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
            return;
        };

        let (action_id, unique_id, item_index) = {
            let game = g_game().lock().unwrap();
            if let Some(tile) = game.map.get_tile(pos) {
                let item = tile.get_use_item(stackpos);
                match item {
                    Some(item) => (item.action_id, item.unique_id, stackpos as i32 - if tile.ground.is_some() { 1 } else { 0 }),
                    None => (0u16, 0u16, -1i32),
                }
            } else {
                (0, 0, -1)
            }
        };

        if crate::events::dispatch::execute_action_use_async(
            self.creature_id,
            pos,
            server_id,
            item_index,
            action_id,
            unique_id,
            false,
        ).await {
            if let Some(fresh) = load_known_creatures_from_session(self.creature_id) {
                self.known_creatures = fresh;
            }
            return;
        }

        {
            let is_bed = {
                let game = g_game().lock().unwrap();
                game.items.get_item_type(usize::from(server_id)).kind == crate::items::ItemKind::Bed
            };
            if is_bed {
                self.handle_use_bed(conn, pos, server_id);
                return;
            }
        }

        {
            let game = g_game().lock().unwrap();
            let item_type = game.items.get_item_type(usize::from(server_id));
            let is_depot = item_type.kind == crate::items::ItemKind::Depot;
            if item_type.group == crate::items::ItemGroup::Container {
                // Resolve the depot id from the tile item (depot box) if this is a depot.
                let depot_id = if is_depot && pos.x != 0xFFFF {
                    let idx = if item_index >= 0 { item_index as usize } else { 0 };
                    game.map.get_tile(pos)
                        .and_then(|t| t.items.get(idx))
                        .map(|it| it.depot_id as u32)
                        .unwrap_or(0)
                } else { 0 };
                drop(game);
                if is_depot && pos.x != 0xFFFF {
                    self.open_depot(conn, server_id, depot_id);
                } else if pos.x == 0xFFFF {
                    self.open_container_in_inventory(conn, pos.y as u8, server_id);
                } else {
                    let tile_item_index = if item_index >= 0 { item_index as usize } else { 0 };
                    self.open_container_on_tile(conn, pos, tile_item_index, server_id);
                }
                return;
            }
        }

        {
            let game = g_game().lock().unwrap();
            let item_type = game.items.get_item_type(usize::from(server_id));
            if item_type.can_read_text {
                let (item_text, item_writer, item_date, item_count) = if pos.x == 0xFFFF {
                    let slot = usize::from(pos.y);
                    game.get_player(self.creature_id)
                        .and_then(|p| p.inventory_items[slot].as_ref())
                        .map(|it| (it.text.clone(), it.written_by.clone(), it.written_date, it.count))
                        .unwrap_or_default()
                } else {
                    game.map.get_tile(pos)
                        .and_then(|t| t.get_use_item(stackpos))
                        .map(|it| (it.text.clone(), it.written_by.clone(), it.written_date, it.count))
                        .unwrap_or_default()
                };
                let can_write = item_type.can_write_text;
                let max_text_len = item_type.max_text_len;
                let client_id = item_type.client_id;
                let item_count_byte = if item_type.stackable {
                    Some(item_count.clamp(1, 255) as u8)
                } else if item_type.group == ItemGroup::Splash || item_type.group == ItemGroup::Fluid {
                    static FLUID_MAP: [u8; 8] = [0, 6, 7, 2, 1, 5, 4, 9];
                    Some(FLUID_MAP[(item_count & 7) as usize])
                } else {
                    None
                };
                let creature_id = self.creature_id;
                if let Some(player) = game.get_player(creature_id) {
                    let window_text_id = player.window_text_id.wrapping_add(1);
                    drop(game);

                    {
                        let mut game = g_game().lock().unwrap();
                        if let Some(player) = game.get_player_mut(creature_id) {
                            player.window_text_id = window_text_id;
                            if can_write {
                                player.write_item_id = Some(server_id);
                                player.write_item_pos = Some(pos);
                                player.write_item_stack_pos = stackpos;
                                player.max_write_len = max_text_len;
                            } else {
                                player.write_item_id = None;
                                player.write_item_pos = None;
                                player.max_write_len = 0;
                            }
                        }
                    }

                    send_text_window_to_player(
                        creature_id,
                        window_text_id,
                        client_id,
                        item_count_byte,
                        &item_text,
                        &item_writer,
                        item_date,
                        can_write,
                        max_text_len,
                    );
                }
                return;
            }
        }

        let new_pos = {
            let game = g_game().lock().unwrap();
            if USE_TELEPORT_UP_IDS.contains(&server_id) {
                game.map.move_upstairs_position(pos)
            } else if USE_TELEPORT_DOWN_IDS.contains(&server_id) {
                pos.z.checked_add(1).and_then(|z| {
                    let candidate = Position { x: pos.x, y: pos.y, z };
                    game.map.get_tile(candidate).map(|_| candidate)
                })
            } else {
                let item_type = game.items.get_item_type(usize::from(server_id));
                if item_type.floor_change != 0 {
                    game.map.resolve_floor_change_destination(pos)
                } else {
                    None
                }
            }
        };

        let Some(new_pos) = new_pos else {
            return;
        };

        if !can_teleport_to(new_pos, is_pz_locked) {
            send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
            return;
        }

        self.teleport_player(conn, old_pos, new_pos);
    }

    /// Port of the `BedItem` branch of `Actions::internalUseItem` + `BedItem::canUse/trySleep/sleep`.
    fn handle_use_bed(&mut self, conn: &ConnectionHandle, pos: Position, server_id: u16) {
        use crate::map::tile::TileKind;
        let creature_id = self.creature_id;

        let info = {
            let game = g_game().lock().unwrap();
            let Some(tile) = game.map.get_tile(pos) else { return };
            let house_id = match tile.kind {
                TileKind::House { house_id } => Some(house_id),
                _ => None,
            };
            let is_pz = tile.has_flag(TILESTATE_PROTECTIONZONE);
            let sleeper_guid = tile
                .find_item_index_by_server_id(server_id)
                .map(|idx| tile.items[idx].sleeper_guid)
                .unwrap_or(0);

            let it = game.items.get_item_type(usize::from(server_id));
            let transform_to_free = it.transform_to_free;
            let transform_male = it.transform_to_on_use[1];
            let transform_female = it.transform_to_on_use[0];
            let partner_dir = it.bed_partner_dir;

            let Some(player) = game.get_player(creature_id) else { return };
            let p_guid = player.guid;
            let p_account = player.account_number;
            let p_sex = player.sex;
            let premium = player.is_premium();
            let can_edit =
                player.group_flags & crate::creatures::player::PLAYER_FLAG_CAN_EDIT_HOUSES != 0;
            let p_name = player.name.clone();

            let owned_by_account = g_config().get_boolean(BooleanConfig::HouseOwnedByAccount);
            let (house_owner, my_access) = match house_id
                .and_then(|hid| game.map.houses.get_house(hid))
            {
                Some(h) => (
                    h.get_owner(),
                    h.access_level_for(p_guid, p_account, can_edit, owned_by_account, &p_name, "", ""),
                ),
                None => (0, crate::map::houses::AccessHouseLevel::NotInvited),
            };

            BedUseInfo {
                house_id,
                is_pz,
                sleeper_guid,
                transform_to_free,
                transform_male,
                transform_female,
                partner_dir,
                p_guid,
                p_sex,
                premium,
                house_owner,
                my_access,
            }
        };

        let has_house = info.house_id.is_some();
        let can_use = if !has_house || !info.premium || !info.is_pz {
            false
        } else if info.sleeper_guid == 0 {
            true
        } else {
            info.my_access == crate::map::houses::AccessHouseLevel::Owner
        };

        if !can_use {
            let msg: &[u8] = if !has_house {
                b"You can not use this bed."
            } else if !info.premium {
                b"You need a premium account."
            } else {
                b"You cannot use this object."
            };
            send_status_message(&mut self.crypto, conn, msg);
            return;
        }

        if info.sleeper_guid != 0 {
            if info.transform_to_free != 0 && info.house_owner == info.p_guid {
                self.bed_wake_up(pos, server_id);
            }
            let mut game = g_game().lock().unwrap();
            let ppos = game
                .get_player(creature_id)
                .map(|p| p.base.position)
                .unwrap_or(pos);
            game.add_magic_effect(ppos, crate::game::CONST_ME_POFF);
            return;
        }

        self.bed_sleep(pos, server_id, &info);
    }

    fn bed_sleep(&mut self, pos: Position, server_id: u16, info: &BedUseInfo) {
        use crate::creatures::player::PlayerSex;
        let creature_id = self.creature_id;
        let now = (crate::util::otsys_time() / 1000) as u32;
        let partner_pos = next_position(info.partner_dir, pos);

        {
            let mut game = g_game().lock().unwrap();
            let pname = game
                .get_player(creature_id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            let desc = format!("{} is sleeping there.", pname);
            let guid = info.p_guid;

            let partner_sid = game.map.get_tile(partner_pos).and_then(|t| {
                t.items
                    .iter()
                    .find(|it| {
                        game.items.get_item_type(usize::from(it.server_id)).kind
                            == crate::items::ItemKind::Bed
                    })
                    .map(|it| it.server_id)
            });

            game.set_item_sleeper(pos, server_id, guid, now, desc.clone());
            if let Some(psid) = partner_sid {
                game.set_item_sleeper(partner_pos, psid, guid, now, desc);
            }
            game.set_bed_sleeper(guid, pos);
            if let Some(p) = game.get_player_mut(creature_id) {
                p.bed_item_id = Some(server_id);
            }

            let old_pos = game
                .get_player(creature_id)
                .map(|p| p.base.position)
                .unwrap_or(pos);
            if old_pos != pos {
                game.move_creature_position(creature_id, old_pos, pos);
            }
            game.add_magic_effect(pos, crate::game::CONST_ME_SLEEP);

            let sex_transform = match info.p_sex {
                PlayerSex::Male => info.transform_male,
                PlayerSex::Female => info.transform_female,
            };
            let new_id = if sex_transform != 0 {
                sex_transform
            } else {
                info.transform_to_free
            };
            game.transform_tile_item(pos, server_id, new_id);

            if let Some(psid) = partner_sid {
                let pit = game.items.get_item_type(usize::from(psid));
                let p_sex_t = match info.p_sex {
                    PlayerSex::Male => pit.transform_to_on_use[1],
                    PlayerSex::Female => pit.transform_to_on_use[0],
                };
                let p_free = pit.transform_to_free;
                let p_new = if p_sex_t != 0 { p_sex_t } else { p_free };
                game.transform_tile_item(partner_pos, psid, p_new);
            }
        }

        crate::runtime::g_scheduler().add_event(crate::runtime::scheduler::SchedulerTask::new(
            crate::runtime::scheduler::SCHEDULER_MINTICKS,
            move || kick_player_by_id(creature_id),
        ));
    }

    /// Port of `BedItem::wakeUp` for the online-owner re-use path (regen + clear).
    fn bed_wake_up(&mut self, pos: Position, server_id: u16) {
        g_game().lock().unwrap().wake_bed_at(pos, server_id);
    }

    async fn handle_use_item_ex(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let from_pos = to_map_position(msg.get_position());
        let from_sprite_id = msg.get_u16();
        let from_stackpos = msg.get_byte();
        let to_pos = to_map_position(msg.get_position());
        let _to_sprite_id = msg.get_u16();
        let to_stackpos = msg.get_byte();

        let Some((server_id, old_pos, is_pz_locked)) =
            resolve_player_item_for_use(self.creature_id, from_pos, from_stackpos, from_sprite_id, true)
        else {
            send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
            return;
        };

        let (action_id, unique_id, item_index) = {
            let game = g_game().lock().unwrap();
            if let Some(tile) = game.map.get_tile(from_pos) {
                let item = tile.get_use_item(from_stackpos);
                match item {
                    Some(item) => (item.action_id, item.unique_id, from_stackpos as i32 - if tile.ground.is_some() { 1 } else { 0 }),
                    None => (0u16, 0u16, -1i32),
                }
            } else {
                (0, 0, -1)
            }
        };

        if crate::events::dispatch::execute_action_use_ex_async(
            self.creature_id,
            from_pos,
            server_id,
            item_index,
            action_id,
            unique_id,
            to_pos,
            to_stackpos,
            false,
        ).await {
            if let Some(fresh) = load_known_creatures_from_session(self.creature_id) {
                self.known_creatures = fresh;
            }
            return;
        }

        if !ROPE_ITEM_IDS.contains(&server_id) {
            send_status_message(&mut self.crypto, conn, b"You cannot use this object.");
            return;
        }

        let new_pos = {
            let game = g_game().lock().unwrap();
            let Some(tile) = game.map.get_tile(to_pos) else {
                send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
                return;
            };

            let Some(ground) = tile.ground.as_ref() else {
                send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
                return;
            };

            if !ROPE_SPOT_IDS.contains(&ground.server_id) {
                send_status_message(&mut self.crypto, conn, b"You cannot use this object.");
                return;
            }

            game.map.move_upstairs_position(to_pos)
        };

        let Some(new_pos) = new_pos else {
            send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
            return;
        };

        if !can_teleport_to(new_pos, is_pz_locked) {
            send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
            return;
        }

        self.teleport_player(conn, old_pos, new_pos);
    }

    fn teleport_player(&mut self, conn: &ConnectionHandle, old_pos: Position, new_pos: Position) {
        if old_pos == new_pos {
            return;
        }

        let creature_id = self.creature_id;
        let old_stackpos = {
            let mut game = g_game().lock().unwrap();
            let idx = game.map.get_tile(old_pos)
                .map(|t| t.get_client_index_of_creature(creature_id))
                .unwrap_or(-1);
            let old_stack = if idx >= 0 { idx as u8 } else { 0 };
            game.move_creature_position(creature_id, old_pos, new_pos);
            old_stack
        };

        let mut known = std::mem::take(&mut self.known_creatures);
        let mut output = OutputMessage::new();
        write_remove_tile_creature(&mut output, old_pos, old_stackpos, creature_id);
        {
            let game = g_game().lock().unwrap();
            if let Some(player) = game.get_player(creature_id) {
                write_map_description(
                    &mut output,
                    &game,
                    game.get_items(),
                    new_pos,
                    &mut known,
                    creature_id,
                    player,
                );
            }
        }
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
        self.known_creatures = known;

        sync_known_creatures(creature_id, &self.known_creatures);
        broadcast_creature_teleport(creature_id, old_pos, old_stackpos, new_pos);
    }

    fn handle_say(&mut self, msg: &mut NetworkMessage, _conn: &ConnectionHandle) {
        let creature_id = self.creature_id;
        if creature_id == 0 {
            return;
        }

        let speak_type_wire = msg.get_byte();
        let speak_type = crate::net::protocol_version::translate_speak_class_from_client(speak_type_wire);

        let mut receiver = Vec::new();
        let mut channel_id: u16 = 0;

        const TALKTYPE_PRIVATE: u8 = 6;
        const TALKTYPE_CHANNEL_Y: u8 = 7;
        const TALKTYPE_PRIVATE_PN: u8 = 9;
        const TALKTYPE_CHANNEL_R1: u8 = 13;
        const TALKTYPE_PRIVATE_RED: u8 = 14;
        const TALKTYPE_CHANNEL_O: u8 = 15;
        const TALKTYPE_CHANNEL_R2: u8 = 17;

        match speak_type {
            TALKTYPE_PRIVATE | TALKTYPE_PRIVATE_RED => { receiver = msg.get_string(None); }
            TALKTYPE_CHANNEL_Y | TALKTYPE_CHANNEL_R1 | TALKTYPE_CHANNEL_R2 => { channel_id = msg.get_u16(); }
            _ => {}
        }

        let text_bytes = msg.get_string(None);
        if text_bytes.len() > 255 {
            return;
        }

        // Reset idle time on any say (C++ Game::playerSay).
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.idle_time = 0;
            }
        }

        let (name, level, pos) = {
            let game = g_game().lock().unwrap();
            match game.get_player(creature_id) {
                Some(p) => (p.name.clone(), p.level, p.base.position),
                None => return,
            }
        };

        let text = String::from_utf8_lossy(&text_bytes).to_string();

        // Try talk actions first (C++ g_talkActions->playerSaySpell).
        if speak_type <= 3 {
            let param = text.split_once(' ').map(|(_, p)| p).unwrap_or("");
            if crate::events::dispatch::execute_talk_action(creature_id, &text, param, speak_type) {
                return;
            }
            // NOTE: the spell's magic effect (0x83) + heal stats (0xA0) are sent
            // as separate frames; C++ bundles them in one. Bundling here would
            // require routing broadcast_effect_to_spectators through the
            // player-bundle path — deferred (cosmetic, client-tolerant).
            if crate::events::dispatch::execute_spell_say(creature_id, &text) {
                return;
            }
        }

        if speak_type <= 3 || speak_type == TALKTYPE_PRIVATE || speak_type == TALKTYPE_PRIVATE_RED {
            let game = g_game().lock().unwrap();
            if let Some(player) = game.get_player(creature_id) {
                if player.base.has_condition(crate::combat::condition::ConditionType::Muted)
                    && !player.has_flag(crate::creatures::player::PLAYER_FLAG_CANNOT_BE_MUTED)
                {
                    let remaining_secs = player.base.conditions.iter()
                        .find(|c| c.get_type() == crate::combat::condition::ConditionType::Muted)
                        .map(|c| (c.get_ticks() + 999) / 1000)
                        .unwrap_or(0);
                    drop(game);
                    let msg = format!("You are still muted for {} seconds.", remaining_secs);
                    send_status_message_to_player(creature_id, &msg);
                    return;
                }
            }
        }

        let group_flags = {
            let game = g_game().lock().unwrap();
            game.get_player(creature_id).map(|p| p.group_flags).unwrap_or(0)
        };
        let is_access = {
            let game = g_game().lock().unwrap();
            game.get_player(creature_id).map(|p| p.is_access_player()).unwrap_or(false)
        };

        match speak_type {
            1 => {
                broadcast_creature_say(creature_id, pos, &name, level as u16, speak_type, &text_bytes);
                notify_nearby_npcs(creature_id, pos, speak_type, &text);
            }
            2 => {
                broadcast_whisper(creature_id, pos, &name, level as u16, &text_bytes);
                notify_nearby_npcs(creature_id, pos, speak_type, &text);
            }
            3 => {
                {
                    let game = g_game().lock().unwrap();
                    if let Some(player) = game.get_player(creature_id) {
                        if player.base.has_condition(crate::combat::condition::ConditionType::YellTicks) {
                            drop(game);
                            send_status_message_to_player(creature_id, "You are exhausted.");
                            return;
                        }
                    }
                }
                if !is_access && (group_flags & crate::creatures::player::PLAYER_FLAG_IGNORE_YELL_CHECK == 0) {
                    if level < 2 {
                        send_status_message_to_player(creature_id, "You may not yell as long as you are on level 1.");
                        return;
                    }
                    let cond = crate::combat::condition::ConditionGeneric::new(
                        crate::combat::condition::ConditionId::Default,
                        crate::combat::condition::ConditionType::YellTicks,
                        30000,
                        false,
                        0,
                        false,
                    );
                    let effects = {
                        let mut game = g_game().lock().unwrap();
                        if let Some(player) = game.get_player_mut(creature_id) {
                            let base_speed = player.base.base_speed as i32;
                            let conditions = &mut player.base.conditions;
                            crate::combat::condition::add_condition_to_creature(conditions, Box::new(cond), base_speed)
                        } else {
                            vec![]
                        }
                    };
                    if !effects.is_empty() {
                        crate::game::tick::apply_condition_effects(creature_id, &effects);
                    }
                }
                let yell_text: Vec<u8> = text_bytes.iter().map(|b| b.to_ascii_uppercase()).collect();
                broadcast_creature_say(creature_id, pos, &name, level as u16, 3, &yell_text);
                notify_nearby_npcs(creature_id, pos, speak_type, &text);
            }
            TALKTYPE_PRIVATE_PN => {
                notify_nearby_npcs(creature_id, pos, speak_type, &text);
            }
            TALKTYPE_PRIVATE | TALKTYPE_PRIVATE_RED => {
                broadcast_private_message(creature_id, &receiver, &name, level as u16, speak_type, &text_bytes);
            }
            TALKTYPE_CHANNEL_Y | TALKTYPE_CHANNEL_R1 | TALKTYPE_CHANNEL_O | TALKTYPE_CHANNEL_R2 => {
                broadcast_channel_message(channel_id, &name, level as u16, speak_type, &text_bytes);
            }
            12 => {
                if group_flags & crate::creatures::player::PLAYER_FLAG_CAN_BROADCAST == 0 {
                    return;
                }
                broadcast_to_all_players(&name, level as u16, 12, &text_bytes);
            }
            _ => {}
        }
    }

    fn handle_ping_back(&mut self) {
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.last_pong = crate::util::get_milliseconds_time();
        }
    }

    fn handle_turn(&mut self, conn: &ConnectionHandle, dir: Direction) {
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }

        let (pos, stackpos) = {
            let mut game = g_game().lock().unwrap();
            let player = match game.get_player_mut(creature_id) {
                Some(p) => p,
                None => return,
            };
            if player.base.direction == dir { return; }
            player.base.direction = dir;
            let pos = player.base.position;
            let sp = game.map.get_tile(pos)
                .map(|t| t.get_client_index_of_creature(creature_id))
                .unwrap_or(-1);
            (pos, sp)
        };

        let mut output = OutputMessage::new();
        write_creature_turn(&mut output, pos, if stackpos >= 0 { stackpos as u8 } else { 0 }, creature_id, dir);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());

        broadcast_creature_turn(creature_id, pos, stackpos, dir);
    }

    fn parse_auto_walk(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let numdirs = msg.get_byte();
        if numdirs == 0 { return; }

        let mut raw_dirs = Vec::with_capacity(numdirs as usize);
        for _ in 0..numdirs {
            raw_dirs.push(msg.get_byte());
        }

        let mut directions = Vec::with_capacity(numdirs as usize);
        for &raw in raw_dirs.iter().rev() {
            let dir = match raw {
                1 => Direction::East,
                2 => Direction::NorthEast,
                3 => Direction::North,
                4 => Direction::NorthWest,
                5 => Direction::West,
                6 => Direction::SouthWest,
                7 => Direction::South,
                8 => Direction::SouthEast,
                _ => {
                    send_cancel_walk(&mut self.crypto, conn, self.creature_id);
                    return;
                }
            };
            directions.push(dir);
        }

        let creature_id = self.creature_id;
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.idle_time = 0;
                player.base.list_walk_dir = directions;

                if player.base.event_walk != 0 {
                    crate::runtime::g_scheduler().stop_event(player.base.event_walk);
                    player.base.event_walk = 0;
                }
            }
        }

        schedule_auto_walk_step(creature_id);
    }

    fn parse_throw(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let from_pos = to_map_position(msg.get_position());
        let sprite_id = msg.get_u16();
        let from_stackpos = msg.get_byte();
        let to_pos = to_map_position(msg.get_position());
        let _count = msg.get_byte();

        let creature_id = self.creature_id;
        if creature_id == 0 || from_pos == to_pos {
            return;
        }

        // Decode the C++ position scheme: x==0xFFFF means a non-map location.
        // (y & 0x40) => open container (cid = y & 0x0F, slot = z); otherwise an
        // equipment/inventory slot (slot = y). x!=0xFFFF => a map tile.
        let from_is_container = from_pos.x == 0xFFFF && (from_pos.y & 0x40) != 0;
        let to_is_container = to_pos.x == 0xFFFF && (to_pos.y & 0x40) != 0;

        // Any move that touches an open container goes through the container path.
        if from_is_container || to_is_container {
            self.handle_container_move(conn, from_pos, from_stackpos, sprite_id, to_pos);
            return;
        }

        let from_is_inv = from_pos.x == 0xFFFF;
        let to_is_inv = to_pos.x == 0xFFFF;

        if from_is_inv && to_is_inv {
            handle_inventory_to_inventory(creature_id, from_pos, to_pos, self, conn);
            return;
        }

        if from_is_inv && !to_is_inv {
            handle_inventory_to_ground(creature_id, from_pos, to_pos, from_stackpos, self, conn);
            return;
        }

        if !from_is_inv && to_is_inv {
            handle_ground_to_inventory(creature_id, from_pos, to_pos, from_stackpos, sprite_id, self, conn);
            return;
        }

        let game = g_game().lock().unwrap();
        let player_pos = match game.get_player(creature_id) {
            Some(p) => p.base.position,
            None => return,
        };

        if player_pos.z != from_pos.z {
            let msg_text = if from_pos.z > player_pos.z {
                "First go downstairs."
            } else {
                "First go upstairs."
            };
            drop(game);
            let mut output = OutputMessage::new();
            output.add_byte(0xB4);
            output.add_byte(20);
            output.add_string(msg_text.as_bytes());
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
            return;
        }

        let dx = (player_pos.x as i32 - from_pos.x as i32).unsigned_abs();
        let dy = (player_pos.y as i32 - from_pos.y as i32).unsigned_abs();
        if dx > 1 || dy > 1 {
            drop(game);
            return;
        }

        let throw_dx = (from_pos.x as i32 - to_pos.x as i32).unsigned_abs();
        let throw_dy = (from_pos.y as i32 - to_pos.y as i32).unsigned_abs();
        if throw_dx > 7 || throw_dy > 5 || from_pos.z != to_pos.z {
            drop(game);
            let mut output = OutputMessage::new();
            output.add_byte(0xB4);
            output.add_byte(20);
            output.add_string(b"Destination is out of reach.");
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
            return;
        }

        let from_tile = match game.map.get_tile(from_pos) {
            Some(t) => t,
            None => return,
        };

        let item_idx = if from_stackpos == 0 && from_tile.ground.is_some() {
            None
        } else {
            let ground_offset = if from_tile.ground.is_some() { 1 } else { 0 };
            let idx = (from_stackpos as usize).saturating_sub(ground_offset);
            if idx < from_tile.creature_ids.len() {
                let pushed_creature_id = from_tile.creature_ids[from_tile.creature_ids.len() - 1 - idx];
                drop(game);
                self.handle_push_creature(conn, creature_id, pushed_creature_id, to_pos);
                return;
            }
            let item_idx = idx - from_tile.creature_ids.len();
            if item_idx >= from_tile.items.len() {
                drop(game);
                return;
            }
            Some(item_idx)
        };

        let item = match item_idx {
            Some(idx) => from_tile.items[idx].clone(),
            None => {
                drop(game);
                return;
            }
        };

        let it = game.items.get_item_type(item.server_id as usize);
        if it.client_id != sprite_id {
            drop(game);
            return;
        }
        if !it.moveable {
            drop(game);
            let mut output = OutputMessage::new();
            output.add_byte(0xB4);
            output.add_byte(20);
            output.add_string(b"You cannot move this object.");
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
            return;
        }

        let to_tile = match game.map.get_tile(to_pos) {
            Some(t) => t,
            None => {
                drop(game);
                return;
            }
        };
        if to_tile.ground.is_none() {
            drop(game);
            let mut output = OutputMessage::new();
            output.add_byte(0xB4);
            output.add_byte(20);
            output.add_string(b"There is no way.");
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
            return;
        }
        drop(game);

        let delivered_to_mailbox = {
            let mut game = g_game().lock().unwrap();
            let has_mailbox = game
                .map
                .get_tile(to_pos)
                .map(|t| t.has_flag(crate::map::tile::TILESTATE_MAILBOX))
                .unwrap_or(false);
            let delivered = has_mailbox
                && crate::items::special::mailbox::Mailbox::can_send(item.server_id)
                && mailbox_deliver(&mut game, &item);
            if let Some(from_t) = game.map.get_tile_mut(from_pos) {
                if let Some(idx) = item_idx {
                    from_t.items.remove(idx);
                }
            }
            if !delivered {
                if let Some(to_t) = game.map.get_tile_mut(to_pos) {
                    to_t.items.push(item.clone());
                }
            }
            delivered
        };

        let mut game = g_game().lock().unwrap();
        let from_spectators = game.map.get_spectators(from_pos, true, true, 0, 0, 0, 0);
        let to_spectators = game.map.get_spectators(to_pos, true, true, 0, 0, 0, 0);

        let sessions = player_sessions().lock().unwrap();

        for &spec_id in &from_spectators {
            let Some(session) = sessions.get(&spec_id) else { continue };
            let mut output = OutputMessage::new();
            write_remove_tile_thing(&mut output, from_pos, from_stackpos);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }

        if delivered_to_mailbox {
            return;
        }

        for &spec_id in &to_spectators {
            let Some(session) = sessions.get(&spec_id) else { continue };
            let to_tile = game.map.get_tile(to_pos);
            let stackpos = to_tile.map(|t| {
                let gnd: u8 = if t.ground.is_some() { 1 } else { 0 };
                gnd + t.creature_ids.len() as u8 + t.items.len().saturating_sub(1) as u8
            }).unwrap_or(0);
            let mut output = OutputMessage::new();
            write_add_tile_item(&mut output, to_pos, stackpos, &item, &game.items);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }

    fn handle_push_creature(
        &mut self,
        conn: &ConnectionHandle,
        player_id: CreatureId,
        pushed_creature_id: CreatureId,
        to_pos: Position,
    ) {
        use crate::creatures::player::{PLAYER_FLAG_CAN_PUSH_ALL_CREATURES};
        use crate::map::tile::{TILESTATE_BLOCKPATH, TILESTATE_PROTECTIONZONE, TILESTATE_NOPVPZONE};

        let old_pos: Position;
        let old_stackpos: u8;
        {
            let game = g_game().lock().unwrap();

            let player = match game.get_player(player_id) {
                Some(p) => p,
                None => return,
            };
            let player_pos = player.base.position;
            let can_push_all = player.has_flag(PLAYER_FLAG_CAN_PUSH_ALL_CREATURES);

            let creature: &crate::creatures::Creature = match game.get_creature(pushed_creature_id) {
                Some(c) => c,
                None => return,
            };

            if creature.base().movement_blocked {
                send_status_message(&mut self.crypto, conn, b"You cannot move this object.");
                return;
            }

            let creature_pos = creature.position();

            let is_pushable = match creature {
                crate::creatures::Creature::Player(p) => p.is_pushable(),
                crate::creatures::Creature::Monster(m) => m.is_pushable(),
                crate::creatures::Creature::Npc(_) => true,
            };

            if !is_pushable && !can_push_all {
                send_status_message(&mut self.crypto, conn, b"You cannot move this object.");
                return;
            }

            if creature.is_in_ghost_mode() {
                send_status_message(&mut self.crypto, conn, b"You cannot move this object.");
                return;
            }

            let dx = (creature_pos.x as i32 - player_pos.x as i32).unsigned_abs();
            let dy = (creature_pos.y as i32 - player_pos.y as i32).unsigned_abs();
            let dz = if creature_pos.z > player_pos.z {
                (creature_pos.z - player_pos.z) as u32
            } else {
                (player_pos.z - creature_pos.z) as u32
            };
            if dx > 1 || dy > 1 || dz > 0 {
                send_status_message(&mut self.crypto, conn, b"There is no way.");
                return;
            }

            let throw_dx = (creature_pos.x as i32 - to_pos.x as i32).unsigned_abs();
            let throw_dy = (creature_pos.y as i32 - to_pos.y as i32).unsigned_abs();
            let throw_dz = if creature_pos.z > to_pos.z {
                (creature_pos.z - to_pos.z) as u32
            } else {
                (to_pos.z - creature_pos.z) as u32
            };
            if throw_dx > 1 || throw_dy > 1 || throw_dz * 4 > 1 {
                send_status_message(&mut self.crypto, conn, b"Destination is out of range.");
                return;
            }

            let to_tile = match game.map.get_tile(to_pos) {
                Some(t) => t,
                None => {
                    send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
                    return;
                }
            };

            if player_id != pushed_creature_id {
                if to_tile.has_flag(TILESTATE_BLOCKPATH) {
                    send_status_message(&mut self.crypto, conn, b"There is not enough room.");
                    return;
                }

                let creature_tile = game.map.get_tile(creature_pos);
                let creature_on_pz = creature_tile
                    .map(|t| t.has_flag(TILESTATE_PROTECTIONZONE))
                    .unwrap_or(false);
                let creature_on_nopvp = creature_tile
                    .map(|t| t.has_flag(TILESTATE_NOPVPZONE))
                    .unwrap_or(false);
                let dest_is_pz = to_tile.has_flag(TILESTATE_PROTECTIONZONE);
                let dest_is_nopvp = to_tile.has_flag(TILESTATE_NOPVPZONE);

                if (creature_on_pz && !dest_is_pz) || (creature_on_nopvp && !dest_is_nopvp) {
                    send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
                    return;
                }

                for &cid in to_tile.get_creatures() {
                    let Some(tc) = game.get_creature(cid) else { continue };
                    if !tc.is_in_ghost_mode() {
                        send_status_message(&mut self.crypto, conn, b"There is not enough room.");
                        return;
                    }
                }
            }

            if to_tile.ground.is_none() || to_tile.has_flag(TILESTATE_BLOCKSOLID) {
                send_status_message(&mut self.crypto, conn, b"There is not enough room.");
                return;
            }

            old_pos = creature_pos;
            let idx = game.map.get_tile(old_pos)
                .map(|t| t.get_client_index_of_creature(pushed_creature_id))
                .unwrap_or(-1);
            old_stackpos = if idx >= 0 { idx as u8 } else { 0 };
        }

        let creature_class = {
            let game = g_game().lock().unwrap();
            match game.get_creature(pushed_creature_id) {
                Some(crate::creatures::Creature::Player(_)) => "Player",
                Some(crate::creatures::Creature::Monster(_)) => "Monster",
                Some(crate::creatures::Creature::Npc(_)) => "Npc",
                None => return,
            }
        };

        {
            let events = crate::events::g_events().lock().unwrap();
            if !events.event_player_on_move_creature(
                player_id,
                pushed_creature_id,
                creature_class,
                old_pos,
                to_pos,
            ) {
                return;
            }
        }

        {
            let mut game = g_game().lock().unwrap();
            game.move_creature_position(pushed_creature_id, old_pos, to_pos);
        }

        crate::events::dispatch::execute_step_event(pushed_creature_id, old_pos, to_pos, 1);
        crate::events::dispatch::execute_step_event(pushed_creature_id, to_pos, old_pos, 0);

        broadcast_creature_move(pushed_creature_id, old_pos, to_pos, old_stackpos);

        {
            let game = g_game().lock().unwrap();
            if game.get_creature(pushed_creature_id).map(|c| c.is_player()).unwrap_or(false)
                && pushed_creature_id != player_id
            {
                let sessions = player_sessions().lock().unwrap();
                if let Some(session) = sessions.get(&pushed_creature_id) {
                    let known = &mut session.known_creatures.lock().unwrap();
                    let mut output = OutputMessage::new();

                    output.add_byte(0x6D);
                    write_creature_movement(&mut output, old_pos, to_pos, old_stackpos, pushed_creature_id);

                    append_walk_map_slices(&mut output, &game, game.get_items(), known, old_pos, to_pos);

                    finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
                }
            }
        }
    }

    fn parse_look_in_shop(&mut self, msg: &mut NetworkMessage) {
        let sprite_id = msg.get_u16();
        let count = msg.get_byte();
        let player_id = self.creature_id;
        if player_id == 0 { return; }

        let game = g_game().lock().unwrap();
        let player = match game.get_player(player_id) { Some(p) => p, None => return };
        if player.shop_owner_id.is_none() { return; }

        let it = game.items.get_item_id_by_client_id(sprite_id);
        if it.id == 0 { return; }

        let sub_type: i32 = if it.is_fluid_container() || it.is_splash() {
            crate::util::client_fluid_to_server(count) as i32
        } else {
            count as i32
        };

        let in_shop = player.shop_item_list.iter().any(|s| s.item_id as u16 == it.id && (s.buy_price > 0 || s.sell_price > 0));
        if !in_shop { return; }

        let description = crate::items::description::get_item_description(it, -1, sub_type as u32);
        drop(game);

        let desc_bytes = description.into_bytes();
        send_packet_to_player(player_id, move |output: &mut OutputMessage| {
            output.add_byte(0xB4);
            output.add_byte(25); // MESSAGE_INFO_DESCR
            output.add_string(&desc_bytes);
        });
    }

    fn parse_player_purchase(&mut self, msg: &mut NetworkMessage) {
        let sprite_id = msg.get_u16();
        let count = msg.get_byte();
        let amount = msg.get_byte();
        let ignore_cap = msg.get_byte() != 0;
        let in_backpacks = msg.get_byte() != 0;
        let player_id = self.creature_id;
        if amount == 0 || amount > 100 { return; }
        let (cb, item_id, sub_type) = {
            let game = g_game().lock().unwrap();
            let player = match game.get_player(player_id) { Some(p) => p, None => return };
            if player.shop_owner_id.is_none() { return; }
            let cb = player.purchase_callback;
            let it = game.items.get_item_id_by_client_id(sprite_id);
            if it.id == 0 { return; }
            let st = if it.is_fluid_container() || it.is_splash() {
                crate::util::client_fluid_to_server(count)
            } else { count };
            // Verify item is in shop
            let in_shop = player.shop_item_list.iter().any(|s| s.item_id as u16 == it.id);
            if !in_shop { return; }
            (cb, it.id, st)
        };
        if cb == -1 { return; }
        let lua = crate::lua::script::g_lua();
        let registry = crate::events::registry::g_script_registry().lock().unwrap();
        let func = registry.get_callback_function(lua, cb);
        drop(registry);
        if let Some(func) = func {
            let creature_tbl = crate::lua::registrations::push_creature_ref(lua, player_id, "Player").ok();
            let _ = func.call::<()>((creature_tbl, item_id as i64, sub_type as i64, amount as i64, ignore_cap, in_backpacks));
        }
        send_sale_item_list(player_id, &{
            let game = g_game().lock().unwrap();
            game.get_player(player_id).map(|p| p.shop_item_list.clone()).unwrap_or_default()
        });
    }

    fn parse_player_sale(&mut self, msg: &mut NetworkMessage) {
        let sprite_id = msg.get_u16();
        let count = msg.get_byte();
        let amount = msg.get_byte();
        let ignore_equipped = msg.get_byte() != 0;
        let player_id = self.creature_id;
        if amount == 0 || amount > 100 { return; }
        let (cb, item_id, sub_type) = {
            let game = g_game().lock().unwrap();
            let player = match game.get_player(player_id) { Some(p) => p, None => return };
            if player.shop_owner_id.is_none() { return; }
            let cb = player.sale_callback;
            let it = game.items.get_item_id_by_client_id(sprite_id);
            if it.id == 0 { return; }
            let st = if it.is_fluid_container() || it.is_splash() {
                crate::util::client_fluid_to_server(count)
            } else { count };
            (cb, it.id, st)
        };
        if cb == -1 { return; }
        let lua = crate::lua::script::g_lua();
        let registry = crate::events::registry::g_script_registry().lock().unwrap();
        let func = registry.get_callback_function(lua, cb);
        drop(registry);
        if let Some(func) = func {
            let creature_tbl = crate::lua::registrations::push_creature_ref(lua, player_id, "Player").ok();
            let _ = func.call::<()>((creature_tbl, item_id as i64, sub_type as i64, amount as i64, ignore_equipped));
        }
        send_sale_item_list(player_id, &{
            let game = g_game().lock().unwrap();
            game.get_player(player_id).map(|p| p.shop_item_list.clone()).unwrap_or_default()
        });
    }

    fn parse_request_trade(&mut self, msg: &mut NetworkMessage) {
        let pos = to_map_position(msg.get_position());
        let sprite_id = msg.get_u16();
        let stackpos = msg.get_byte();
        let trade_player_id = msg.get_u32();
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        self.player_request_trade(pos, stackpos, sprite_id, trade_player_id);
    }

    /// Mirrors C++ `Game::playerRequestTrade` + `internalStartTrade`.
    fn player_request_trade(&mut self, pos: Position, stackpos: u8, sprite_id: u16, trade_player_id: u32) {
        use crate::creatures::player::TradeState;
        let creature_id = self.creature_id;

        // Resolve + validate partner, item, and trade preconditions under one lock.
        let (my_name, partner_name, item, loc, partner_was_idle) = {
            let game = g_game().lock().unwrap();
            let Some(player) = game.get_player(creature_id) else { return };
            let player_pos = player.base.position;
            let my_name = player.name.clone();

            // Partner must exist, not be self, and be within 2 tiles.
            if trade_player_id == creature_id { return; }
            let Some(partner) = game.get_player(trade_player_id) else {
                drop(game);
                send_status_message_to_player(creature_id, "Select a player to trade with.");
                return;
            };
            let ppos = partner.base.position;
            if ppos.z != player_pos.z
                || (ppos.x as i32 - player_pos.x as i32).abs() > 2
                || (ppos.y as i32 - player_pos.y as i32).abs() > 2
            {
                drop(game);
                send_status_message_to_player(creature_id, "Destination is out of reach.");
                return;
            }
            let partner_name = partner.name.clone();
            let partner_state = partner.trade_state;
            let partner_partner = partner.trade_partner_id;

            // Resolve the offered item (clone + location).
            let ep = MoveEndpoint::decode(pos, stackpos);
            let Some((item, loc)) = peek_trade_item(&game, creature_id, &ep) else {
                drop(game);
                send_status_message_to_player(creature_id, "Sorry, not possible.");
                return;
            };

            // Item must match sprite, be pickupable, and have no unique id.
            let it = game.items.get_item_type(usize::from(item.server_id));
            if it.client_id != sprite_id || !it.pickupable || item.unique_id != 0 {
                drop(game);
                send_status_message_to_player(creature_id, "Sorry, not possible.");
                return;
            }

            // "Already trading" checks (C++ internalStartTrade guards).
            let my_state = player.trade_state;
            if my_state != TradeState::None
                && !(my_state == TradeState::Acknowledge && player.trade_partner_id == Some(trade_player_id))
            {
                drop(game);
                send_status_message_to_player(creature_id, "You are already trading.");
                return;
            }
            if partner_state != TradeState::None && partner_partner != Some(creature_id) {
                drop(game);
                send_status_message_to_player(creature_id, "This player is already trading.");
                return;
            }

            (my_name, partner_name, item, loc, partner_state == TradeState::None)
        };

        // Commit trade state.
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.trade_partner_id = Some(trade_player_id);
                player.trade_item_id = Some(item.server_id);
                player.trade_item = Some(item.clone());
                player.trade_item_loc = Some(loc);
                player.trade_state = TradeState::Initiated;
            }
        }

        // Requester always sees their own offer (0x7D).
        send_trade_offer(creature_id, &my_name, &item, true);

        if partner_was_idle {
            // Partner is notified and acknowledges; sees nothing until they offer.
            {
                let mut game = g_game().lock().unwrap();
                if let Some(partner) = game.get_player_mut(trade_player_id) {
                    partner.trade_state = TradeState::Acknowledge;
                    partner.trade_partner_id = Some(creature_id);
                }
            }
            send_status_message_to_player(
                trade_player_id,
                &format!("{my_name} wants to trade with you."),
            );
        } else {
            // Counter-offer: both see each other's items (0x7E).
            let counter = {
                let game = g_game().lock().unwrap();
                game.get_player(trade_player_id).and_then(|p| p.trade_item.clone())
            };
            if let Some(counter_item) = counter {
                send_trade_offer(creature_id, &partner_name, &counter_item, false);
            }
            send_trade_offer(trade_player_id, &my_name, &item, false);
        }
    }

    fn parse_look_in_trade(&mut self, msg: &mut NetworkMessage) {
        let counter_offer = msg.get_byte() == 0x01;
        let index = msg.get_byte();
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }

        // Resolve the item being inspected (own or partner's offer).
        let item = {
            let game = g_game().lock().unwrap();
            let Some(player) = game.get_player(creature_id) else { return };
            let Some(partner_id) = player.trade_partner_id else { return };
            let source = if counter_offer {
                game.get_player(partner_id).and_then(|p| p.trade_item.clone())
            } else {
                player.trade_item.clone()
            };
            let Some(root) = source else { return };
            // index 0 = the offered item itself; >0 walks into a container.
            if index == 0 {
                Some(root)
            } else {
                let mut flat: Vec<crate::map::tile::MapItem> = Vec::new();
                fn collect(it: &crate::map::tile::MapItem, out: &mut Vec<crate::map::tile::MapItem>) {
                    for child in &it.children {
                        out.push(child.clone());
                        collect(child, out);
                    }
                }
                collect(&root, &mut flat);
                flat.get((index as usize).saturating_sub(1)).cloned()
            }
        };

        if let Some(item) = item {
            let desc = {
                let game = g_game().lock().unwrap();
                let it = game.items.get_item_type(usize::from(item.server_id));
                format!("You see {}.", if it.name.is_empty() { "an item".to_owned() } else { format!("a {}", it.name) })
            };
            send_status_message_to_player(creature_id, &desc);
        }
    }

    async fn handle_use_with_creature(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let from_pos = to_map_position(msg.get_position());
        let sprite_id = msg.get_u16();
        let from_stackpos = msg.get_byte();
        let target_creature_id = msg.get_u32();

        let creature_id = self.creature_id;

        let (creature_pos, to_stackpos) = {
            let game = g_game().lock().unwrap();
            let Some(player) = game.get_player(creature_id) else { return };
            let Some(creature) = game.get_creature(target_creature_id) else { return };

            let cpos = match creature {
                crate::creatures::Creature::Player(p) => p.base.position,
                crate::creatures::Creature::Monster(m) => m.base.position,
                crate::creatures::Creature::Npc(n) => n.base.position,
            };

            if player.base.position.z != cpos.z
                || (player.base.position.x as i32 - cpos.x as i32).unsigned_abs() > (MAX_VIEWPORT_X - 1) as u32
                || (player.base.position.y as i32 - cpos.y as i32).unsigned_abs() > (MAX_VIEWPORT_Y - 1) as u32
            {
                return;
            }

            let is_hotkey = from_pos.x == 0xFFFF && from_pos.y == 0 && from_pos.z == 0;
            if !crate::config::g_config().get_boolean(crate::config::BooleanConfig::AimbotHotkeyEnabled)
                && (matches!(creature, crate::creatures::Creature::Player(_)) || is_hotkey)
            {
                send_status_message(&mut self.crypto, conn, b"You are not allowed to shoot directly on players.");
                return;
            }

            let stackpos = game.map.get_tile(cpos)
                .map(|tile| {
                    let idx = tile.get_client_index_of_creature(target_creature_id);
                    if idx >= 0 { idx as u8 } else { 0 }
                })
                .unwrap_or(0);

            (cpos, stackpos)
        };

        let Some((server_id, _player_pos, _is_pz_locked)) =
            resolve_player_item_for_use(creature_id, from_pos, from_stackpos, sprite_id, true)
        else {
            send_status_message(&mut self.crypto, conn, b"You cannot use this object.");
            return;
        };

        let (action_id, unique_id, item_index) = {
            let game = g_game().lock().unwrap();
            if from_pos.x == 0xFFFF {
                (0u16, 0u16, from_stackpos as i32)
            } else if let Some(tile) = game.map.get_tile(from_pos) {
                let item = tile.get_use_item(from_stackpos);
                match item {
                    Some(item) => (item.action_id, item.unique_id, from_stackpos as i32 - if tile.ground.is_some() { 1 } else { 0 }),
                    None => (0u16, 0u16, -1i32),
                }
            } else {
                (0, 0, -1)
            }
        };

        let is_hotkey = from_pos.x == 0xFFFF && from_pos.y == 0 && from_pos.z == 0;

        if crate::events::dispatch::execute_action_use_ex_async(
            creature_id,
            from_pos,
            server_id,
            item_index,
            action_id,
            unique_id,
            creature_pos,
            to_stackpos,
            is_hotkey,
        ).await {
            return;
        }

        send_status_message(&mut self.crypto, conn, b"You cannot use this object.");
    }

    fn parse_rotate_item(&mut self, msg: &mut NetworkMessage) {
        let pos = to_map_position(msg.get_position());
        let _sprite_id = msg.get_u16();
        let stackpos = msg.get_byte();

        if pos.x == 0xFFFF {
            return;
        }

        let mut game = g_game().lock().unwrap();

        let (old_id, item_idx) = {
            let tile = match game.map.get_tile(pos) {
                Some(t) => t,
                None => return,
            };
            let ground_offset = if tile.ground.is_some() { 1u8 } else { 0 };
            let creature_offset = tile.creature_ids.len() as u8;
            if stackpos < ground_offset + creature_offset {
                return;
            }
            let item_idx = (stackpos - ground_offset - creature_offset) as usize;
            if item_idx >= tile.items.len() {
                return;
            }
            (tile.items[item_idx].server_id, item_idx)
        };

        let rotate_to = game.items.get_item_type(old_id as usize).rotate_to;
        if rotate_to == 0 {
            return;
        }

        if let Some(tile) = game.map.get_tile_mut(pos) {
            tile.items[item_idx].server_id = rotate_to;
        }

        let spectators = game.map.get_spectators(pos, true, true, 0, 0, 0, 0);
        let items_ref = game.items.clone();
        drop(game);

        let sessions = player_sessions().lock().unwrap();
        for &spec_id in &spectators {
            let Some(session) = sessions.get(&spec_id) else { continue };
            let mut output = OutputMessage::new();
            let item = crate::map::tile::MapItem {
                server_id: rotate_to,
                ..Default::default()
            };
            write_update_tile_item(&mut output, pos, stackpos, &item, &items_ref);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }

    fn open_container_on_tile(&mut self, conn: &ConnectionHandle, pos: Position, tile_item_index: usize, server_id: u16) {
        use crate::creatures::player::ContainerParent;

        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(self.creature_id) else { return };

        if let Some(existing_cid) = player.get_container_id_by_tile(pos, tile_item_index) {
            drop(game);
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(self.creature_id) {
                player.close_container(existing_cid);
            }
            drop(game);
            let mut output = OutputMessage::new();
            write_close_container(&mut output, existing_cid);
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
            return;
        }

        let Some(cid) = player.get_free_container_id() else {
            drop(game);
            send_status_message(&mut self.crypto, conn, b"You cannot open any more containers.");
            return;
        };

        let Some(tile) = game.map.get_tile(pos) else {
            return;
        };

        let Some(container_item) = tile.items.get(tile_item_index) else {
            return;
        };

        let item_type = game.items.get_item_type(usize::from(server_id));
        let name = if container_item.name.is_empty() {
            item_type.name.clone()
        } else {
            container_item.name.clone()
        };
        let capacity = item_type.max_items.min(255) as u8;
        let container_item_clone = container_item.clone();
        let children_clone: Vec<crate::map::tile::MapItem> = container_item.children.clone();
        let items_ref = game.items.clone();
        drop(game);

        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(self.creature_id) {
            player.add_container(cid, ContainerParent::Tile(pos, tile_item_index));
        }
        drop(game);

        let mut output = OutputMessage::new();
        write_container(&mut output, cid, &container_item_clone, &items_ref, &name, capacity, false, &children_clone);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    /// Move an item where the source and/or destination is an open container
    /// window. Mirrors the container branches of C++ `Game::playerMoveItem` +
    /// `internalGetCylinder`. Endpoints are decoded from the Tibia position
    /// scheme (see `parse_throw`). Item attributes/children are preserved.
    fn handle_container_move(
        &mut self,
        conn: &ConnectionHandle,
        from_pos: Position,
        from_stackpos: u8,
        sprite_id: u16,
        to_pos: Position,
    ) {
        let creature_id = self.creature_id;
        let from = MoveEndpoint::decode(from_pos, from_stackpos);
        let to = MoveEndpoint::decode(to_pos, 0);

        let mut game = g_game().lock().unwrap();

        // Phase 1: extract the source item (owned, with its full child tree).
        let item = match extract_move_item(&mut game, creature_id, &from) {
            Some(i) => i,
            None => {
                drop(game);
                send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
                return;
            }
        };

        // Optional client-sprite sanity check (skip when 0 / unknown).
        if sprite_id != 0 {
            let client_id = game.items.get_item_type(usize::from(item.server_id)).client_id;
            if client_id != sprite_id {
                // Sprite mismatch — put it back and bail.
                insert_move_item(&mut game, creature_id, &from, item);
                drop(game);
                send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
                return;
            }
        }

        // Phase 2: insert into the destination; on failure, restore to source.
        if !insert_move_item(&mut game, creature_id, &to, item.clone()) {
            insert_move_item(&mut game, creature_id, &from, item);
            drop(game);
            send_status_message(&mut self.crypto, conn, b"Sorry, not possible.");
            return;
        }

        drop(game);

        // Refresh every surface the move touched for the acting player.
        self.refresh_move_endpoint(conn, &from);
        if to != from {
            self.refresh_move_endpoint(conn, &to);
        }
    }

    /// Re-sync the client view of a move endpoint after a container move.
    /// Containers are refreshed with a fresh 0x6E; inventory slots with
    /// 0x78/0x79; ground tiles with a full-tile 0x69 update to spectators.
    fn refresh_move_endpoint(&mut self, conn: &ConnectionHandle, ep: &MoveEndpoint) {
        match *ep {
            MoveEndpoint::Container { cid, .. } => {
                self.resend_open_container(conn, cid);
            }
            MoveEndpoint::Inventory { slot } => {
                let (sid, count) = {
                    let game = g_game().lock().unwrap();
                    match game.get_player(self.creature_id) {
                        Some(p) => (p.inventory[slot], p.inventory_count[slot]),
                        None => (None, 0),
                    }
                };
                let client_id = sid.map(|s| {
                    g_game().lock().unwrap().items.get_item_type(usize::from(s)).client_id
                });
                let s = slot as u8;
                let c = count.max(1) as u8;
                send_packet_to_player(self.creature_id, move |output: &mut OutputMessage| {
                    match client_id {
                        Some(cid) => { output.add_byte(0x78); output.add_byte(s); output.add_u16(cid); output.add_byte(c); }
                        None => { output.add_byte(0x79); output.add_byte(s); }
                    }
                });
            }
            MoveEndpoint::Ground { pos, .. } => {
                // Refresh the acting player's view of the tile (0x69 UpdateTile).
                let mut output = OutputMessage::new();
                {
                    let game = g_game().lock().unwrap();
                    output.add_byte(0x69);
                    output.add_position(pos.x, pos.y, pos.z);
                    if let Some(tile) = game.map.get_tile(pos) {
                        let mut known = std::mem::take(&mut self.known_creatures);
                        write_tile_description(&mut output, &game, tile, game.get_items(), &mut known, None);
                        self.known_creatures = known;
                        output.add_byte(0x00);
                        output.add_byte(0xFF);
                    } else {
                        output.add_byte(0x01);
                        output.add_byte(0xFF);
                    }
                }
                self.crypto.finalize_output(&mut output);
                conn.send_bytes(output.get_output_buffer().to_vec());
            }
        }
    }

    /// Re-send an open container window (0x6E) to refresh its contents.
    fn resend_open_container(&mut self, conn: &ConnectionHandle, cid: u8) {
        use crate::creatures::player::ContainerParent;
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(self.creature_id) else { return };
        let Some(oc) = player.get_container_by_id(cid) else { return };
        let parent = oc.parent.clone();

        // Depot chests have no wrapping item — rebuild the synthetic chest.
        if let ContainerParent::Depot(depot_id) = parent {
            let children: Vec<crate::map::tile::MapItem> =
                player.depot_items.get(&depot_id).cloned().unwrap_or_default();
            let items_ref = game.items.clone();
            // Use a depot item id for the header if any child exists, else a generic.
            let chest = crate::map::tile::MapItem { server_id: 2594, ..crate::map::tile::MapItem::default() };
            let capacity = 255u8;
            drop(game);
            let mut output = OutputMessage::new();
            write_container(&mut output, cid, &chest, &items_ref, "Depot chest", capacity, false, &children);
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
            return;
        }

        // Resolve the container item (with children) from its storage root.
        let Some((root, path, _scroll)) = resolve_container_storage(player, cid) else { return };
        let container_item = match container_item_ref(&game, self.creature_id, &root, &path) {
            Some(it) => it.clone(),
            None => return,
        };
        let item_type = game.items.get_item_type(usize::from(container_item.server_id));
        let name = if container_item.name.is_empty() { item_type.name.clone() } else { container_item.name.clone() };
        let capacity = item_type.max_items.min(255) as u8;
        let has_parent = matches!(parent, ContainerParent::Container(_, _));
        let children = container_item.children.clone();
        let items_ref = game.items.clone();
        drop(game);

        let mut output = OutputMessage::new();
        write_container(&mut output, cid, &container_item, &items_ref, &name, capacity, has_parent, &children);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    /// Open a player's depot chest for `depot_id` as a container, showing the
    /// stored items. Mirrors C++ actions.cpp opening the depot locker. (We open
    /// the chest contents directly rather than the locker→chest wrapper.)
    fn open_depot(&mut self, conn: &ConnectionHandle, server_id: u16, depot_id: u32) {
        use crate::creatures::player::ContainerParent;

        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(self.creature_id) else { return };

        // Toggle closed if this depot is already open.
        let existing = player.open_containers.iter()
            .find(|(_, oc)| matches!(oc.parent, ContainerParent::Depot(d) if d == depot_id))
            .map(|(&cid, _)| cid);
        if let Some(existing_cid) = existing {
            drop(game);
            let mut game = g_game().lock().unwrap();
            if let Some(p) = game.get_player_mut(self.creature_id) { p.close_container(existing_cid); }
            drop(game);
            let mut output = OutputMessage::new();
            write_close_container(&mut output, existing_cid);
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
            return;
        }

        let Some(cid) = player.get_free_container_id() else {
            drop(game);
            send_status_message(&mut self.crypto, conn, b"You cannot open any more containers.");
            return;
        };

        let item_type = game.items.get_item_type(usize::from(server_id));
        let capacity = item_type.max_items.clamp(1, 255) as u8;
        let children: Vec<crate::map::tile::MapItem> =
            player.depot_items.get(&depot_id).cloned().unwrap_or_default();
        let chest = crate::map::tile::MapItem { server_id, ..crate::map::tile::MapItem::default() };
        let items_ref = game.items.clone();
        drop(game);

        let mut game = g_game().lock().unwrap();
        if let Some(p) = game.get_player_mut(self.creature_id) {
            p.add_container(cid, ContainerParent::Depot(depot_id));
            p.set_last_depot_id(depot_id as i16);
        }
        drop(game);

        let mut output = OutputMessage::new();
        write_container(&mut output, cid, &chest, &items_ref, "Depot chest", capacity, false, &children);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn open_container_in_inventory(&mut self, conn: &ConnectionHandle, slot: u8, server_id: u16) {
        use crate::creatures::player::ContainerParent;

        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(self.creature_id) else { return };

        // Toggle: clicking an already-open inventory container closes it.
        if let Some(existing_cid) = player.get_container_id_by_inventory(slot) {
            drop(game);
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(self.creature_id) {
                player.close_container(existing_cid);
            }
            drop(game);
            let mut output = OutputMessage::new();
            write_close_container(&mut output, existing_cid);
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
            return;
        }

        let Some(cid) = player.get_free_container_id() else {
            drop(game);
            send_status_message(&mut self.crypto, conn, b"You cannot open any more containers.");
            return;
        };

        let Some(Some(container_item)) = player.inventory_items.get(usize::from(slot)) else {
            return;
        };

        let item_type = game.items.get_item_type(usize::from(server_id));
        let name = if container_item.name.is_empty() {
            item_type.name.clone()
        } else {
            container_item.name.clone()
        };
        let capacity = item_type.max_items.min(255) as u8;
        let container_item_clone = container_item.clone();
        let children_clone: Vec<crate::map::tile::MapItem> = container_item.children.clone();
        let items_ref = game.items.clone();
        drop(game);

        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(self.creature_id) {
            player.add_container(cid, ContainerParent::Inventory(slot));
        }
        drop(game);

        let mut output = OutputMessage::new();
        write_container(&mut output, cid, &container_item_clone, &items_ref, &name, capacity, false, &children_clone);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_close_container(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let cid = msg.get_byte();
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(self.creature_id) {
            player.close_container(cid);
        }
        drop(game);

        let mut output = OutputMessage::new();
        write_close_container(&mut output, cid);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_up_arrow_container(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        use crate::creatures::player::ContainerParent;

        let cid = msg.get_byte();
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(self.creature_id) else { return };
        let Some(oc) = player.get_container_by_id(cid) else { return };

        match &oc.parent {
            ContainerParent::Container(parent_cid, _child_idx) => {
                let _parent_cid = *parent_cid;
                drop(game);
                // Deferred: resolve parent container and send it (nested container navigation)
            }
            _ => {
                drop(game);
                let mut game_w = g_game().lock().unwrap();
                if let Some(player) = game_w.get_player_mut(self.creature_id) {
                    player.close_container(cid);
                }
                drop(game_w);

                let mut output = OutputMessage::new();
                write_close_container(&mut output, cid);
                self.crypto.finalize_output(&mut output);
                conn.send_bytes(output.get_output_buffer().to_vec());
            }
        }
    }

    fn parse_text_window(&mut self, msg: &mut NetworkMessage) {
        let window_text_id = msg.get_u32();
        let new_text = msg.get_string(None);

        let creature_id = self.creature_id;
        if creature_id == 0 {
            return;
        }

        let mut game = g_game().lock().unwrap();
        let (write_item_id, write_item_pos, write_item_stack_pos, max_write_len, internal_window_text_id) = {
            let Some(player) = game.get_player(creature_id) else { return };
            (
                player.write_item_id,
                player.write_item_pos,
                player.write_item_stack_pos,
                player.max_write_len,
                player.window_text_id,
            )
        };

        if new_text.len() > max_write_len as usize || window_text_id != internal_window_text_id {
            return;
        }

        let Some(_server_id) = write_item_id else {
            send_status_message_to_player(creature_id, "Sorry, not possible.");
            return;
        };

        let Some(item_pos) = write_item_pos else {
            send_status_message_to_player(creature_id, "Sorry, not possible.");
            return;
        };

        let player_pos = match game.get_player(creature_id) {
            Some(p) => p.base.position,
            None => return,
        };

        if item_pos.x != 0xFFFF {
            let dx = (item_pos.x as i32 - player_pos.x as i32).unsigned_abs();
            let dy = (item_pos.y as i32 - player_pos.y as i32).unsigned_abs();
            let dz = (item_pos.z as i32 - player_pos.z as i32).unsigned_abs();
            if dx > 1 || dy > 1 || dz > 0 {
                send_status_message_to_player(creature_id, "Sorry, not possible.");
                return;
            }
        }

        let text_str = String::from_utf8_lossy(&new_text).to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;
        let player_name = game.get_player(creature_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();

        if item_pos.x == 0xFFFF {
            let slot = usize::from(item_pos.y);
            let cur_sid = game.get_player(creature_id)
                .and_then(|p| p.inventory_items[slot].as_ref())
                .map(|it| it.server_id)
                .unwrap_or(0);
            let write_once = game.items.get_item_type(usize::from(cur_sid)).write_once_item_id;
            if let Some(player) = game.get_player_mut(creature_id) {
                if let Some(ref mut item) = player.inventory_items[slot] {
                    if !text_str.is_empty() {
                        if item.text != text_str {
                            item.text = text_str;
                            item.written_by = player_name;
                            item.written_date = now;
                        }
                    } else {
                        item.text.clear();
                        item.written_by.clear();
                        item.written_date = 0;
                    }
                    if write_once != 0 {
                        item.server_id = write_once;
                    }
                }
                player.write_item_id = None;
                player.write_item_pos = None;
                player.max_write_len = 0;
            }
        } else {
            let stack = write_item_stack_pos;
            let idx = game.map.get_tile(item_pos)
                .map(|t| if t.ground.is_some() { stack as usize - 1 } else { stack as usize })
                .unwrap_or(0);
            let cur_sid = game.map.get_tile(item_pos)
                .and_then(|t| t.items.get(idx))
                .map(|it| it.server_id)
                .unwrap_or(0);
            let write_once = game.items.get_item_type(usize::from(cur_sid)).write_once_item_id;

            if let Some(tile) = game.map.get_tile_mut(item_pos) {
                if let Some(item) = tile.items.get_mut(idx) {
                    if !text_str.is_empty() {
                        if item.text != text_str {
                            item.text = text_str;
                            item.written_by = player_name;
                            item.written_date = now;
                        }
                    } else {
                        item.text.clear();
                        item.written_by.clear();
                        item.written_date = 0;
                    }
                    if write_once != 0 {
                        item.server_id = write_once;
                    }
                }
            }

            if let Some(player) = game.get_player_mut(creature_id) {
                player.write_item_id = None;
                player.write_item_pos = None;
                player.max_write_len = 0;
            }

            if write_once != 0 && write_once != cur_sid {
                let stackpos = game.map.get_tile(item_pos)
                    .map(|tile| tile.item_client_stackpos(idx))
                    .unwrap_or(0);
                let new_client_id = game.items.get_item_type(usize::from(write_once)).client_id;
                let spectators = game.map.get_spectators(item_pos, true, true, 0, 0, 0, 0);
                for spec_id in spectators {
                    send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                        output.add_byte(0x6B);
                        output.add_position(item_pos.x, item_pos.y, item_pos.z);
                        output.add_byte(stackpos);
                        output.add_u16(new_client_id);
                    });
                }
            }
        }
    }

    fn parse_house_window(&mut self, msg: &mut NetworkMessage) {
        let list_id_from_client = msg.get_byte();
        let window_text_id = msg.get_u32();
        let text = msg.get_string(None);

        let creature_id = self.creature_id;
        if creature_id == 0 {
            return;
        }

        let mut game = g_game().lock().unwrap();

        let (edit_house_id, internal_window_text_id, internal_list_id, player_guid, player_account_id, can_edit_houses, player_name) = {
            let Some(player) = game.get_player(creature_id) else { return };
            let can_edit = crate::world::groups::access_for_group_id(player.group_id);
            (
                player.edit_house_id,
                player.window_text_id,
                player.edit_list_id,
                player.guid,
                player.account_number,
                can_edit,
                player.name.clone(),
            )
        };

        let Some(house_id) = edit_house_id else {
            return;
        };

        if window_text_id != internal_window_text_id || list_id_from_client != 0 {
            if let Some(player) = game.get_player_mut(creature_id) {
                player.edit_house_id = None;
            }
            return;
        }

        let can_edit = game.map.houses.get_house(house_id).map(|house| {
            let access_level = house.access_level_for(
                player_guid,
                player_account_id,
                can_edit_houses,
                false,
                &player_name,
                "",
                "",
            );
            house.can_edit_access_list(internal_list_id, access_level)
        }).unwrap_or(false);
        if can_edit {
            let text_str = String::from_utf8_lossy(&text).to_string();
            if let Some(house_mut) = game.map.houses.get_house_mut(house_id) {
                house_mut.set_access_list(internal_list_id, &text_str);
            }
        }

        if let Some(player) = game.get_player_mut(creature_id) {
            player.edit_house_id = None;
        }
    }

    fn parse_look_at(&mut self, msg: &mut NetworkMessage, _conn: &ConnectionHandle) {
        let pos = to_map_position(msg.get_position());
        let _sprite_id = msg.get_u16();
        let stackpos = msg.get_byte();

        let creature_id = self.creature_id;
        if creature_id == 0 {
            return;
        }

        let game = g_game().lock().unwrap();
        let player_pos = match game.get_player(creature_id) {
            Some(p) => p.base.position,
            None => return,
        };

        let tile = match game.map.get_tile(pos) {
            Some(t) => t,
            None => return,
        };

        enum ThingRef {
            Creature(u32, &'static str),
            Item(u16, u32),
        }

        let thing = {
            let mut found_creature: Option<ThingRef> = None;
            for &cid in &tile.creature_ids {
                if let Some(c) = game.get_creature(cid) {
                    let class_name = match c {
                        crate::creatures::Creature::Player(_) => "Player",
                        crate::creatures::Creature::Monster(_) => "Monster",
                        crate::creatures::Creature::Npc(_) => "Npc",
                    };
                    found_creature = Some(ThingRef::Creature(cid, class_name));
                    break;
                }
            }
            found_creature.or_else(|| {
                for item in tile.items.iter().rev() {
                    let it = game.items.get_item_type(item.server_id as usize);
                    if !it.look_through {
                        return Some(ThingRef::Item(item.server_id, item.count as u32));
                    }
                }
                tile.ground.as_ref().map(|g| ThingRef::Item(g.server_id, 1))
            })
        };

        let thing = match thing {
            Some(t) => t,
            None => return,
        };

        let is_self = matches!(&thing, ThingRef::Creature(cid, _) if *cid == creature_id);
        let look_distance = if is_self {
            -1i32
        } else {
            let dx = (pos.x as i32 - player_pos.x as i32).unsigned_abs();
            let dy = (pos.y as i32 - player_pos.y as i32).unsigned_abs();
            let mut d = dx.max(dy) as i32;
            if pos.z != player_pos.z {
                d += 15;
            }
            d
        };

        let thing_type = match thing {
            ThingRef::Creature(cid, class_name) => crate::events::LookThingType::Creature(cid, class_name),
            ThingRef::Item(server_id, count) => crate::events::LookThingType::Item(server_id, count),
        };
        drop(game);

        crate::events::g_events().lock().unwrap().event_player_on_look(
            creature_id, thing_type, pos, stackpos, look_distance,
        );
    }

    fn parse_look_in_battle_list(&mut self, msg: &mut NetworkMessage, _conn: &ConnectionHandle) {
        let target_id = msg.get_u32();
        let player_id = self.creature_id;
        if player_id == 0 {
            return;
        }

        let game = g_game().lock().unwrap();
        let player_pos = match game.get_player(player_id) {
            Some(p) => p.base.position,
            None => return,
        };

        let (creature_class, creature_pos) = match game.get_creature(target_id) {
            Some(crate::creatures::Creature::Player(p)) => ("Player", p.base.position),
            Some(crate::creatures::Creature::Monster(m)) => ("Monster", m.base.position),
            Some(crate::creatures::Creature::Npc(n)) => ("Npc", n.base.position),
            None => return,
        };

        let look_distance = if target_id == player_id {
            -1i32
        } else {
            let dx = (player_pos.x as i32 - creature_pos.x as i32).unsigned_abs();
            let dy = (player_pos.y as i32 - creature_pos.y as i32).unsigned_abs();
            let mut d = dx.max(dy) as i32;
            if player_pos.z != creature_pos.z {
                d += 15;
            }
            d
        };
        drop(game);

        crate::events::g_events().lock().unwrap().event_player_on_look_in_battle_list(
            player_id, target_id, creature_class, look_distance,
        );
    }

    fn handle_request_channels(&mut self, conn: &ConnectionHandle) {
        let creature_id = self.creature_id;
        if creature_id == 0 {
            return;
        }

        let game = g_game().lock().unwrap();
        let player = match game.get_player(creature_id) {
            Some(p) => p,
            None => return,
        };
        let guid = player.guid;
        let has_party = player.party_id.is_some();
        let guild_id = player.guild_id;
        drop(game);

        let chat = crate::chat::g_chat().lock().unwrap();
        let channels = chat.get_channel_list_refs(guid, true, guild_id, has_party);

        let mut output = OutputMessage::new();
        output.add_byte(0xAB);
        output.add_byte(channels.len() as u8);
        for ch in &channels {
            output.add_u16(ch.id);
            output.add_string(ch.name.as_bytes());
        }
        drop(chat);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_open_channel(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let channel_id = msg.get_u16();
        let creature_id = self.creature_id;
        if creature_id == 0 {
            return;
        }

        let (guid, guild_id) = {
            let game = g_game().lock().unwrap();
            match game.get_player(creature_id) {
                Some(p) => (p.guid, p.guild_id),
                None => return,
            }
        };

        let (ch_id, ch_name) = {
            let mut chat = crate::chat::g_chat().lock().unwrap();
            // Try mutable access to add user; fall back to immutable for channel info.
            let ch_info = if let Some(ch) = chat.get_channel_mut_by_id(channel_id, guid, guild_id) {
                ch.add_user(creature_id, guid);
                Some((ch.id, ch.name.clone()))
            } else {
                chat.get_channel_ref(channel_id, guid, guild_id)
                    .map(|ch| (ch.id, ch.name.clone()))
            };
            match ch_info {
                Some(info) => info,
                None => return,
            }
        };

        let mut output = OutputMessage::new();
        output.add_byte(0xAC);
        output.add_u16(ch_id);
        output.add_string(ch_name.as_bytes());
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_close_channel(&mut self, msg: &mut NetworkMessage) {
        let channel_id = msg.get_u16();
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }

        let (guid, guild_id) = {
            let game = g_game().lock().unwrap();
            let Some(player) = game.get_player(creature_id) else { return };
            (player.guid, player.guild_id)
        };

        let mut chat = crate::chat::g_chat().lock().unwrap();
        chat.remove_user_from_channel(guid, guid, channel_id, guild_id);
    }

    fn parse_open_private_channel(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let receiver = msg.get_string(None);
        let receiver_name = String::from_utf8_lossy(&receiver).to_string();
        if receiver_name.is_empty() {
            return;
        }

        let mut output = OutputMessage::new();
        output.add_byte(0xAD); // open private channel
        output.add_string(receiver_name.as_bytes());
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_fight_modes(&mut self, msg: &mut NetworkMessage) {
        let raw_fight_mode = msg.get_byte();
        let raw_chase_mode = msg.get_byte();
        let raw_secure_mode = msg.get_byte();

        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(self.creature_id) {
            player.set_fight_mode(match raw_fight_mode {
                1 => crate::creatures::player::FightMode::Attack,
                2 => crate::creatures::player::FightMode::Balanced,
                _ => crate::creatures::player::FightMode::Defense,
            });
            player.set_chase_mode(raw_chase_mode != 0);
            player.set_secure_mode(raw_secure_mode != 0);
        }
    }

    fn parse_attack(&mut self, msg: &mut NetworkMessage) {
        let target_id = msg.get_u32();
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }

        let (had_target, target_exists) = {
            let mut game = g_game().lock().unwrap();
            let had_target = game.get_player(creature_id)
                .map(|p| p.base.attacked_creature_id.is_some())
                .unwrap_or(false);
            let target_exists = target_id != 0 && game.get_creature(target_id).is_some();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.base.attacked_creature_id = if target_exists { Some(target_id) } else { None };
            }
            (had_target, target_exists)
        };

        if target_id == 0 && had_target || (target_id != 0 && !target_exists) {
            send_packet_to_player(creature_id, |output: &mut OutputMessage| {
                output.add_byte(0xA3);
                output.add_u32(0);
            });
        }
    }

    fn parse_follow(&mut self, msg: &mut NetworkMessage) {
        let target_id = msg.get_u32();
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(self.creature_id) {
            if target_id == 0 {
                player.base.follow_creature_id = None;
            } else {
                player.base.follow_creature_id = Some(target_id);
            }
        }
    }

    fn handle_cancel_attack_and_follow(&mut self) {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(self.creature_id) {
            player.base.attacked_creature_id = None;
            player.base.follow_creature_id = None;
        }
    }

    fn parse_invite_to_party(&mut self, msg: &mut NetworkMessage) {
        let target_id = msg.get_u32();
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        g_game().lock().unwrap().player_invite_to_party(creature_id, target_id);
    }

    fn parse_join_party(&mut self, msg: &mut NetworkMessage) {
        let leader_id = msg.get_u32();
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        g_game().lock().unwrap().player_join_party(creature_id, leader_id);
    }

    fn parse_revoke_party_invite(&mut self, msg: &mut NetworkMessage) {
        let target_id = msg.get_u32();
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        g_game().lock().unwrap().player_revoke_party_invitation(creature_id, target_id);
    }

    fn parse_pass_party_leadership(&mut self, msg: &mut NetworkMessage) {
        let new_leader_id = msg.get_u32();
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        g_game().lock().unwrap().player_pass_party_leadership(creature_id, new_leader_id);
    }

    fn handle_leave_party(&mut self) {
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        g_game().lock().unwrap().player_leave_party(creature_id);
    }

    fn handle_close_shop(&mut self) {
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(creature_id) {
                player.shop_owner_id = None;
            }
        }
        send_packet_to_player(creature_id, |output: &mut OutputMessage| {
            output.add_byte(0x7C);
        });
    }

    fn handle_accept_trade(&mut self) {
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        use crate::creatures::player::TradeState;
        let partner_id = {
            let mut game = g_game().lock().unwrap();
            let Some(player) = game.get_player_mut(creature_id) else { return };
            if player.trade_state != TradeState::Acknowledge
                && player.trade_state != TradeState::Initiated
            {
                return;
            }
            player.trade_state = TradeState::Accept;
            player.trade_partner_id
        };
        let Some(partner_id) = partner_id else { return };

        let partner_accepted = {
            let game = g_game().lock().unwrap();
            game.get_player(partner_id)
                .map(|p| p.trade_state == TradeState::Accept)
                .unwrap_or(false)
        };
        if !partner_accepted {
            return; // wait for the other side to accept
        }

        // Both accepted — perform the swap (C++ Game::playerAcceptTrade transfer).
        let mut game = g_game().lock().unwrap();

        // Mark both in TRANSFER so close-trade re-entrancy is blocked.
        if let Some(p) = game.get_player_mut(creature_id) { p.trade_state = TradeState::Transfer; }
        if let Some(p) = game.get_player_mut(partner_id) { p.trade_state = TradeState::Transfer; }

        let my_loc = game.get_player(creature_id).and_then(|p| p.trade_item_loc.clone());
        let partner_loc = game.get_player(partner_id).and_then(|p| p.trade_item_loc.clone());

        // Extract both items from their current locations.
        let my_item = my_loc.and_then(|loc| extract_trade_item(&mut game, creature_id, &loc));
        let partner_item = partner_loc.and_then(|loc| extract_trade_item(&mut game, partner_id, &loc));

        let mut success = true;
        // Give my item to the partner; give partner's item to me.
        if let Some(it) = my_item {
            if !add_item_to_player(&mut game, partner_id, it) { success = false; }
        } else {
            success = false;
        }
        if let Some(it) = partner_item {
            if !add_item_to_player(&mut game, creature_id, it) { success = false; }
        } else {
            success = false;
        }

        // Reset both trade states.
        for id in [creature_id, partner_id] {
            if let Some(p) = game.get_player_mut(id) {
                p.trade_state = TradeState::None;
                p.trade_item = None;
                p.trade_item_id = None;
                p.trade_item_loc = None;
                p.trade_partner_id = None;
            }
        }
        drop(game);

        // Refresh inventory views and close the trade windows on both clients.
        send_full_inventory(creature_id);
        send_full_inventory(partner_id);
        send_trade_close(creature_id);
        send_trade_close(partner_id);
        if !success {
            send_status_message_to_player(creature_id, "Trade could not be completed.");
            send_status_message_to_player(partner_id, "Trade could not be completed.");
        }
    }

    fn handle_close_trade(&mut self) {
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        self.internal_close_trade(creature_id, true);
    }

    fn internal_close_trade(&self, creature_id: u32, send_cancel: bool) {
        use crate::creatures::player::TradeState;
        let partner_id = {
            let mut game = g_game().lock().unwrap();
            let Some(player) = game.get_player_mut(creature_id) else { return };
            if player.trade_state == TradeState::Transfer || player.trade_state == TradeState::None {
                return;
            }
            let partner = player.trade_partner_id;
            player.trade_state = TradeState::None;
            player.trade_item_id = None;
            player.trade_item = None;
            player.trade_item_loc = None;
            player.trade_partner_id = None;
            partner
        };

        // Notify player.
        if send_cancel {
            send_packet_to_player(creature_id, |output: &mut OutputMessage| {
                let msg = b"Trade cancelled.";
                output.add_byte(0xB4);
                output.add_byte(translate_message_class_to_client(0x04));
                output.add_u16(msg.len() as u16);
                output.add_bytes(msg);
            });
        }
        send_packet_to_player(creature_id, |output: &mut OutputMessage| {
            output.add_byte(0x7F);
        });

        // Notify partner.
        if let Some(partner_id) = partner_id {
            {
                let mut game = g_game().lock().unwrap();
                if let Some(partner) = game.get_player_mut(partner_id) {
                    if partner.trade_state != TradeState::Transfer {
                        partner.trade_state = TradeState::None;
                        partner.trade_item_id = None;
                        partner.trade_item = None;
                        partner.trade_item_loc = None;
                        partner.trade_partner_id = None;
                    }
                }
            }
            if send_cancel {
                send_packet_to_player(partner_id, |output: &mut OutputMessage| {
                    let msg = b"Trade cancelled.";
                    output.add_byte(0xB4);
                    output.add_byte(translate_message_class_to_client(0x04));
                    output.add_u16(msg.len() as u16);
                    output.add_bytes(msg);
                });
            }
            send_packet_to_player(partner_id, |output: &mut OutputMessage| {
                output.add_byte(0x7F);
            });
        }
    }

    fn handle_close_npc_channel(&mut self) {
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        let mut game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let pos = player.base.position;
        let spectator_ids = game.get_spectators(pos, false, false);
        drop(game);
        let _ = (pos, spectator_ids);
    }

    fn handle_create_private_channel(&mut self, conn: &ConnectionHandle) {
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }

        let (is_premium, player_guid, player_name) = {
            let game = g_game().lock().unwrap();
            match game.get_player(creature_id) {
                Some(p) => (p.is_premium(), p.guid, p.name.clone()),
                None => return,
            }
        };
        if !is_premium { return; }

        let channel_id = {
            let mut chat = crate::chat::g_chat().lock().unwrap();
            let ch = chat.create_channel(player_guid, creature_id, crate::chat::CHANNEL_PRIVATE, None, None, None, is_premium);
            ch.map(|c| c.id).unwrap_or(0)
        };
        if channel_id == 0 { return; }

        let mut output = OutputMessage::new();
        output.add_byte(0xB2);
        output.add_u16(channel_id);
        let name_bytes = player_name.as_bytes();
        output.add_u16(name_bytes.len() as u16);
        output.add_bytes(name_bytes);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_enable_shared_party_exp(&mut self, msg: &mut NetworkMessage) {
        let shared = msg.get_byte() == 1;
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }
        g_game().lock().unwrap().player_enable_shared_party_experience(creature_id, shared);
    }

    fn parse_channel_invite(&mut self, msg: &mut NetworkMessage) {
        let name = msg.get_string(None);
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }

        let name_str = String::from_utf8_lossy(&name).to_string();
        if name_str.is_empty() { return; }

        let (owner_guid, owner_name, owner_sex, invite_guid, invite_id) = {
            let game = g_game().lock().unwrap();
            let Some(player) = game.get_player(creature_id) else { return };
            let owner_guid = player.guid;
            let owner_name = player.name.clone();
            let owner_sex = player.sex;
            let Some(target) = game.get_player_by_name(&name_str) else { return };
            if target.base.id == creature_id { return; }
            (owner_guid, owner_name, owner_sex, target.guid, target.base.id)
        };

        let mut chat = crate::chat::g_chat().lock().unwrap();
        let Some(pc) = chat.get_private_channel_mut(owner_guid) else { return };
        if pc.is_invited(invite_guid) { return; }
        pc.invite_player(owner_guid, invite_guid, invite_id);
        drop(chat);

        let pronoun = if owner_sex == crate::creatures::player::PlayerSex::Female { "her" } else { "his" };
        let invite_msg = format!("{} invites you to {} private chat channel.", owner_name, pronoun);
        let invite_bytes = invite_msg.into_bytes();
        send_packet_to_player(invite_id, move |output: &mut OutputMessage| {
            output.add_byte(0xB4);
            output.add_byte(25); // MESSAGE_INFO_DESCR
            output.add_string(&invite_bytes);
        });

        let confirm_msg = format!("{} has been invited.", name_str);
        let confirm_bytes = confirm_msg.into_bytes();
        send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
            output.add_byte(0xB4);
            output.add_byte(25); // MESSAGE_INFO_DESCR
            output.add_string(&confirm_bytes);
        });
    }

    fn parse_channel_exclude(&mut self, msg: &mut NetworkMessage) {
        let name = msg.get_string(None);
        let creature_id = self.creature_id;
        if creature_id == 0 { return; }

        let name_str = String::from_utf8_lossy(&name).to_string();
        if name_str.is_empty() { return; }

        let (owner_guid, exclude_guid, exclude_id) = {
            let game = g_game().lock().unwrap();
            let Some(player) = game.get_player(creature_id) else { return };
            let owner_guid = player.guid;
            let Some(target) = game.get_player_by_name(&name_str) else { return };
            if target.base.id == creature_id { return; }
            (owner_guid, target.guid, target.base.id)
        };

        let channel_id = {
            let mut chat = crate::chat::g_chat().lock().unwrap();
            let Some(pc) = chat.get_private_channel_mut(owner_guid) else { return };
            let ch_id = pc.base.id;
            pc.exclude_player(owner_guid, exclude_guid);
            ch_id
        };

        let confirm_msg = format!("{} has been excluded.", name_str);
        let confirm_bytes = confirm_msg.into_bytes();
        send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
            output.add_byte(0xB4);
            output.add_byte(25); // MESSAGE_INFO_DESCR
            output.add_string(&confirm_bytes);
        });

        send_packet_to_player(exclude_id, move |output: &mut OutputMessage| {
            output.add_byte(0xB3); // close private channel
            output.add_u16(channel_id);
        });
    }

    fn parse_update_container(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let cid = msg.get_byte();
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(self.creature_id) else { return };
        let Some(oc) = player.get_container_by_id(cid) else { return };

        use crate::creatures::player::ContainerParent;
        if let ContainerParent::Tile(pos, idx) = &oc.parent {
            let pos = *pos;
            let idx = *idx;
            let Some(tile) = game.map.get_tile(pos) else { return };
            let Some(container_item) = tile.items.get(idx) else { return };
            let server_id = container_item.server_id;
            let item_type = game.items.get_item_type(usize::from(server_id));
            let name = if container_item.name.is_empty() {
                item_type.name.clone()
            } else {
                container_item.name.clone()
            };
            let capacity = item_type.max_items.min(255) as u8;
            let container_item_clone = container_item.clone();
            let children_clone: Vec<crate::map::tile::MapItem> = container_item.children.clone();
            let items_ref = game.items.clone();
            drop(game);

            let mut output = OutputMessage::new();
            write_container(&mut output, cid, &container_item_clone, &items_ref, &name, capacity, false, &children_clone);
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
        }
    }

    fn handle_request_outfit(&mut self, conn: &ConnectionHandle) {
        let game = g_game().lock().unwrap();
        let player = match game.get_player(self.creature_id) {
            Some(p) => p,
            None => return,
        };

        let sex = player.sex;
        let current_outfit = player.base.current_outfit;
        let player_outfits = player.outfits.clone();
        // Mirror C++ getOutfitAddons gating: access players see all (addons 3)
        // and get a prepended "Gamemaster" entry; others are premium-gated.
        let is_access = player.is_access_player();
        let is_premium = player.is_premium();
        drop(game);

        let outfits = crate::world::outfit::g_outfits();
        let outfit_sex = match sex {
            crate::creatures::player::PlayerSex::Female => crate::world::outfit::PlayerSex::Female,
            crate::creatures::player::PlayerSex::Male => crate::world::outfit::PlayerSex::Male,
        };
        let available = outfits.get_outfits(outfit_sex);

        let mut output = OutputMessage::new();
        output.add_byte(0xC8);
        write_outfit(&mut output, &current_outfit);

        let mut outfit_list: Vec<(u16, String, u8)> = Vec::new();
        if is_access {
            outfit_list.push((75, "Gamemaster".to_owned(), 0));
        }
        for outfit in available {
            // getOutfitAddons: access => 3; else premium-gate then look up the
            // player's unlocked addons (default outfits are always available).
            let addons = if is_access {
                3
            } else {
                if outfit.premium && !is_premium {
                    continue;
                }
                let unlocked_addons = player_outfits.iter()
                    .find(|o| o.look_type == outfit.look_type)
                    .map(|o| o.addons);
                match unlocked_addons {
                    Some(a) => a,
                    None if outfit.unlocked => 0,
                    None => continue,
                }
            };
            outfit_list.push((outfit.look_type, outfit.name.clone(), addons));
            if outfit_list.len() == 50 { break; } // client cap
        }

        output.add_byte(outfit_list.len().min(255) as u8);
        for (look_type, name, addons) in outfit_list.iter().take(255) {
            output.add_u16(*look_type);
            output.add_string(name.as_bytes());
            output.add_byte(*addons);
        }

        if client_version().is_1098() {
            output.add_byte(0); // mount count (mounts not loaded yet)
        }

        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_set_outfit(&mut self, msg: &mut NetworkMessage, _conn: &ConnectionHandle) {
        let look_type = msg.get_u16();
        let look_head = msg.get_byte();
        let look_body = msg.get_byte();
        let look_legs = msg.get_byte();
        let look_feet = msg.get_byte();
        let look_addons = msg.get_byte();

        let creature_id = self.creature_id;
        let outfit = Outfit {
            look_type,
            look_type_ex: 0,
            look_head,
            look_body,
            look_legs,
            look_feet,
            look_addons,
            look_mount: if crate::net::protocol_version::client_version().is_1098() { msg.get_u16() } else { 0 },
        };

        let pos = {
            let mut game = g_game().lock().unwrap();
            let player = match game.get_player_mut(creature_id) {
                Some(p) => p,
                None => return,
            };
            player.base.current_outfit = outfit;
            player.base.position
        };

        let mut game = g_game().lock().unwrap();
        let spectator_ids = game.map.get_spectators(pos, true, true, 0, 0, 0, 0);
        drop(game);

        let sessions = player_sessions().lock().unwrap();
        for spec_id in spectator_ids {
            let Some(session) = sessions.get(&spec_id) else { continue };
            let mut output = OutputMessage::new();
            write_creature_outfit(&mut output, creature_id, &outfit);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }

    fn parse_add_vip(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let name_bytes = msg.get_string(None);
        let name = String::from_utf8_lossy(&name_bytes).into_owned();
        if name.is_empty() || name.len() > 25 {
            return;
        }

        let creature_id = self.creature_id;
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        if player.vip_list.len() >= 200 {
            drop(game);
            send_status_message(&mut self.crypto, conn, b"You cannot add more buddies.");
            return;
        }

        if let Some(target) = game.get_player_by_name(&name) {
            let guid = target.guid;
            let target_name = target.name.clone();
            if player.vip_list.contains(&guid) {
                drop(game);
                return;
            }
            drop(game);

            let account_id = {
                let mut game = g_game().lock().unwrap();
                let acct = game.get_player(creature_id).map(|p| p.account_number).unwrap_or(0);
                if let Some(p) = game.get_player_mut(creature_id) {
                    p.vip_list.insert(guid);
                }
                acct
            };

            if account_id != 0 {
                tokio::spawn(crate::db::login::add_vip_entry(account_id, guid));
            }

            let mut output = OutputMessage::new();
            write_vip(&mut output, guid, &target_name, true);
            self.crypto.finalize_output(&mut output);
            conn.send_bytes(output.get_output_buffer().to_vec());
        } else {
            let account_id = game.get_player(creature_id).map(|p| p.account_number).unwrap_or(0);
            drop(game);
            let cid = creature_id;
            let name_owned = name;
            tokio::spawn(async move {
                use crate::db::DatabaseEngine;
                let db = crate::db::g_database();
                let escaped = db.escape_string(&name_owned);
                let query = format!(
                    "SELECT `id`, `name` FROM `players` WHERE `name` = {escaped}"
                );
                if let Ok(Some(result)) = db.store_query(&query).await {
                    let guid = result.get_u64("id").unwrap_or(0) as u32;
                    let db_name = result.get_string("name").unwrap_or_default();
                    if guid != 0 {
                        {
                            let mut game = g_game().lock().unwrap();
                            if let Some(p) = game.get_player_mut(cid) {
                                if p.vip_list.contains(&guid) { return; }
                                p.vip_list.insert(guid);
                            }
                        } // MutexGuard dropped before await

                        if account_id != 0 {
                            crate::db::login::add_vip_entry(account_id, guid).await;
                        }

                        let sessions = player_sessions().lock().unwrap();
                        if let Some(session) = sessions.get(&cid) {
                            let mut output = OutputMessage::new();
                            write_vip(&mut output, guid, &db_name, false);
                            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
                        }
                    }
                }
            });
        }
    }

    fn parse_remove_vip(&mut self, msg: &mut NetworkMessage) {
        let guid = msg.get_u32();
        let account_id = {
            let mut game = g_game().lock().unwrap();
            let acct = game.get_player(self.creature_id).map(|p| p.account_number).unwrap_or(0);
            if let Some(player) = game.get_player_mut(self.creature_id) {
                player.vip_list.remove(&guid);
            }
            acct
        };
        if account_id != 0 {
            tokio::spawn(crate::db::login::remove_vip_entry(account_id, guid));
        }
    }

    fn parse_bug_report(&mut self, msg: &mut NetworkMessage) {
        let _message = msg.get_string(None);
    }

    fn parse_debug_assert(&mut self, msg: &mut NetworkMessage) {
        let _assert_line = msg.get_string(None);
        let _date = msg.get_string(None);
        let _description = msg.get_string(None);
        let _comment = msg.get_string(None);
    }

    fn handle_quest_log(&mut self, conn: &ConnectionHandle) {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(self.creature_id) else { return };
        let storage = player.storage_map.clone();
        drop(game);

        let quests = crate::world::quests::g_quests();
        let started: Vec<_> = quests.get_quests().iter()
            .filter(|q| q.is_started(&storage))
            .collect();

        let mut output = OutputMessage::new();
        output.add_byte(0xF0);
        output.add_u16(started.len().min(0xFFFF) as u16);
        for quest in started.iter().take(0xFFFF) {
            output.add_u16(quest.id);
            output.add_string(quest.name.as_bytes());
            output.add_byte(if quest.is_completed(&storage) { 1 } else { 0 });
        }
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_quest_line(&mut self, msg: &mut NetworkMessage, conn: &ConnectionHandle) {
        let quest_id = msg.get_u16();

        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(self.creature_id) else { return };
        let storage = player.storage_map.clone();
        drop(game);

        let quests = crate::world::quests::g_quests();
        let Some(quest) = quests.get_quest_by_id(quest_id) else { return };
        if !quest.is_started(&storage) { return; }

        let started_missions: Vec<_> = quest.missions.iter()
            .filter(|m| m.is_started(&storage))
            .collect();

        let mut output = OutputMessage::new();
        output.add_byte(0xF1);
        output.add_u16(quest_id);
        output.add_byte(started_missions.len().min(255) as u8);
        for mission in started_missions.iter().take(255) {
            output.add_string(mission.name.as_bytes());
            output.add_string(mission.get_description(&storage).as_bytes());
        }
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn parse_rule_violation_report(&mut self, msg: &mut NetworkMessage) {
        let report_type = msg.get_byte();
        let _report_reason = msg.get_byte();
        let _target_name = msg.get_string(None);
        let _comment = msg.get_string(None);
        if report_type == 0 {
            let _translation = msg.get_string(None);
        } else if report_type == 1 {
            let _translation = msg.get_string(None);
            let _statement_id = msg.get_u32();
        }
    }

    fn parse_extended_opcode(&mut self, msg: &mut NetworkMessage) {
        let _opcode = msg.get_byte();
        let _buffer = msg.get_string(None);
    }

    fn send_cancel_walk(&mut self, conn: &ConnectionHandle, _dir: Direction, message: &[u8]) {
        let player_dir = {
            let game = g_game().lock().unwrap();
            game.get_player(self.creature_id)
                .map(|p| p.base.direction)
                .unwrap_or(Direction::South)
        };
        let mut output = OutputMessage::new();
        output.add_byte(0xB4);
        output.add_byte(translate_message_class_to_client(26));
        output.add_string(message);
        output.add_byte(0xB5);
        output.add_byte(player_dir as u8);
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
    }

    fn disconnect_client(&mut self, conn: &ConnectionHandle, message: &str) {
        let mut output = OutputMessage::new();
        output.add_byte(0x14);
        output.add_string(message.as_bytes());
        self.crypto.finalize_output(&mut output);
        conn.send_bytes(output.get_output_buffer().to_vec());
        conn.disconnect();
    }
}

// ── Dispatcher-side free functions (Phase 2) ────────────────────────────────

fn game_handle_ping_back(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    let mut game = g_game().lock().unwrap();
    if let Some(player) = game.get_player_mut(creature_id) {
        player.last_pong = crate::util::get_milliseconds_time();
    }
}

fn game_handle_logout(creature_id: CreatureId) {
    if creature_id != 0 {
        perform_player_logout(creature_id);
    }
    let sessions = player_sessions().lock().unwrap();
    if let Some(session) = sessions.get(&creature_id) {
        session.conn.disconnect();
    }
}

fn game_handle_walk(creature_id: CreatureId, dir: Direction) {
    if creature_id == 0 { return; }

    let old_pos;
    let new_pos;
    let stackpos: u8;
    let block_msg: Option<&'static [u8]>;
    {
        let game = g_game().lock().unwrap();
        let player = match game.get_player(creature_id) {
            Some(p) => p,
            None => return,
        };
        old_pos = player.base.position;
        new_pos = match game.resolve_walk_destination(old_pos, dir, true) {
            Some(pos) => pos,
            None => {
                send_cancel_walk_to_player(creature_id);
                send_status_message_to_player(creature_id, "Sorry, not possible.");
                return;
            }
        };
        let naive = old_pos.offset_direction(dir);
        let t_ms = crate::util::get_milliseconds_time();
        if new_pos != naive {
            tracing::info!(t_ms, ?old_pos, ?dir, ?naive, resolved = ?new_pos, "DBG walk ADJUSTED (height/floor logic)");
        } else {
            tracing::info!(t_ms, ?old_pos, ?dir, ?new_pos, "DBG walk");
        }

        let idx = game.map.get_tile(old_pos)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1);
        stackpos = if idx >= 0 { idx as u8 } else { 0 };

        if let Some(t) = game.map.get_tile(old_pos) {
            let ncre = t.get_creature_count();
            let creatures: Vec<u32> = t.get_creatures().to_vec();
            tracing::info!(?old_pos, sent_stackpos = stackpos, has_ground = t.ground.is_some(), top = t.get_top_item_count(), down = t.get_down_item_count(), ncre, ?creatures, "DBG walk SRC stackpos");
        }

        block_msg = match game.map.get_tile(new_pos) {
            None => Some(b"Sorry, not possible." as &[u8]),
            Some(t) if t.ground.is_none() => Some(b"Sorry, not possible."),
            Some(t) if t.has_flag(TILESTATE_BLOCKSOLID) => Some(b"There is not enough room."),
            _ => None,
        };
        if block_msg.is_some() {
            if let Some(t) = game.map.get_tile(new_pos) {
                let item_ids: Vec<u16> = t.ground.iter().chain(t.items.iter()).map(|i| i.server_id).collect();
                tracing::info!(?old_pos, ?new_pos, ?dir, flags = format!("{:#x}", t.flags), has_ground = t.ground.is_some(), ?item_ids, "DBG walk BLOCKED");
            } else {
                tracing::info!(?old_pos, ?new_pos, ?dir, "DBG walk BLOCKED: no tile");
            }
        }
    }

    if let Some(msg) = block_msg {
        send_cancel_walk_to_player(creature_id);
        send_status_message_to_player(creature_id, std::str::from_utf8(msg).unwrap_or(""));
        return;
    }

    {
        let mut game = g_game().lock().unwrap();
        game.move_creature_position(creature_id, old_pos, new_pos);
        if let Some(player) = game.get_player_mut(creature_id) {
            player.base.direction = dir;
        }
    }

    crate::events::dispatch::execute_step_event(creature_id, old_pos, new_pos, 1);
    crate::events::dispatch::execute_step_event(creature_id, new_pos, old_pos, 0);

    {
        let game = g_game().lock().unwrap();
        let sessions = player_sessions().lock().unwrap();
        if let Some(session) = sessions.get(&creature_id) {
            let mut known = session.known_creatures.lock().unwrap();
            if old_pos.z != new_pos.z {
                let mut output = OutputMessage::new();
                write_remove_tile_creature(&mut output, old_pos, stackpos, creature_id);
                finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);

                let mut output = OutputMessage::new();
                if let Some(player) = game.get_player(creature_id) {
                    write_map_description(
                        &mut output,
                        &game,
                        game.get_items(),
                        new_pos,
                        &mut known,
                        creature_id,
                        player,
                    );
                }
                finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
            } else {
                let mut output = OutputMessage::new();
                output.add_byte(0x6D);
                write_creature_movement(&mut output, old_pos, new_pos, stackpos, creature_id);
                append_walk_map_slices(&mut output, &game, game.get_items(), &mut known, old_pos, new_pos);
                finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
            }
        }
    }

    broadcast_creature_move(creature_id, old_pos, new_pos, stackpos);
}

fn game_handle_turn(creature_id: CreatureId, dir: Direction) {
    if creature_id == 0 { return; }

    let (pos, stackpos) = {
        let mut game = g_game().lock().unwrap();
        let player = match game.get_player_mut(creature_id) {
            Some(p) => p,
            None => return,
        };
        if player.base.direction == dir { return; }
        player.base.direction = dir;
        let pos = player.base.position;
        let sp = game.map.get_tile(pos)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1);
        (pos, sp)
    };

    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        write_creature_turn(output, pos, if stackpos >= 0 { stackpos as u8 } else { 0 }, creature_id, dir);
    });

    broadcast_creature_turn(creature_id, pos, stackpos, dir);
}

fn game_parse_auto_walk(creature_id: CreatureId, dirs: Vec<u8>) {
    if dirs.is_empty() { return; }

    let mut directions = Vec::with_capacity(dirs.len());
    for &raw in dirs.iter().rev() {
        let dir = match raw {
            1 => Direction::East,
            2 => Direction::NorthEast,
            3 => Direction::North,
            4 => Direction::NorthWest,
            5 => Direction::West,
            6 => Direction::SouthWest,
            7 => Direction::South,
            8 => Direction::SouthEast,
            _ => {
                send_cancel_walk_to_player(creature_id);
                return;
            }
        };
        directions.push(dir);
    }

    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.idle_time = 0;
            player.base.list_walk_dir = directions;
            if player.base.event_walk != 0 {
                crate::runtime::g_scheduler().stop_event(player.base.event_walk);
                player.base.event_walk = 0;
            }
        }
    }

    schedule_auto_walk_step(creature_id);
}

fn game_parse_fight_modes(creature_id: CreatureId, fight_mode: u8, chase_mode: u8, safe_fight: u8) {
    let mut game = g_game().lock().unwrap();
    if let Some(player) = game.get_player_mut(creature_id) {
        player.set_fight_mode(match fight_mode {
            1 => crate::creatures::player::FightMode::Attack,
            2 => crate::creatures::player::FightMode::Balanced,
            _ => crate::creatures::player::FightMode::Defense,
        });
        player.set_chase_mode(chase_mode != 0);
        player.set_secure_mode(safe_fight != 0);
    }
}

fn game_parse_attack(creature_id: CreatureId, target_id: u32) {
    if creature_id == 0 { return; }

    let (had_target, target_exists) = {
        let mut game = g_game().lock().unwrap();
        let had_target = game.get_player(creature_id)
            .map(|p| p.base.attacked_creature_id.is_some())
            .unwrap_or(false);
        let target_exists = target_id != 0 && game.get_creature(target_id).is_some();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.base.attacked_creature_id = if target_exists { Some(target_id) } else { None };
        }
        (had_target, target_exists)
    };

    if target_id == 0 && had_target || (target_id != 0 && !target_exists) {
        send_packet_to_player(creature_id, |output: &mut OutputMessage| {
            output.add_byte(0xA3);
            output.add_u32(0);
        });
    }
}

fn game_parse_follow(creature_id: CreatureId, target_id: u32) {
    let mut game = g_game().lock().unwrap();
    if let Some(player) = game.get_player_mut(creature_id) {
        player.base.follow_creature_id = if target_id == 0 { None } else { Some(target_id) };
    }
}

fn game_handle_cancel_attack_and_follow(creature_id: CreatureId) {
    let mut game = g_game().lock().unwrap();
    if let Some(player) = game.get_player_mut(creature_id) {
        player.base.attacked_creature_id = None;
        player.base.follow_creature_id = None;
    }
}

fn game_parse_invite_to_party(creature_id: CreatureId, target_id: u32) {
    if creature_id == 0 { return; }
    g_game().lock().unwrap().player_invite_to_party(creature_id, target_id);
}

fn game_parse_join_party(creature_id: CreatureId, leader_id: u32) {
    if creature_id == 0 { return; }
    g_game().lock().unwrap().player_join_party(creature_id, leader_id);
}

fn game_parse_revoke_party_invite(creature_id: CreatureId, target_id: u32) {
    if creature_id == 0 { return; }
    g_game().lock().unwrap().player_revoke_party_invitation(creature_id, target_id);
}

fn game_parse_pass_party_leadership(creature_id: CreatureId, new_leader_id: u32) {
    if creature_id == 0 { return; }
    g_game().lock().unwrap().player_pass_party_leadership(creature_id, new_leader_id);
}

fn game_handle_leave_party(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    g_game().lock().unwrap().player_leave_party(creature_id);
}

fn game_parse_enable_shared_party_exp(creature_id: CreatureId, active: bool) {
    if creature_id == 0 { return; }
    g_game().lock().unwrap().player_enable_shared_party_experience(creature_id, active);
}

fn game_handle_close_npc_channel(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    let mut game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    let pos = player.base.position;
    let spectator_ids = game.get_spectators(pos, false, false);
    drop(game);
    let _ = (pos, spectator_ids);
}

fn game_handle_close_shop(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.shop_owner_id = None;
        }
    }
    send_packet_to_player(creature_id, |output: &mut OutputMessage| {
        output.add_byte(0x7C);
    });
}

fn game_handle_close_trade(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    use crate::creatures::player::TradeState;
    let (partner_id, trade_item, trade_item_loc) = {
        let mut game = g_game().lock().unwrap();
        let Some(player) = game.get_player_mut(creature_id) else { return };
        let partner_id = player.trade_partner_id;
        let item = player.trade_item.take();
        let loc = player.trade_item_loc.take();
        player.trade_state = TradeState::None;
        player.trade_partner_id = None;
        (partner_id, item, loc)
    };
    send_packet_to_player(creature_id, |output: &mut OutputMessage| {
        output.add_byte(0x7F);
    });
    if let Some(partner_id) = partner_id {
        let mut game = g_game().lock().unwrap();
        if let Some(partner) = game.get_player_mut(partner_id) {
            partner.trade_state = TradeState::None;
            partner.trade_partner_id = None;
            partner.trade_item = None;
            partner.trade_item_loc = None;
        }
        drop(game);
        send_packet_to_player(partner_id, |output: &mut OutputMessage| {
            output.add_byte(0x7F);
        });
    }
    let _ = (trade_item, trade_item_loc);
}

fn game_parse_remove_vip(creature_id: CreatureId, guid: u32) {
    if creature_id == 0 { return; }
    let mut game = g_game().lock().unwrap();
    if let Some(player) = game.get_player_mut(creature_id) {
        player.vip_list.retain(|&v| v != guid);
    }
}

fn game_parse_look_at(creature_id: CreatureId, pos: Position, _sprite_id: u16, stackpos: u8) {
    if creature_id == 0 { return; }

    let game = g_game().lock().unwrap();
    let player_pos = match game.get_player(creature_id) {
        Some(p) => p.base.position,
        None => return,
    };
    let tile = match game.map.get_tile(pos) {
        Some(t) => t,
        None => return,
    };

    let thing = {
        let mut found_creature: Option<(u32, &'static str)> = None;
        for &cid in &tile.creature_ids {
            if let Some(c) = game.get_creature(cid) {
                let class_name = match c {
                    crate::creatures::Creature::Player(_) => "Player",
                    crate::creatures::Creature::Monster(_) => "Monster",
                    crate::creatures::Creature::Npc(_) => "Npc",
                };
                found_creature = Some((cid, class_name));
                break;
            }
        }
        if let Some((cid, cn)) = found_creature {
            Some(crate::events::LookThingType::Creature(cid, cn))
        } else {
            let item = tile.items.iter().rev().find(|item| {
                !game.items.get_item_type(item.server_id as usize).look_through
            });
            if let Some(item) = item {
                Some(crate::events::LookThingType::Item(item.server_id, item.count as u32))
            } else {
                tile.ground.as_ref().map(|g| crate::events::LookThingType::Item(g.server_id, 1))
            }
        }
    };
    let Some(thing) = thing else { return };

    let is_self = matches!(&thing, crate::events::LookThingType::Creature(cid, _) if *cid == creature_id);
    let look_distance = if is_self {
        -1i32
    } else {
        let dx = (pos.x as i32 - player_pos.x as i32).unsigned_abs();
        let dy = (pos.y as i32 - player_pos.y as i32).unsigned_abs();
        let mut d = dx.max(dy) as i32;
        if pos.z != player_pos.z { d += 15; }
        d
    };
    drop(game);

    crate::events::g_events().lock().unwrap().event_player_on_look(
        creature_id, thing, pos, stackpos, look_distance,
    );
}

fn game_parse_look_in_battle_list(creature_id: CreatureId, target_id: u32) {
    if creature_id == 0 { return; }

    let game = g_game().lock().unwrap();
    let player_pos = match game.get_player(creature_id) {
        Some(p) => p.base.position,
        None => return,
    };
    let (creature_class, creature_pos) = match game.get_creature(target_id) {
        Some(crate::creatures::Creature::Player(p)) => ("Player", p.base.position),
        Some(crate::creatures::Creature::Monster(m)) => ("Monster", m.base.position),
        Some(crate::creatures::Creature::Npc(n)) => ("Npc", n.base.position),
        None => return,
    };
    let look_distance = if target_id == creature_id {
        -1i32
    } else {
        let dx = (player_pos.x as i32 - creature_pos.x as i32).unsigned_abs();
        let dy = (player_pos.y as i32 - creature_pos.y as i32).unsigned_abs();
        let mut d = dx.max(dy) as i32;
        if player_pos.z != creature_pos.z { d += 15; }
        d
    };
    drop(game);

    crate::events::g_events().lock().unwrap().event_player_on_look_in_battle_list(
        creature_id, target_id, creature_class, look_distance,
    );
}

fn game_parse_look_in_trade(creature_id: CreatureId, counter_offer: bool, index: u8) {
    if creature_id == 0 { return; }
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    let trade_partner_id = player.trade_partner_id;
    let source_id = if counter_offer { trade_partner_id.unwrap_or(0) } else { creature_id };
    let item = game.get_player(source_id).and_then(|p| p.trade_item.as_ref());
    let Some(item) = item else { return };
    let name = game.items.get_item_type(item.server_id as usize).name.clone();
    drop(game);
    if !name.is_empty() {
        send_status_message_to_player(creature_id, &format!("You see {}.", name));
    }
}

fn game_parse_close_channel(creature_id: CreatureId, channel_id: u16) {
    if creature_id == 0 { return; }
    let (guid, guild_id) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        (player.guid, player.guild_id)
    };
    let mut chat = crate::chat::g_chat().lock().unwrap();
    chat.remove_user_from_channel(guid, guid, channel_id, guild_id);
}

fn game_parse_rotate_item(creature_id: CreatureId, pos: Position, _sprite_id: u16, stackpos: u8) {
    if pos.x == 0xFFFF { return; }

    let mut game = g_game().lock().unwrap();
    let (old_id, item_idx) = {
        let tile = match game.map.get_tile(pos) {
            Some(t) => t,
            None => return,
        };
        let ground_offset = if tile.ground.is_some() { 1u8 } else { 0 };
        let creature_offset = tile.creature_ids.len() as u8;
        if stackpos < ground_offset + creature_offset { return; }
        let item_idx = (stackpos - ground_offset - creature_offset) as usize;
        if item_idx >= tile.items.len() { return; }
        (tile.items[item_idx].server_id, item_idx)
    };

    let rotate_to = game.items.get_item_type(old_id as usize).rotate_to;
    if rotate_to == 0 { return; }

    if let Some(tile) = game.map.get_tile_mut(pos) {
        tile.items[item_idx].server_id = rotate_to;
    }

    let spectators = game.map.get_spectators(pos, true, true, 0, 0, 0, 0);
    let items_ref = game.items.clone();
    drop(game);

    let sessions = player_sessions().lock().unwrap();
    for &spec_id in &spectators {
        let Some(session) = sessions.get(&spec_id) else { continue };
        let mut output = OutputMessage::new();
        let item = crate::map::tile::MapItem {
            server_id: rotate_to,
            ..Default::default()
        };
        write_update_tile_item(&mut output, pos, stackpos, &item, &items_ref);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

fn game_parse_text_window(creature_id: CreatureId, window_text_id: u32, new_text: Vec<u8>) {
    if creature_id == 0 { return; }

    let mut game = g_game().lock().unwrap();
    let (write_item_id, write_item_pos, write_item_stack_pos, max_write_len, internal_window_text_id) = {
        let Some(player) = game.get_player(creature_id) else { return };
        (player.write_item_id, player.write_item_pos, player.write_item_stack_pos, player.max_write_len, player.window_text_id)
    };

    if new_text.len() > max_write_len as usize || window_text_id != internal_window_text_id { return; }

    let Some(_server_id) = write_item_id else {
        send_status_message_to_player(creature_id, "Sorry, not possible.");
        return;
    };
    let Some(item_pos) = write_item_pos else {
        send_status_message_to_player(creature_id, "Sorry, not possible.");
        return;
    };

    let player_pos = match game.get_player(creature_id) {
        Some(p) => p.base.position,
        None => return,
    };

    if item_pos.x != 0xFFFF {
        let dx = (item_pos.x as i32 - player_pos.x as i32).unsigned_abs();
        let dy = (item_pos.y as i32 - player_pos.y as i32).unsigned_abs();
        let dz = (item_pos.z as i32 - player_pos.z as i32).unsigned_abs();
        if dx > 1 || dy > 1 || dz > 0 {
            send_status_message_to_player(creature_id, "Sorry, not possible.");
            return;
        }
    }

    let text_str = String::from_utf8_lossy(&new_text).to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as u32;
    let player_name = game.get_player(creature_id).map(|p| p.name.clone()).unwrap_or_default();

    if item_pos.x == 0xFFFF {
        let slot = usize::from(item_pos.y);
        let cur_sid = game.get_player(creature_id).and_then(|p| p.inventory_items[slot].as_ref()).map(|it| it.server_id).unwrap_or(0);
        let write_once = game.items.get_item_type(usize::from(cur_sid)).write_once_item_id;
        if let Some(player) = game.get_player_mut(creature_id) {
            if let Some(ref mut item) = player.inventory_items[slot] {
                if !text_str.is_empty() {
                    if item.text != text_str {
                        item.text = text_str;
                        item.written_by = player_name;
                        item.written_date = now;
                    }
                } else {
                    item.text.clear();
                    item.written_by.clear();
                    item.written_date = 0;
                }
                if write_once != 0 { item.server_id = write_once; }
            }
            player.write_item_id = None;
            player.write_item_pos = None;
            player.max_write_len = 0;
        }
    } else {
        let stack = write_item_stack_pos;
        let idx = game.map.get_tile(item_pos).map(|t| if t.ground.is_some() { stack as usize - 1 } else { stack as usize }).unwrap_or(0);
        let cur_sid = game.map.get_tile(item_pos).and_then(|t| t.items.get(idx)).map(|it| it.server_id).unwrap_or(0);
        let write_once = game.items.get_item_type(usize::from(cur_sid)).write_once_item_id;
        if let Some(tile) = game.map.get_tile_mut(item_pos) {
            if let Some(item) = tile.items.get_mut(idx) {
                if !text_str.is_empty() {
                    if item.text != text_str {
                        item.text = text_str;
                        item.written_by = player_name;
                        item.written_date = now;
                    }
                } else {
                    item.text.clear();
                    item.written_by.clear();
                    item.written_date = 0;
                }
                if write_once != 0 { item.server_id = write_once; }
            }
        }
        if let Some(player) = game.get_player_mut(creature_id) {
            player.write_item_id = None;
            player.write_item_pos = None;
            player.max_write_len = 0;
        }
        if write_once != 0 && write_once != cur_sid {
            let stackpos_client = game.map.get_tile(item_pos).map(|tile| tile.item_client_stackpos(idx)).unwrap_or(0);
            let new_client_id = game.items.get_item_type(usize::from(write_once)).client_id;
            let spectators = game.map.get_spectators(item_pos, true, true, 0, 0, 0, 0);
            for spec_id in spectators {
                send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                    output.add_byte(0x6B);
                    output.add_position(item_pos.x, item_pos.y, item_pos.z);
                    output.add_byte(stackpos_client);
                    output.add_u16(new_client_id);
                });
            }
        }
    }
}

fn game_parse_set_outfit(creature_id: CreatureId, look_type: u16, look_head: u8, look_body: u8, look_legs: u8, look_feet: u8, look_addons: u8) {
    let outfit = Outfit {
        look_type,
        look_type_ex: 0,
        look_head,
        look_body,
        look_legs,
        look_feet,
        look_addons,
        look_mount: 0,
    };

    let pos = {
        let mut game = g_game().lock().unwrap();
        let player = match game.get_player_mut(creature_id) {
            Some(p) => p,
            None => return,
        };
        player.base.current_outfit = outfit;
        player.base.position
    };

    let mut game = g_game().lock().unwrap();
    let spectator_ids = game.map.get_spectators(pos, true, true, 0, 0, 0, 0);
    drop(game);

    let sessions = player_sessions().lock().unwrap();
    for spec_id in spectator_ids {
        let Some(session) = sessions.get(&spec_id) else { continue };
        let mut output = OutputMessage::new();
        write_creature_outfit(&mut output, creature_id, &outfit);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

fn game_handle_accept_trade(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    use crate::creatures::player::TradeState;
    let partner_id = {
        let mut game = g_game().lock().unwrap();
        let Some(player) = game.get_player_mut(creature_id) else { return };
        if player.trade_state != TradeState::Acknowledge && player.trade_state != TradeState::Initiated { return; }
        player.trade_state = TradeState::Accept;
        player.trade_partner_id
    };
    let Some(partner_id) = partner_id else { return };

    let partner_accepted = {
        let game = g_game().lock().unwrap();
        game.get_player(partner_id).map(|p| p.trade_state == TradeState::Accept).unwrap_or(false)
    };
    if !partner_accepted { return; }

    let mut game = g_game().lock().unwrap();
    if let Some(p) = game.get_player_mut(creature_id) { p.trade_state = TradeState::Transfer; }
    if let Some(p) = game.get_player_mut(partner_id) { p.trade_state = TradeState::Transfer; }

    let my_loc = game.get_player(creature_id).and_then(|p| p.trade_item_loc.clone());
    let partner_loc = game.get_player(partner_id).and_then(|p| p.trade_item_loc.clone());
    let my_item = my_loc.and_then(|loc| extract_trade_item(&mut game, creature_id, &loc));
    let partner_item = partner_loc.and_then(|loc| extract_trade_item(&mut game, partner_id, &loc));

    let mut success = true;
    if let Some(it) = my_item {
        if !add_item_to_player(&mut game, partner_id, it) { success = false; }
    } else { success = false; }
    if let Some(it) = partner_item {
        if !add_item_to_player(&mut game, creature_id, it) { success = false; }
    } else { success = false; }

    for id in [creature_id, partner_id] {
        if let Some(p) = game.get_player_mut(id) {
            p.trade_state = TradeState::None;
            p.trade_item = None;
            p.trade_item_id = None;
            p.trade_item_loc = None;
            p.trade_partner_id = None;
        }
    }
    drop(game);

    send_full_inventory(creature_id);
    send_full_inventory(partner_id);
    send_trade_close(creature_id);
    send_trade_close(partner_id);
    if !success {
        send_status_message_to_player(creature_id, "Trade could not be completed.");
        send_status_message_to_player(partner_id, "Trade could not be completed.");
    }
}

fn game_parse_request_trade(creature_id: CreatureId, pos: Position, sprite_id: u16, stackpos: u8, target_id: u32) {
    if creature_id == 0 { return; }
    let game = g_game().lock().unwrap();
    let Some(_player) = game.get_player(creature_id) else { return };
    let Some(_partner) = game.get_player(target_id) else { return };
    drop(game);
    // Trade request validation and offer exchange already handled by the old parse_request_trade method.
    // The full logic will be wired when the old method bodies are removed.
}

fn game_handle_say(creature_id: CreatureId, speak_type: u8, channel_id: Option<u16>, receiver_name: Option<Vec<u8>>, text: Vec<u8>) {
    if creature_id == 0 { return; }
    let text_str = String::from_utf8_lossy(&text).to_string();
    if text_str.is_empty() { return; }

    let (pos, name, level) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        (player.base.position, player.name.clone(), player.level)
    };

    if crate::events::dispatch::execute_spell_say(creature_id, &text_str) {
        broadcast_creature_say(creature_id, pos, &name, level as u16, 1, text_str.as_bytes());
        return;
    }

    if crate::events::dispatch::execute_talk_action(creature_id, &text_str, "", speak_type) {
        return;
    }

    match speak_type {
        1 | 2 | 3 => broadcast_creature_say(creature_id, pos, &name, level as u16, speak_type, text_str.as_bytes()),
        6 | 11 => {
            let recv_name = receiver_name.map(|n| String::from_utf8_lossy(&n).to_string()).unwrap_or_default();
            let game = g_game().lock().unwrap();
            if let Some(target) = game.get_player_by_name(&recv_name) {
                let target_id = target.base.id;
                drop(game);
                send_packet_to_player(target_id, {
                    let name = name.clone();
                    let text = text_str.clone();
                    move |output: &mut OutputMessage| {
                        output.add_byte(0xAA);
                        output.add_u32(0);
                        output.add_string(name.as_bytes());
                        output.add_u16(level as u16);
                        output.add_byte(translate_speak_class_to_client(speak_type));
                        output.add_string(text.as_bytes());
                    }
                });
                send_packet_to_player(creature_id, {
                    let name = recv_name;
                    move |output: &mut OutputMessage| {
                        output.add_byte(0xAA);
                        output.add_u32(0);
                        output.add_string(name.as_bytes());
                        output.add_u16(0);
                        output.add_byte(translate_speak_class_to_client(speak_type));
                        output.add_string(text_str.as_bytes());
                    }
                });
            }
        }
        7 | 10 => {
            let cid = channel_id.unwrap_or(0);
            let mut game = g_game().lock().unwrap();
            let spectators = game.map.get_spectators(pos, true, true, 0, 0, 0, 0);
            drop(game);
            for spec_id in spectators {
                send_packet_to_player(spec_id, {
                    let name = name.clone();
                    let text = text_str.clone();
                    move |output: &mut OutputMessage| {
                        output.add_byte(0xAA);
                        output.add_u32(0);
                        output.add_string(name.as_bytes());
                        output.add_u16(level as u16);
                        output.add_byte(translate_speak_class_to_client(speak_type));
                        output.add_u16(cid);
                        output.add_string(text.as_bytes());
                    }
                });
            }
        }
        _ => {}
    }
}

fn game_handle_request_channels(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    let guid = player.guid;
    let has_party = player.party_id.is_some();
    let guild_id = player.guild_id;
    drop(game);

    let chat = crate::chat::g_chat().lock().unwrap();
    let channels = chat.get_channel_list_refs(guid, true, guild_id, has_party);

    let mut output = OutputMessage::new();
    output.add_byte(0xAB);
    output.add_byte(channels.len() as u8);
    for ch in &channels {
        output.add_u16(ch.id);
        output.add_string(ch.name.as_bytes());
    }
    drop(chat);
    send_raw_to_player(creature_id, &mut output);
}

fn game_parse_open_channel(creature_id: CreatureId, channel_id: u16) {
    if creature_id == 0 { return; }
    let (guid, guild_id) = {
        let game = g_game().lock().unwrap();
        match game.get_player(creature_id) {
            Some(p) => (p.guid, p.guild_id),
            None => return,
        }
    };

    let (ch_id, ch_name) = {
        let mut chat = crate::chat::g_chat().lock().unwrap();
        let ch_info = if let Some(ch) = chat.get_channel_mut_by_id(channel_id, guid, guild_id) {
            ch.add_user(creature_id, guid);
            Some((ch.id, ch.name.clone()))
        } else {
            chat.get_channel_ref(channel_id, guid, guild_id)
                .map(|ch| (ch.id, ch.name.clone()))
        };
        match ch_info {
            Some(info) => info,
            None => return,
        }
    };

    let mut output = OutputMessage::new();
    output.add_byte(0xAC);
    output.add_u16(ch_id);
    output.add_string(ch_name.as_bytes());
    send_raw_to_player(creature_id, &mut output);
}

fn game_parse_close_container(creature_id: CreatureId, container_id: u8) {
    if creature_id == 0 { return; }
    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.close_container(container_id);
        }
    }
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0x6F);
        output.add_byte(container_id);
    });
}

fn game_parse_up_arrow_container(creature_id: CreatureId, container_id: u8) {
    if creature_id == 0 { return; }
    let mut game = g_game().lock().unwrap();
    if let Some(player) = game.get_player_mut(creature_id) {
        player.close_container(container_id);
    }
    drop(game);
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0x6F);
        output.add_byte(container_id);
    });
}

fn game_parse_open_private_channel(creature_id: CreatureId, name: Vec<u8>) {
    if creature_id == 0 { return; }
    let receiver_name = String::from_utf8_lossy(&name).to_string();
    if receiver_name.is_empty() { return; }
    let game = g_game().lock().unwrap();
    let target_exists = game.get_player_by_name(&receiver_name).is_some();
    drop(game);
    if !target_exists { return; }
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0xAD);
        output.add_string(receiver_name.as_bytes());
    });
}

fn game_handle_create_private_channel(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    let (is_premium, _guid, player_name) = {
        let game = g_game().lock().unwrap();
        match game.get_player(creature_id) {
            Some(p) => (p.is_premium(), p.guid, p.name.clone()),
            None => return,
        }
    };
    if !is_premium { return; }
    let (guid,) = {
        let game = g_game().lock().unwrap();
        let Some(p) = game.get_player(creature_id) else { return };
        (p.guid,)
    };
    let channel_id = {
        let mut chat = crate::chat::g_chat().lock().unwrap();
        let ch = chat.create_channel(guid, creature_id, crate::chat::CHANNEL_PRIVATE, None, None, None, is_premium);
        ch.map(|c| c.id).unwrap_or(0)
    };
    if channel_id == 0 { return; }
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0xB2);
        output.add_u16(channel_id);
        output.add_string(player_name.as_bytes());
    });
}

fn game_parse_channel_invite(creature_id: CreatureId, name: Vec<u8>) {
    if creature_id == 0 { return; }
    let name_str = String::from_utf8_lossy(&name).to_string();
    if name_str.is_empty() { return; }
    let (owner_guid, owner_name, owner_sex, invite_guid, invite_id) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let owner_guid = player.guid;
        let owner_name = player.name.clone();
        let owner_sex = player.sex;
        let Some(target) = game.get_player_by_name(&name_str) else { return };
        if target.base.id == creature_id { return; }
        (owner_guid, owner_name, owner_sex, target.guid, target.base.id)
    };
    let mut chat = crate::chat::g_chat().lock().unwrap();
    let Some(pc) = chat.get_private_channel_mut(owner_guid) else { return };
    if pc.is_invited(invite_guid) { return; }
    pc.invite_player(owner_guid, invite_guid, invite_id);
    drop(chat);
    let pronoun = if owner_sex == crate::creatures::player::PlayerSex::Female { "her" } else { "his" };
    let invite_msg = format!("{} invites you to {} private chat channel.", owner_name, pronoun);
    send_status_message_to_player(invite_id, &invite_msg);
}

fn game_parse_channel_exclude(creature_id: CreatureId, name: Vec<u8>) {
    if creature_id == 0 { return; }
    let name_str = String::from_utf8_lossy(&name).to_string();
    if name_str.is_empty() { return; }
    let (owner_guid, exclude_guid) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let owner_guid = player.guid;
        let Some(target) = game.get_player_by_name(&name_str) else { return };
        (owner_guid, target.guid)
    };
    let mut chat = crate::chat::g_chat().lock().unwrap();
    let Some(pc) = chat.get_private_channel_mut(owner_guid) else { return };
    pc.exclude_player(owner_guid, exclude_guid);
}

fn game_handle_quest_log(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    let storage = player.storage_map.clone();
    drop(game);

    let quests = crate::world::quests::g_quests();
    let started: Vec<_> = quests.get_quests().iter()
        .filter(|q| q.is_started(&storage))
        .collect();

    let mut output = OutputMessage::new();
    output.add_byte(0xF0);
    output.add_u16(started.len().min(0xFFFF) as u16);
    for quest in started.iter().take(0xFFFF) {
        output.add_u16(quest.id);
        output.add_string(quest.name.as_bytes());
        output.add_byte(if quest.is_completed(&storage) { 1 } else { 0 });
    }
    send_raw_to_player(creature_id, &mut output);
}

fn game_parse_quest_line(creature_id: CreatureId, quest_id: u16) {
    if creature_id == 0 { return; }
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    let storage = player.storage_map.clone();
    drop(game);

    let quests = crate::world::quests::g_quests();
    let Some(quest) = quests.get_quest_by_id(quest_id) else { return };
    if !quest.is_started(&storage) { return; }

    let started_missions: Vec<_> = quest.missions.iter()
        .filter(|m| m.is_started(&storage))
        .collect();

    let mut output = OutputMessage::new();
    output.add_byte(0xF1);
    output.add_u16(quest_id);
    output.add_byte(started_missions.len().min(255) as u8);
    for mission in started_missions.iter().take(255) {
        output.add_string(mission.name.as_bytes());
        output.add_string(mission.get_description(&storage).as_bytes());
    }
    send_raw_to_player(creature_id, &mut output);
}

fn game_handle_request_outfit(creature_id: CreatureId) {
    if creature_id == 0 { return; }
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    let sex = player.sex;
    let current_outfit = player.base.current_outfit;
    let player_outfits = player.outfits.clone();
    let is_access = player.is_access_player();
    let is_premium = player.is_premium();
    drop(game);

    let outfits = crate::world::outfit::g_outfits();
    let outfit_sex = match sex {
        crate::creatures::player::PlayerSex::Female => crate::world::outfit::PlayerSex::Female,
        crate::creatures::player::PlayerSex::Male => crate::world::outfit::PlayerSex::Male,
    };
    let available = outfits.get_outfits(outfit_sex);

    let mut output = OutputMessage::new();
    output.add_byte(0xC8);
    write_outfit(&mut output, &current_outfit);

    let mut outfit_list: Vec<(u16, String, u8)> = Vec::new();
    if is_access {
        outfit_list.push((75, "Gamemaster".to_owned(), 0));
    }
    for outfit in available {
        let addons = if is_access {
            3
        } else {
            if outfit.premium && !is_premium { continue; }
            let unlocked_addons = player_outfits.iter()
                .find(|o| o.look_type == outfit.look_type)
                .map(|o| o.addons);
            match unlocked_addons {
                Some(a) => a,
                None if outfit.unlocked => 0,
                None => continue,
            }
        };
        outfit_list.push((outfit.look_type, outfit.name.clone(), addons));
        if outfit_list.len() == 50 { break; }
    }

    output.add_byte(outfit_list.len().min(255) as u8);
    for (look_type, name, addons) in outfit_list.iter().take(255) {
        output.add_u16(*look_type);
        output.add_string(name.as_bytes());
        output.add_byte(*addons);
    }

    if client_version().is_1098() {
        output.add_byte(0);
    }

    send_raw_to_player(creature_id, &mut output);
}

fn game_parse_add_vip(creature_id: CreatureId, name: Vec<u8>) {
    let name_str = String::from_utf8_lossy(&name).into_owned();
    if name_str.is_empty() || name_str.len() > 25 { return; }
    if creature_id == 0 { return; }

    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(creature_id) else { return };
    if player.vip_list.len() >= 200 {
        send_status_message_to_player(creature_id, "You cannot add more buddies.");
        return;
    }
    let target = game.get_player_by_name(&name_str);
    let (target_guid, target_online) = if let Some(t) = target {
        (t.guid, true)
    } else {
        drop(game);
        return;
    };
    if player.vip_list.contains(&target_guid) { return; }
    drop(game);

    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.vip_list.insert(target_guid);
        }
    }

    let name_owned = name_str;
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0xD2);
        output.add_u32(target_guid);
        output.add_string(name_owned.as_bytes());
        output.add_byte(if target_online { 1 } else { 0 });
    });
}

fn game_parse_update_container(creature_id: CreatureId, container_id: u8) {
    if creature_id == 0 { return; }
    // Re-send the container contents for this container ID.
    // The old method rebuilt the 0x6E packet for tile containers.
    // Stub for now — the full container refresh logic is complex.
}

#[allow(clippy::too_many_arguments)]
fn game_handle_use_item(creature_id: CreatureId, pos: Position, sprite_id: u16, stackpos: u8, _index: u8) {
    let Some((server_id, old_pos, is_pz_locked)) =
        resolve_player_item_for_use(creature_id, pos, stackpos, sprite_id, false)
    else {
        send_status_message_to_player(creature_id, "Sorry, not possible.");
        return;
    };

    let (action_id, unique_id, item_index) = {
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(pos) {
            let item = tile.get_use_item(stackpos);
            match item {
                Some(item) => (item.action_id, item.unique_id, stackpos as i32 - if tile.ground.is_some() { 1 } else { 0 }),
                None => (0u16, 0u16, -1i32),
            }
        } else {
            (0, 0, -1)
        }
    };

    if crate::events::dispatch::execute_action_use(
        creature_id, pos, server_id, item_index,
        action_id, unique_id, false,
    ) {
        return;
    }

    {
        let is_bed = {
            let game = g_game().lock().unwrap();
            game.items.get_item_type(usize::from(server_id)).kind == crate::items::ItemKind::Bed
        };
        if is_bed {
            // Bed use handled by the old method on ProtocolGame — needs session context.
            // TODO: port handle_use_bed to free function
            return;
        }
    }

    {
        let game = g_game().lock().unwrap();
        let item_type = game.items.get_item_type(usize::from(server_id));
        let is_depot = item_type.kind == crate::items::ItemKind::Depot;
        if item_type.group == crate::items::ItemGroup::Container {
            let depot_id = if is_depot && pos.x != 0xFFFF {
                let idx = if item_index >= 0 { item_index as usize } else { 0 };
                game.map.get_tile(pos)
                    .and_then(|t| t.items.get(idx))
                    .map(|it| it.depot_id as u32)
                    .unwrap_or(0)
            } else { 0 };
            drop(game);
            // Container opening needs session context for the 0x6E packet.
            // TODO: port open_container_*/open_depot to free functions
            return;
        }
    }

    {
        let game = g_game().lock().unwrap();
        let item_type = game.items.get_item_type(usize::from(server_id));
        if item_type.can_read_text {
            // Text window needs session context for the window packet.
            // The read/write window is already handled via send_text_window_to_player (free fn).
            let can_write = item_type.can_write_text;
            let max_text_len = item_type.max_text_len;
            let client_id = item_type.client_id;
            let (item_text, item_writer, item_date, item_count) = if pos.x == 0xFFFF {
                let slot = usize::from(pos.y);
                game.get_player(creature_id)
                    .and_then(|p| p.inventory_items[slot].as_ref())
                    .map(|it| (it.text.clone(), it.written_by.clone(), it.written_date, it.count))
                    .unwrap_or_default()
            } else {
                game.map.get_tile(pos)
                    .and_then(|t| t.get_use_item(stackpos))
                    .map(|it| (it.text.clone(), it.written_by.clone(), it.written_date, it.count))
                    .unwrap_or_default()
            };
            let item_count_byte = if item_type.stackable {
                Some(item_count.clamp(1, 255) as u8)
            } else if item_type.group == ItemGroup::Splash || item_type.group == ItemGroup::Fluid {
                static FLUID_MAP: [u8; 8] = [0, 6, 7, 2, 1, 5, 4, 9];
                Some(FLUID_MAP[(item_count & 7) as usize])
            } else {
                None
            };

            if let Some(player) = game.get_player(creature_id) {
                let window_text_id = player.window_text_id.wrapping_add(1);
                drop(game);
                {
                    let mut game = g_game().lock().unwrap();
                    if let Some(player) = game.get_player_mut(creature_id) {
                        player.window_text_id = window_text_id;
                        if can_write {
                            player.write_item_id = Some(server_id);
                            player.write_item_pos = Some(pos);
                            player.write_item_stack_pos = stackpos;
                            player.max_write_len = max_text_len;
                        } else {
                            player.write_item_id = None;
                            player.write_item_pos = None;
                            player.max_write_len = 0;
                        }
                    }
                }
                send_text_window_to_player(creature_id, window_text_id, client_id, item_count_byte, &item_text, &item_writer, item_date, can_write, max_text_len);
            }
            return;
        }
    }

    let new_pos = {
        let game = g_game().lock().unwrap();
        if USE_TELEPORT_UP_IDS.contains(&server_id) {
            game.map.move_upstairs_position(pos)
        } else if USE_TELEPORT_DOWN_IDS.contains(&server_id) {
            pos.z.checked_add(1).and_then(|z| {
                let candidate = Position { x: pos.x, y: pos.y, z };
                game.map.get_tile(candidate).map(|_| candidate)
            })
        } else {
            let item_type = game.items.get_item_type(usize::from(server_id));
            if item_type.floor_change != 0 {
                game.map.resolve_floor_change_destination(pos)
            } else {
                None
            }
        }
    };

    let Some(new_pos) = new_pos else { return; };

    if !can_teleport_to(new_pos, is_pz_locked) {
        send_status_message_to_player(creature_id, "Sorry, not possible.");
        return;
    }

    game_teleport_player(creature_id, old_pos, new_pos);
}

fn game_handle_use_item_ex(creature_id: CreatureId, from_pos: Position, from_sprite_id: u16, from_stackpos: u8, to_pos: Position, _to_sprite_id: u16, to_stackpos: u8) {
    let Some((server_id, _old_pos, _is_pz_locked)) =
        resolve_player_item_for_use(creature_id, from_pos, from_stackpos, from_sprite_id, true)
    else {
        send_status_message_to_player(creature_id, "Sorry, not possible.");
        return;
    };

    let (action_id, unique_id, item_index) = {
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(from_pos) {
            let item = tile.get_use_item(from_stackpos);
            match item {
                Some(item) => (item.action_id, item.unique_id, from_stackpos as i32 - if tile.ground.is_some() { 1 } else { 0 }),
                None => (0u16, 0u16, -1i32),
            }
        } else {
            (0, 0, -1)
        }
    };

    if crate::events::dispatch::execute_action_use_ex(
        creature_id, from_pos, server_id, item_index,
        action_id, unique_id, to_pos, to_stackpos, false,
    ) {
        return;
    }

    // Rope / teleport fallbacks handled similarly to handle_use_item.
    let is_rope = ROPE_ITEM_IDS.contains(&server_id);
    if is_rope {
        let new_pos = {
            let game = g_game().lock().unwrap();
            game.map.move_upstairs_position(to_pos)
        };
        if let Some(new_pos) = new_pos {
            let old_pos = {
                let game = g_game().lock().unwrap();
                game.get_player(creature_id).map(|p| p.base.position).unwrap_or(from_pos)
            };
            game_teleport_player(creature_id, old_pos, new_pos);
        }
    }
}

fn game_handle_use_with_creature(creature_id: CreatureId, from_pos: Position, from_sprite_id: u16, from_stackpos: u8, target_creature_id: u32) {
    let Some((server_id, _old_pos, _is_pz_locked)) =
        resolve_player_item_for_use(creature_id, from_pos, from_stackpos, from_sprite_id, true)
    else {
        send_status_message_to_player(creature_id, "You cannot use this object.");
        return;
    };

    let (action_id, unique_id, item_index) = {
        let game = g_game().lock().unwrap();
        if let Some(tile) = game.map.get_tile(from_pos) {
            let item = tile.get_use_item(from_stackpos);
            match item {
                Some(item) => (item.action_id, item.unique_id, from_stackpos as i32 - if tile.ground.is_some() { 1 } else { 0 }),
                None => (0u16, 0u16, -1i32),
            }
        } else {
            (0, 0, -1)
        }
    };

    let target_pos = {
        let game = g_game().lock().unwrap();
        game.get_creature(target_creature_id).map(|c| c.position()).unwrap_or(from_pos)
    };

    crate::events::dispatch::execute_action_use_ex(
        creature_id, from_pos, server_id, item_index,
        action_id, unique_id, target_pos, 0, false,
    );
}

#[allow(clippy::too_many_arguments)]
fn game_parse_throw(creature_id: CreatureId, from_pos: Position, sprite_id: u16, from_stackpos: u8, to_pos: Position, count: u8) {
    if creature_id == 0 { return; }
    // The full parse_throw is the most complex handler (~500 lines: ground↔inventory↔container moves).
    // It delegates to handle_container_move, handle_push_creature, and various extract/insert paths.
    // All those sub-methods already use send_packet_to_player or the session map.
    // For now, delegate to the old method's game-state logic directly.
    // TODO: fully port the parse_throw body when container model is finalized.

    // Decode endpoints.
    let from_is_container = from_pos.x == 0xFFFF && (from_pos.y & 0x40) != 0;
    let to_is_container = to_pos.x == 0xFFFF && (to_pos.y & 0x40) != 0;
    let from_is_inventory = from_pos.x == 0xFFFF && !from_is_container;
    let to_is_inventory = to_pos.x == 0xFFFF && !to_is_container;

    if from_is_inventory && to_is_inventory {
        // Inventory slot swap
        let from_slot = from_pos.y as usize;
        let to_slot = to_pos.y as usize;
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.inventory.swap(from_slot, to_slot);
            player.inventory_count.swap(from_slot, to_slot);
            player.inventory_items.swap(from_slot, to_slot);
        }
        drop(game);
        send_full_inventory(creature_id);
    }
    // Other move types (ground→inv, inv→ground, container moves) need full port.
    // The old parse_throw on ProtocolGame is still available for reference.
}

fn game_teleport_player(creature_id: CreatureId, old_pos: Position, new_pos: Position) {
    let stackpos = {
        let game = g_game().lock().unwrap();
        game.map.get_tile(old_pos)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1)
    };
    let old_stackpos = if stackpos >= 0 { stackpos as u8 } else { 0 };

    {
        let mut game = g_game().lock().unwrap();
        game.move_creature_position(creature_id, old_pos, new_pos);
    }

    let sessions = player_sessions().lock().unwrap();
    if let Some(session) = sessions.get(&creature_id) {
        let game = g_game().lock().unwrap();
        let mut known = session.known_creatures.lock().unwrap();

        let mut output = OutputMessage::new();
        write_remove_tile_creature(&mut output, old_pos, old_stackpos, creature_id);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);

        let mut output = OutputMessage::new();
        if let Some(player) = game.get_player(creature_id) {
            write_map_description(
                &mut output,
                &game,
                game.get_items(),
                new_pos,
                &mut known,
                creature_id,
                player,
            );
        }
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }

    broadcast_creature_teleport(creature_id, old_pos, old_stackpos, new_pos);
}

/// Shared logout path: remove player from game, broadcast disappear, save to DB.
fn perform_player_logout(creature_id: u32) {
    let (name, pos, stackpos, save_snapshot, player_guid) = {
        let mut game = g_game().lock().unwrap();
        let (name, pos, snapshot, guid) = match game.get_player_mut(creature_id) {
            Some(p) => {
                p.last_logout = crate::util::get_milliseconds_time() / 1000;
                p.login_position = p.base.position;
                let name = p.name.clone();
                let pos = p.base.position;
                let guid = p.guid;
                let snap = crate::db::login::PlayerSaveSnapshot::from_player(p);
                (name, pos, Some(snap), guid)
            }
            None => return, // already removed
        };
        let sp = game.map.get_tile(pos)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1);
        (name, pos, sp, snapshot, guid)
    };

    unregister_player_connection(creature_id);
    {
        let mut game = g_game().lock().unwrap();
        game.remove_creature_check(creature_id);
        game.remove_player(creature_id);
    }

    if player_guid != 0 {
        broadcast_vip_status(player_guid, false);
    }

    if stackpos >= 0 {
        broadcast_creature_disappear(creature_id, pos, stackpos as u8);
    }

    crate::events::dispatch::execute_creature_event_logout(creature_id);
    if !name.is_empty() && g_config().get_boolean(BooleanConfig::PlayerConsoleLogs) {
        println!("> {} has logged out.", name);
    }

    if let Some(snap) = save_snapshot {
        let pg = player_guid;
        tokio::spawn(async move {
            crate::db::login::update_online_status(pg, false).await;
            crate::db::login::save_player(&snap).await;
        });
    } else if player_guid != 0 {
        tokio::spawn(async move {
            crate::db::login::update_online_status(player_guid, false).await;
        });
    }
}

/// Kick a player by creature id: remove from game and disconnect the socket.
/// Port of `Game::kickPlayer` (displayEffect handled at the call site).
pub fn kick_player_by_id(creature_id: CreatureId) {
    if creature_id == 0 {
        return;
    }
    let conn = {
        let sessions = player_sessions().lock().unwrap();
        sessions.get(&creature_id).map(|s| s.conn.clone())
    };
    perform_player_logout(creature_id);
    if let Some(conn) = conn {
        conn.disconnect();
    }
}

impl Drop for ProtocolGame {
    fn drop(&mut self) {
        tracing::info!(creature_id = self.creature_id, accept_packets = self.accept_packets, "ProtocolGame::drop");
        if self.accept_packets && self.creature_id != 0 {
            tracing::info!(creature_id = self.creature_id, "ProtocolGame::drop: fallback logout");
            perform_player_logout(self.creature_id);
        }
    }
}

/// Resolve an item the player wants to use. `expect_useable` mirrors the
/// C++ split: `playerUseItem` (0x82) rejects items that ARE useable, while
/// `playerUseItemEx`/`playerUseWithCreature` (0x83/0x84) require them to be.
fn resolve_player_item_for_use(
    creature_id: u32,
    pos: Position,
    stackpos: u8,
    sprite_id: u16,
    expect_useable: bool,
) -> Option<(u16, Position, bool)> {
    let game = g_game().lock().unwrap();
    let player = game.get_player(creature_id)?;

    if pos.x == 0xFFFF {
        let slot = usize::from(pos.y);
        let server_id = player.get_inventory_item(slot)?;
        let item_type = game.items.get_item_type(usize::from(server_id));
        if item_type.useable != expect_useable || item_type.client_id != sprite_id {
            return None;
        }
        return Some((server_id, player.base.position, player.is_pz_locked()));
    }

    let tile = game.map.get_tile(pos)?;
    let item = tile.get_use_item(stackpos)?;
    let item_type = game.items.get_item_type(usize::from(item.server_id));
    if item_type.useable != expect_useable || item_type.client_id != sprite_id {
        return None;
    }

    if player.base.position.z != pos.z || !is_adjacent(player.base.position, pos) {
        return None;
    }

    Some((item.server_id, player.base.position, player.is_pz_locked()))
}

fn send_status_message(crypto: &mut ProtocolCrypto, conn: &ConnectionHandle, message: &[u8]) {
    let mut output = OutputMessage::new();
    output.add_byte(0xB4);
    output.add_byte(translate_message_class_to_client(26)); // MESSAGE_STATUS_SMALL
    output.add_string(message);
    crypto.finalize_output(&mut output);
    conn.send_bytes(output.get_output_buffer().to_vec());
}

fn send_cancel_walk(crypto: &mut ProtocolCrypto, conn: &ConnectionHandle, creature_id: CreatureId) {
    let game = g_game().lock().unwrap();
    let dir = game.get_player(creature_id)
        .map(|p| p.base.direction)
        .unwrap_or(Direction::South);
    drop(game);

    let mut output = OutputMessage::new();
    output.add_byte(0xB5);
    output.add_byte(dir as u8);
    crypto.finalize_output(&mut output);
    conn.send_bytes(output.get_output_buffer().to_vec());
}

pub fn stop_auto_walk(creature_id: CreatureId) {
    let mut game = g_game().lock().unwrap();
    if let Some(player) = game.get_player_mut(creature_id) {
        player.base.list_walk_dir.clear();
        if player.base.event_walk != 0 {
            crate::runtime::g_scheduler().stop_event(player.base.event_walk);
            player.base.event_walk = 0;
        }
    }
}

fn schedule_auto_walk_step(creature_id: CreatureId) {
    let step_duration = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        if player.base.list_walk_dir.is_empty() { return; }

        let pos = player.base.position;
        let ground_speed = game.map.get_tile(pos)
            .and_then(|t| t.ground.as_ref())
            .map(|g| {
                let item_type = game.items.get_item_type(usize::from(g.server_id));
                item_type.speed as u32
            })
            .unwrap_or(150);

        let step_speed = player.get_step_speed().max(1) as u32;
        let dir = player.base.list_walk_dir.last().copied().unwrap_or(Direction::South);
        let is_diagonal = matches!(dir,
            Direction::NorthWest | Direction::NorthEast |
            Direction::SouthWest | Direction::SouthEast
        );

        let mut duration = (1000u64 * ground_speed as u64) / step_speed as u64;
        duration = ((duration + 49) / 50) * 50;
        if is_diagonal { duration *= 3; }
        duration.max(50)
    };

    let task = crate::runtime::scheduler::SchedulerTask::new(
        step_duration as u32,
        move || execute_auto_walk_step(creature_id),
    );
    let event_id = crate::runtime::g_scheduler().add_event(task);

    let mut game = g_game().lock().unwrap();
    if let Some(player) = game.get_player_mut(creature_id) {
        player.base.event_walk = event_id;
    }
}

fn execute_auto_walk_step(creature_id: CreatureId) {
    let dir = {
        let mut game = g_game().lock().unwrap();
        let Some(player) = game.get_player_mut(creature_id) else { return };
        player.base.event_walk = 0;
        match player.base.list_walk_dir.pop() {
            Some(d) => d,
            None => return,
        }
    };

    let (old_pos, new_pos, stackpos, blocked) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let old = player.base.position;
        let new = match step_position(&game.map, old, dir) {
            Some(p) => p,
            None => {
                drop(game);
                stop_auto_walk(creature_id);
                send_cancel_walk_to_session(creature_id);
                return;
            }
        };

        let sp = game.map.get_tile(old)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1);
        let stackpos = if sp >= 0 { sp as u8 } else { 0 };

        let blocked = match game.map.get_tile(new) {
            None => true,
            Some(t) if t.ground.is_none() => true,
            Some(t) if t.has_flag(TILESTATE_BLOCKSOLID) => true,
            _ => false,
        };

        (old, new, stackpos, blocked)
    };

    if blocked {
        stop_auto_walk(creature_id);
        send_cancel_walk_to_session(creature_id);
        return;
    }

    {
        let mut game = g_game().lock().unwrap();
        game.move_creature_position(creature_id, old_pos, new_pos);
        if let Some(player) = game.get_player_mut(creature_id) {
            player.base.direction = dir;
            player.base.last_step = crate::util::get_milliseconds_time() as u64;
        }
    }

    {
        let game = g_game().lock().unwrap();
        let sessions = player_sessions().lock().unwrap();
        if let Some(session) = sessions.get(&creature_id) {
            let mut known = session.known_creatures.lock().unwrap();

            let mut output = OutputMessage::new();
            if old_pos.z == 7 && new_pos.z >= 8 {
                write_remove_tile_creature(&mut output, old_pos, stackpos, creature_id);
            } else {
                output.add_byte(0x6D);
                write_creature_movement(&mut output, old_pos, new_pos, stackpos, creature_id);
            }
            append_walk_map_slices(&mut output, &game, game.get_items(), &mut known, old_pos, new_pos);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }

    broadcast_creature_move(creature_id, old_pos, new_pos, stackpos);

    let has_more = {
        let game = g_game().lock().unwrap();
        game.get_player(creature_id)
            .map(|p| !p.base.list_walk_dir.is_empty())
            .unwrap_or(false)
    };

    if has_more {
        schedule_auto_walk_step(creature_id);
    }
}

pub fn send_cancel_walk_to_session(creature_id: CreatureId) {
    let game = g_game().lock().unwrap();
    let sessions = player_sessions().lock().unwrap();
    if let Some(session) = sessions.get(&creature_id) {
        let dir_byte = game.get_player(creature_id)
            .map(|p| p.base.direction as u8)
            .unwrap_or(2);
        drop(game);
        let mut output = OutputMessage::new();
        output.add_byte(0xB5);
        output.add_byte(dir_byte);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

fn can_teleport_to(position: Position, is_pz_locked: bool) -> bool {
    let game = g_game().lock().unwrap();
    let Some(tile) = game.map.get_tile(position) else {
        return false;
    };

    if tile.ground.is_none() || tile.has_flag(TILESTATE_BLOCKSOLID) {
        return false;
    }

    if is_pz_locked && tile.has_flag(TILESTATE_PROTECTIONZONE) {
        return false;
    }

    true
}

fn is_adjacent(a: Position, b: Position) -> bool {
    a.x.abs_diff(b.x) <= 1 && a.y.abs_diff(b.y) <= 1
}

fn to_map_position(position: crate::net::message::Position) -> Position {
    Position {
        x: position.x,
        y: position.y,
        z: position.z,
    }
}

// ── Map description helpers ───────────────────────────────────────────────────

// ── Container move support ──────────────────────────────────────────────────

/// A decoded source/destination for an item move (C++ `internalGetCylinder`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveEndpoint {
    Container { cid: u8, slot: u8 },
    Inventory { slot: usize },
    Ground { pos: Position, stackpos: u8 },
}

impl MoveEndpoint {
    fn decode(pos: Position, stackpos: u8) -> Self {
        if pos.x == 0xFFFF {
            if (pos.y & 0x40) != 0 {
                MoveEndpoint::Container { cid: (pos.y & 0x0F) as u8, slot: pos.z }
            } else {
                MoveEndpoint::Inventory { slot: pos.y as usize }
            }
        } else {
            MoveEndpoint::Ground { pos, stackpos }
        }
    }
}

/// Where an open container's backing item ultimately lives.
#[derive(Debug, Clone)]
enum StorageRoot {
    TileItem(Position, usize),
    InvItem(usize),
    /// Depot chest contents (`Player.depot_items[depot_id]`) — a Vec root with
    /// no wrapping item, so an empty path refers to the chest contents directly.
    Depot(u32),
}

/// Resolve an open container `cid` to its storage root, the child-index path
/// from that root item down to the container item, and its scroll offset.
/// Mirrors walking the `ContainerParent` chain in C++.
fn resolve_container_storage(
    player: &Player,
    cid: u8,
) -> Option<(StorageRoot, Vec<usize>, u16)> {
    use crate::creatures::player::ContainerParent;
    let oc = player.get_container_by_id(cid)?;
    let scroll = oc.scroll_index;
    match oc.parent {
        ContainerParent::Tile(pos, idx) => Some((StorageRoot::TileItem(pos, idx), Vec::new(), scroll)),
        ContainerParent::Inventory(slot) => Some((StorageRoot::InvItem(slot as usize), Vec::new(), scroll)),
        ContainerParent::Container(parent_cid, child_idx) => {
            let (root, mut path, _) = resolve_container_storage(player, parent_cid)?;
            path.push(child_idx);
            Some((root, path, scroll))
        }
        ContainerParent::Depot(depot_id) => Some((StorageRoot::Depot(depot_id), Vec::new(), scroll)),
    }
}

/// Mutable access to the children Vec of the container item identified by
/// `(root, path)`.
fn container_children_mut<'a>(
    game: &'a mut crate::game::Game,
    creature_id: CreatureId,
    root: &StorageRoot,
    path: &[usize],
) -> Option<&'a mut Vec<crate::map::tile::MapItem>> {
    // Depot roots are a Vec (the chest contents) with no wrapping item.
    if let StorageRoot::Depot(depot_id) = *root {
        let vec = game.get_player_mut(creature_id)?.depot_items.get_mut(&depot_id)?;
        if path.is_empty() {
            return Some(vec);
        }
        let mut cur = vec.get_mut(path[0])?;
        for &i in &path[1..] {
            cur = cur.children.get_mut(i)?;
        }
        return Some(&mut cur.children);
    }
    let root_item: &mut crate::map::tile::MapItem = match *root {
        StorageRoot::TileItem(pos, idx) => game.map.get_tile_mut(pos)?.items.get_mut(idx)?,
        StorageRoot::InvItem(slot) => game.get_player_mut(creature_id)?.inventory_items.get_mut(slot)?.as_mut()?,
        StorageRoot::Depot(_) => unreachable!(),
    };
    let mut cur = root_item;
    for &i in path {
        cur = cur.children.get_mut(i)?;
    }
    Some(&mut cur.children)
}

/// Shared access to the container item identified by `(root, path)`.
fn container_item_ref<'a>(
    game: &'a crate::game::Game,
    creature_id: CreatureId,
    root: &StorageRoot,
    path: &[usize],
) -> Option<&'a crate::map::tile::MapItem> {
    // Depot roots have no wrapping item; an empty path has no item to return.
    if let StorageRoot::Depot(depot_id) = *root {
        let vec = game.get_player(creature_id)?.depot_items.get(&depot_id)?;
        if path.is_empty() {
            return None;
        }
        let mut cur = vec.get(path[0])?;
        for &i in &path[1..] {
            cur = cur.children.get(i)?;
        }
        return Some(cur);
    }
    let mut cur: &crate::map::tile::MapItem = match *root {
        StorageRoot::TileItem(pos, idx) => game.map.get_tile(pos)?.items.get(idx)?,
        StorageRoot::InvItem(slot) => game.get_player(creature_id)?.inventory_items.get(slot)?.as_ref()?,
        StorageRoot::Depot(_) => unreachable!(),
    };
    for &i in path {
        cur = cur.children.get(i)?;
    }
    Some(cur)
}

/// Remove and return the item at a move source. `None` if it can't be resolved.
fn extract_move_item(
    game: &mut crate::game::Game,
    creature_id: CreatureId,
    from: &MoveEndpoint,
) -> Option<crate::map::tile::MapItem> {
    use crate::map::tile::MapItem;
    match *from {
        MoveEndpoint::Container { cid, slot } => {
            let (root, path, scroll) = {
                let player = game.get_player(creature_id)?;
                resolve_container_storage(player, cid)?
            };
            let children = container_children_mut(game, creature_id, &root, &path)?;
            let real_idx = scroll as usize + slot as usize;
            if real_idx >= children.len() { return None; }
            Some(children.remove(real_idx))
        }
        MoveEndpoint::Inventory { slot } => {
            use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST};
            if !(CONST_SLOT_FIRST..=CONST_SLOT_LAST).contains(&slot) { return None; }
            let player = game.get_player_mut(creature_id)?;
            let sid = player.inventory[slot]?;
            let count = player.inventory_count[slot].max(1);
            let item = player.inventory_items[slot].take()
                .unwrap_or(MapItem { server_id: sid, count, ..MapItem::default() });
            player.inventory[slot] = None;
            player.inventory_count[slot] = 0;
            Some(item)
        }
        MoveEndpoint::Ground { pos, stackpos } => {
            let tile = game.map.get_tile_mut(pos)?;
            let ground_off = if tile.ground.is_some() { 1 } else { 0 };
            let idx = (stackpos as usize).checked_sub(ground_off + tile.creature_ids.len())?;
            if idx >= tile.items.len() { return None; }
            Some(tile.items.remove(idx))
        }
    }
}

/// Insert `item` at a move destination. Returns false if it can't be placed.
fn insert_move_item(
    game: &mut crate::game::Game,
    creature_id: CreatureId,
    to: &MoveEndpoint,
    item: crate::map::tile::MapItem,
) -> bool {
    match *to {
        MoveEndpoint::Container { cid, .. } => {
            let resolved = {
                match game.get_player(creature_id).and_then(|p| resolve_container_storage(p, cid)) {
                    Some(r) => r,
                    None => return false,
                }
            };
            let (root, path, _) = resolved;
            match container_children_mut(game, creature_id, &root, &path) {
                // New items are placed at the top of the container (index 0),
                // matching the client's expectation for a fresh insert.
                Some(children) => { children.insert(0, item); true }
                None => false,
            }
        }
        MoveEndpoint::Inventory { slot } => {
            use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST};
            if !(CONST_SLOT_FIRST..=CONST_SLOT_LAST).contains(&slot) { return false; }
            let Some(player) = game.get_player_mut(creature_id) else { return false };
            if player.inventory[slot].is_some() { return false; }
            player.inventory[slot] = Some(item.server_id);
            player.inventory_count[slot] = item.count.max(1);
            // Keep the full item (attributes + any container tree) so its
            // attributes persist on save.
            player.inventory_items[slot] = Some(item);
            true
        }
        MoveEndpoint::Ground { pos, .. } => {
            let has_mailbox = game
                .map
                .get_tile(pos)
                .map(|t| t.has_flag(crate::map::tile::TILESTATE_MAILBOX))
                .unwrap_or(false);
            if has_mailbox
                && crate::items::special::mailbox::Mailbox::can_send(item.server_id)
                && mailbox_deliver(game, &item)
            {
                return true;
            }
            let items_arc = game.items.clone();
            match game.map.get_tile_mut(pos) {
                Some(tile) if tile.ground.is_some() => {
                    tile.internal_add_item(item, &items_arc);
                    true
                }
                _ => false,
            }
        }
    }
}

/// Parse a mailbox item's receiver. Port of `Mailbox::getReceiver`:
/// parcels (containers) are scanned for an `ITEM_LABEL` child whose text holds
/// the name; letters carry the text directly (line 1 = name, line 2 = town).
fn mailbox_get_receiver(
    game: &crate::game::Game,
    item: &crate::map::tile::MapItem,
) -> Option<(String, u32)> {
    const ITEM_LABEL: u16 = 2599;
    let is_container = game.items.get_item_type(usize::from(item.server_id)).group
        == crate::items::ItemGroup::Container;
    if is_container {
        for child in &item.children {
            if child.server_id == ITEM_LABEL {
                if let Some(r) = mailbox_get_receiver(game, child) {
                    return Some(r);
                }
            }
        }
        return None;
    }

    if item.text.trim().is_empty() {
        return None;
    }
    let mut lines = item.text.lines();
    let name = lines.next().unwrap_or("").trim().to_string();
    let town_name = lines.next().unwrap_or("").trim();
    let depot_id = game
        .map
        .towns
        .values()
        .find(|t| t.name.eq_ignore_ascii_case(town_name))
        .map(|t| t.id)?;
    Some((name, depot_id))
}

/// Deliver a parcel/letter into the receiver's depot. Port of `Mailbox::sendItem`
/// for the online-receiver path: transform the item to `id+1` (stamped) and push
/// it into the receiver's depot, then `onReceiveMail`. Offline receivers are not
/// handled here (would require a synchronous DB load/save in the move path).
fn mailbox_deliver(game: &mut crate::game::Game, item: &crate::map::tile::MapItem) -> bool {
    let Some((name, depot_id)) = mailbox_get_receiver(game, item) else { return false };
    if name.is_empty() || depot_id == 0 {
        return false;
    }
    let Some(cid) = game.get_player_id_by_name(&name) else { return false };

    let mut delivered = item.clone();
    delivered.server_id = item.server_id.saturating_add(1);
    match game.get_player_mut(cid) {
        Some(p) => p.depot_items.entry(depot_id).or_default().insert(0, delivered),
        None => return false,
    }

    if player_is_near_depot_box(game, cid) {
        game.send_text_message(
            cid,
            crate::world::raids::MESSAGE_EVENT_ADVANCE,
            "New mail has arrived.".to_string(),
        );
    }
    true
}

/// Port of `Player::isNearDepotBox`: a depot tile within 1 step of the player.
fn player_is_near_depot_box(game: &crate::game::Game, creature_id: CreatureId) -> bool {
    let Some(pos) = game.get_player(creature_id).map(|p| p.base.position) else { return false };
    for cx in -1i32..=1 {
        for cy in -1i32..=1 {
            let p = Position {
                x: pos.x.wrapping_add(cx as u16),
                y: pos.y.wrapping_add(cy as u16),
                z: pos.z,
            };
            if game
                .map
                .get_tile(p)
                .map(|t| t.has_flag(crate::map::tile::TILESTATE_DEPOT))
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

// ── Secure-trade support ────────────────────────────────────────────────────

/// Clone the item a player is offering for trade (without removing it) and
/// return its location for later extraction. Tile items use the top-down item
/// at the request stackpos (C++ STACKPOS_TOPDOWN_ITEM).
fn peek_trade_item(
    game: &crate::game::Game,
    creature_id: CreatureId,
    ep: &MoveEndpoint,
) -> Option<(crate::map::tile::MapItem, crate::creatures::player::TradeItemLoc)> {
    use crate::creatures::player::TradeItemLoc;
    use crate::map::tile::MapItem;
    match *ep {
        MoveEndpoint::Ground { pos, stackpos } => {
            let tile = game.map.get_tile(pos)?;
            let ground_off = if tile.ground.is_some() { 1 } else { 0 };
            let idx = (stackpos as usize).checked_sub(ground_off + tile.creature_ids.len())?;
            let item = tile.items.get(idx)?.clone();
            Some((item, TradeItemLoc::Tile(pos, idx)))
        }
        MoveEndpoint::Inventory { slot } => {
            use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST};
            if !(CONST_SLOT_FIRST..=CONST_SLOT_LAST).contains(&slot) { return None; }
            let player = game.get_player(creature_id)?;
            let sid = player.inventory[slot]?;
            let item = player.inventory_items[slot].clone()
                .unwrap_or(MapItem { server_id: sid, count: player.inventory_count[slot].max(1), ..MapItem::default() });
            Some((item, TradeItemLoc::Inventory(slot)))
        }
        MoveEndpoint::Container { cid, slot } => {
            let player = game.get_player(creature_id)?;
            let (root, path, scroll) = resolve_container_storage(player, cid)?;
            let children = container_item_ref(game, creature_id, &root, &path)?;
            let real_idx = scroll as usize + slot as usize;
            let item = children.children.get(real_idx)?.clone();
            Some((item, TradeItemLoc::Container(cid, real_idx)))
        }
    }
}

/// Remove a player's offered trade item from wherever it lives.
fn extract_trade_item(
    game: &mut crate::game::Game,
    creature_id: CreatureId,
    loc: &crate::creatures::player::TradeItemLoc,
) -> Option<crate::map::tile::MapItem> {
    use crate::creatures::player::TradeItemLoc;
    match *loc {
        TradeItemLoc::Tile(pos, idx) => {
            let tile = game.map.get_tile_mut(pos)?;
            if idx >= tile.items.len() { return None; }
            Some(tile.items.remove(idx))
        }
        TradeItemLoc::Inventory(slot) => {
            let player = game.get_player_mut(creature_id)?;
            let sid = player.inventory[slot]?;
            let item = player.inventory_items[slot].take()
                .unwrap_or(crate::map::tile::MapItem { server_id: sid, count: player.inventory_count[slot].max(1), ..crate::map::tile::MapItem::default() });
            player.inventory[slot] = None;
            player.inventory_count[slot] = 0;
            Some(item)
        }
        TradeItemLoc::Container(cid, idx) => {
            let (root, path) = {
                let player = game.get_player(creature_id)?;
                let (r, p, _) = resolve_container_storage(player, cid)?;
                (r, p)
            };
            let children = container_children_mut(game, creature_id, &root, &path)?;
            if idx >= children.len() { return None; }
            Some(children.remove(idx))
        }
    }
}

/// Add a received trade item to a player: first free equipment slot, else into
/// the equipped backpack's contents. Returns false if there is no room.
/// Mirrors C++ `internalAddItem(player, item, INDEX_WHEREEVER)` for the common
/// player-inventory cases.
fn add_item_to_player(
    game: &mut crate::game::Game,
    creature_id: CreatureId,
    item: crate::map::tile::MapItem,
) -> bool {
    use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST, CONST_SLOT_BACKPACK};
    let Some(player) = game.get_player_mut(creature_id) else { return false };
    if let Some(slot) = (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()) {
        player.inventory[slot] = Some(item.server_id);
        player.inventory_count[slot] = item.count.max(1);
        player.inventory_items[slot] = Some(item);
        return true;
    }
    // No free slot — try to drop into the equipped backpack.
    if let Some(Some(bp)) = player.inventory_items.get_mut(CONST_SLOT_BACKPACK) {
        bp.children.insert(0, item);
        return true;
    }
    false
}

/// Send a trade offer window (0x7D own / 0x7E counter) listing the item and,
/// if it is a container, its full contents. Mirrors `sendTradeItemRequest`.
fn send_trade_offer(target_id: CreatureId, trader_name: &str, item: &crate::map::tile::MapItem, ack: bool) {
    let name = trader_name.to_owned();
    let item = item.clone();
    send_packet_to_player(target_id, move |output: &mut OutputMessage| {
        output.add_byte(if ack { 0x7D } else { 0x7E });
        output.add_string(name.as_bytes());
        let items_arc = g_game().lock().unwrap().items.clone();
        let is_container = items_arc.get_item_type(usize::from(item.server_id)).group
            == crate::items::ItemGroup::Container;
        if is_container {
            // Flatten the container tree depth-first (container itself first).
            let mut flat: Vec<&crate::map::tile::MapItem> = Vec::new();
            fn collect<'a>(it: &'a crate::map::tile::MapItem, out: &mut Vec<&'a crate::map::tile::MapItem>) {
                for child in &it.children {
                    out.push(child);
                    collect(child, out);
                }
            }
            flat.push(&item);
            collect(&item, &mut flat);
            output.add_byte(flat.len().min(0xFF) as u8);
            for it in flat.into_iter().take(0xFF) {
                write_item(output, &items_arc, it.server_id, it.count.max(1) as u8);
            }
        } else {
            output.add_byte(0x01);
            write_item(output, &items_arc, item.server_id, item.count.max(1) as u8);
        }
    });
}

/// Send the close-trade packet (0x7F).
fn send_trade_close(target_id: CreatureId) {
    send_packet_to_player(target_id, |output: &mut OutputMessage| {
        output.add_byte(0x7F);
    });
}

fn write_map_description(
    output: &mut OutputMessage,
    game: &crate::game::Game,
    items: &Items,
    pos: Position,
    known: &mut HashSet<u32>,
    creature_id: u32,
    player: &Player,
) {
    output.add_byte(0x64);
    output.add_position(pos.x, pos.y, pos.z);

    let x = pos.x as i32 - MAX_VIEWPORT_X;
    let y = pos.y as i32 - MAX_VIEWPORT_Y;
    let z = pos.z as i32;
    let width = MAX_VIEWPORT_X * 2 + 2;
    let height = MAX_VIEWPORT_Y * 2 + 2;

    let mut skip: i32 = -1;
    let (startz, endz, zstep) = floor_range(z);
    for nz in floor_iter(startz, endz, zstep) {
        get_floor_description(output, game, items, x, y, nz, width, height, z - nz, &mut skip,
            known, Some((creature_id, player)));
    }
    if skip >= 0 {
        output.add_byte(skip as u8);
        output.add_byte(0xFF);
    }

    let buf = output.get_output_buffer();
    let hex: String = buf.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
    std::fs::write("map_dump.hex", &hex).ok();
    tracing::info!(len = buf.len(), "MAP_DUMP written to map_dump.hex");

    let mut anim_count = 0u32;
    let mut total = 0u32;
    for sid in 0..30000u16 {
        let it = items.get_item_type(sid as usize);
        if it.client_id > 0 { total += 1; }
        if it.is_animation { anim_count += 1; }
    }
    tracing::info!(anim_count, total, "ANIM_STATS");
}

#[allow(clippy::too_many_arguments)]
fn get_map_description(
    output: &mut OutputMessage,
    game: &crate::game::Game,
    items: &Items,
    x: i32,
    y: i32,
    z: i32,
    width: i32,
    height: i32,
    known: &mut HashSet<u32>,
) {
    let mut skip: i32 = -1;
    let (startz, endz, zstep) = floor_range(z);
    for nz in floor_iter(startz, endz, zstep) {
        get_floor_description(output, game, items, x, y, nz, width, height, z - nz, &mut skip,
            known, None);
    }
    if skip >= 0 {
        output.add_byte(skip as u8);
        output.add_byte(0xFF);
    }
}

fn floor_range(z: i32) -> (i32, i32, i32) {
    if z > 7 {
        let startz = z - 2;
        let endz = (z + 2).min(MAP_MAX_LAYERS - 1);
        (startz, endz, 1)
    } else {
        (7, 0, -1)
    }
}

fn floor_iter(startz: i32, endz: i32, zstep: i32) -> impl Iterator<Item = i32> {
    let mut nz = startz;
    let endz_inclusive = endz + zstep;
    std::iter::from_fn(move || {
        if nz == endz_inclusive {
            return None;
        }
        let current = nz;
        nz += zstep;
        Some(current)
    })
}

#[allow(clippy::too_many_arguments)]
fn get_floor_description(
    output: &mut OutputMessage,
    game: &crate::game::Game,
    items: &Items,
    x: i32, y: i32, z: i32,
    width: i32, height: i32,
    offset: i32,
    skip: &mut i32,
    known: &mut HashSet<u32>,
    player_at: Option<(u32, &Player)>,
) {
    for nx in 0..width {
        for ny in 0..height {
            let tx = (x + nx + offset) as u16;
            let ty = (y + ny + offset) as u16;
            let tz = z as u8;
            let pos = Position { x: tx, y: ty, z: tz };

            if let Some(tile) = game.map.get_tile(pos) {
                if *skip >= 0 {
                    output.add_byte(*skip as u8);
                    output.add_byte(0xFF);
                }
                *skip = 0;
                let at = player_at.filter(|(_, p)| p.base.position == pos);
                write_tile_description(output, game, tile, items, known, at);
            } else if *skip == 0xFE {
                output.add_byte(0xFF);
                output.add_byte(0xFF);
                *skip = -1;
            } else {
                *skip += 1;
            }
        }
    }
}

fn write_tile_description(
    output: &mut OutputMessage,
    game: &crate::game::Game,
    tile: &Tile,
    items: &Items,
    known: &mut HashSet<u32>,
    player_at: Option<(u32, &Player)>,
) {
    let mut count: i32 = 0;

    if client_version().is_1098() {
        output.add_u16(0x00); // environmental effects
    }

    if let Some(ground) = &tile.ground {
        if write_item(output, items, ground.server_id, ground.count as u8) {
            count += 1;
        }
    }

    let top_start = tile.get_down_item_count();
    for item in &tile.items[top_start..] {
        if count >= 10 { break; }
        if write_item(output, items, item.server_id, item.count as u8) {
            count += 1;
        }
    }

    if count < 10 {
        if let Some((creature_id, _player)) = player_at {
            if let Some(creature) = game.get_creature(creature_id) {
                let (is_new, remove_id) = check_creature_as_known(known, creature_id);
                write_creature_data(output, creature, creature_id, is_new, remove_id);
                count += 1;
            }
        }

        for &cid in tile.get_creatures().iter().rev() {
            if count >= 10 { break; }
            if player_at.map(|(pid, _)| pid == cid).unwrap_or(false) {
                continue;
            }
            if let Some(creature) = game.get_creature(cid) {
                let (is_new, remove_id) = check_creature_as_known(known, cid);
                write_creature_data(output, creature, cid, is_new, remove_id);
                count += 1;
            }
        }
    }

    if count < 10 {
        let down_count = tile.get_down_item_count();
        for item in &tile.items[..down_count] {
            if count >= 10 { break; }
            if write_item(output, items, item.server_id, item.count as u8) {
                count += 1;
            }
        }
    }
}

fn write_creature_data(
    output: &mut OutputMessage,
    creature: &crate::creatures::Creature,
    creature_id: u32,
    is_new: bool,
    remove_id: u32,
) {
    let is_1098 = client_version().is_1098();
    let base = creature.base();
    let creature_type = creature.get_type() as u8;

    if is_new {
        output.add_u16(0x61);
        output.add_u32(remove_id);
        output.add_u32(creature_id);
        if is_1098 {
            output.add_byte(creature_type);
        }
        output.add_string(creature.get_name().as_bytes());
    } else {
        output.add_u16(0x62);
        output.add_u32(creature_id);
    }

    let hp_pct = if base.health_max > 0 {
        ((base.health.clamp(0, base.health_max) as f64 / base.health_max as f64) * 100.0).ceil() as u8
    } else {
        0
    };
    output.add_byte(hp_pct);
    output.add_byte(base.direction as u8);
    write_outfit(output, &base.current_outfit);

    output.add_byte(base.internal_light.level);
    output.add_byte(base.internal_light.color);

    let speed = base.get_speed() as u16;
    if is_1098 {
        output.add_u16(speed / 2);
    } else {
        output.add_u16(speed);
    }

    output.add_byte(base.skull as u8);
    output.add_byte(0x00); // party shield

    if is_new {
        output.add_byte(0x00); // guild emblem
    }

    if is_1098 {
        output.add_byte(creature_type); // creatureType
        output.add_byte(0x00); // speechBubble (SPEECHBUBBLE_NONE)
        output.add_byte(0xFF); // MARK_UNMARKED
        output.add_u16(0x00);  // helpers
    }

    output.add_byte(0x01); // walkthrough/unpassable
}

fn write_creature_appearance(output: &mut OutputMessage, player: &Player, is_new: bool) {
    let is_1098 = client_version().is_1098();

    let hp = player.base.health.clamp(0, player.base.health_max);
    let hp_pct = if player.base.health_max > 0 {
        ((hp as f64 / player.base.health_max as f64) * 100.0).ceil() as u8
    } else {
        0
    };
    output.add_byte(hp_pct);
    output.add_byte(player.base.direction as u8);
    write_outfit(output, &player.base.current_outfit);

    let light = player.base.internal_light;
    output.add_byte(if player.is_access_player() { 0xFF } else { light.level });
    output.add_byte(light.color);

    let speed = player.get_step_speed() as u16;
    if is_1098 {
        output.add_u16(speed / 2);
    } else {
        output.add_u16(speed);
    }

    output.add_byte(skull_client(player.base.skull, player.base.skull) as u8);
    output.add_byte(0x00); // party shield

    if is_new {
        output.add_byte(0x00); // guild emblem
    }

    if is_1098 {
        output.add_byte(0x00); // creatureType (CREATURETYPE_PLAYER)
        output.add_byte(0x00); // speechBubble (SPEECHBUBBLE_NONE)
        output.add_byte(0xFF); // MARK_UNMARKED
        output.add_u16(0x00);  // helpers
    }

    output.add_byte(if is_1098 { 0x00 } else { 0x01 }); // walkthrough/unpassable
}

fn check_creature_as_known(known: &mut HashSet<u32>, id: u32) -> (bool, u32) {
    if known.insert(id) {
        // newly inserted
        if known.len() > 1300 {
            // evict an arbitrary entry to stay within client limit
            if let Some(&remove_id) = known.iter().find(|&&k| k != id) {
                known.remove(&remove_id);
                return (true, remove_id);
            }
        }
        (true, 0) // is_new=true, remove_id=0
    } else {
        (false, 0) // already known
    }
}

fn skull_client(own_skull: Skull, other_skull: Skull) -> Skull {
    let _ = own_skull;
    other_skull
}

fn write_outfit(output: &mut OutputMessage, outfit: &Outfit) {
    output.add_u16(outfit.look_type);
    if outfit.look_type != 0 {
        output.add_byte(outfit.look_head);
        output.add_byte(outfit.look_body);
        output.add_byte(outfit.look_legs);
        output.add_byte(outfit.look_feet);
        output.add_byte(outfit.look_addons);
    } else {
        output.add_u16(outfit.look_type_ex);
    }
    if client_version().is_1098() {
        output.add_u16(outfit.look_mount);
    }
}

fn write_item(output: &mut OutputMessage, items: &Items, server_id: u16, count: u8) -> bool {
    let item_type = items.get_item_type(server_id as usize);
    if item_type.client_id == 0 {
        return false;
    }
    output.add_u16(item_type.client_id);

    if client_version().is_1098() {
        output.add_byte(0xFF); // MARK_UNMARKED
    }

    if item_type.stackable {
        output.add_byte(count.max(1));
    } else if item_type.group == ItemGroup::Splash || item_type.group == ItemGroup::Fluid {
        static FLUID_MAP: [u8; 8] = [0, 6, 7, 2, 1, 5, 4, 9];
        output.add_byte(FLUID_MAP[(count & 7) as usize]);
    }

    if client_version().is_1098() && item_type.is_animation {
        output.add_byte(0xFE); // random animation phase
    }

    true
}

fn write_remove_tile_creature(
    output: &mut OutputMessage,
    pos: Position,
    stackpos: u8,
    creature_id: u32,
) {
    output.add_byte(0x6C);
    if stackpos < 10 {
        output.add_position(pos.x, pos.y, pos.z);
        output.add_byte(stackpos);
    } else {
        output.add_u16(0xFFFF);
        output.add_u32(creature_id);
    }
}

fn write_creature_movement(
    output: &mut OutputMessage,
    old_pos: Position,
    new_pos: Position,
    old_stackpos: u8,
    creature_id: u32,
) {
    if old_stackpos < 10 {
        output.add_position(old_pos.x, old_pos.y, old_pos.z);
        output.add_byte(old_stackpos);
    } else {
        output.add_u16(0xFFFF);
        output.add_u32(creature_id);
    }
    output.add_position(new_pos.x, new_pos.y, new_pos.z);
}

#[allow(dead_code)]
fn move_up_creature(
    output: &mut OutputMessage,
    game: &crate::game::Game,
    items: &Items,
    known: &mut HashSet<u32>,
    new_pos: Position,
    old_pos: Position,
) {
    output.add_byte(0xBE);

    if new_pos.z == 7 {
        let mut skip: i32 = -1;
        for z in (0..=5).rev() {
            get_floor_description(
                output,
                game,
                items,
                old_pos.x as i32 - MAX_VIEWPORT_X,
                old_pos.y as i32 - MAX_VIEWPORT_Y,
                z,
                (MAX_VIEWPORT_X * 2) + 2,
                (MAX_VIEWPORT_Y * 2) + 2,
                8 - z,
                &mut skip,
                known,
                None,
            );
        }
        if skip >= 0 {
            output.add_byte(skip as u8);
            output.add_byte(0xFF);
        }
    } else if new_pos.z > 7 {
        let mut skip: i32 = -1;
        get_floor_description(
            output,
            game,
            items,
            old_pos.x as i32 - MAX_VIEWPORT_X,
            old_pos.y as i32 - MAX_VIEWPORT_Y,
            i32::from(old_pos.z) - 3,
            (MAX_VIEWPORT_X * 2) + 2,
            (MAX_VIEWPORT_Y * 2) + 2,
            3,
            &mut skip,
            known,
            None,
        );
        if skip >= 0 {
            output.add_byte(skip as u8);
            output.add_byte(0xFF);
        }
    }

    output.add_byte(0x68);
    get_map_description(
        output,
        game,
        items,
        old_pos.x as i32 - MAX_VIEWPORT_X,
        old_pos.y as i32 - (MAX_VIEWPORT_Y - 1),
        i32::from(new_pos.z),
        1,
        (MAX_VIEWPORT_Y * 2) + 2,
        known,
    );

    output.add_byte(0x65);
    get_map_description(
        output,
        game,
        items,
        old_pos.x as i32 - MAX_VIEWPORT_X,
        old_pos.y as i32 - MAX_VIEWPORT_Y,
        i32::from(new_pos.z),
        (MAX_VIEWPORT_X * 2) + 2,
        1,
        known,
    );
}

#[allow(dead_code)]
fn move_down_creature(
    output: &mut OutputMessage,
    game: &crate::game::Game,
    items: &Items,
    known: &mut HashSet<u32>,
    new_pos: Position,
    old_pos: Position,
) {
    output.add_byte(0xBF);

    if new_pos.z == 8 {
        let mut skip: i32 = -1;
        for i in 0..3 {
            get_floor_description(
                output,
                game,
                items,
                old_pos.x as i32 - MAX_VIEWPORT_X,
                old_pos.y as i32 - MAX_VIEWPORT_Y,
                i32::from(new_pos.z) + i,
                (MAX_VIEWPORT_X * 2) + 2,
                (MAX_VIEWPORT_Y * 2) + 2,
                -i - 1,
                &mut skip,
                known,
                None,
            );
        }
        if skip >= 0 {
            output.add_byte(skip as u8);
            output.add_byte(0xFF);
        }
    } else if new_pos.z > old_pos.z && new_pos.z > 8 && new_pos.z < 14 {
        let mut skip: i32 = -1;
        get_floor_description(
            output,
            game,
            items,
            old_pos.x as i32 - MAX_VIEWPORT_X,
            old_pos.y as i32 - MAX_VIEWPORT_Y,
            i32::from(new_pos.z) + 2,
            (MAX_VIEWPORT_X * 2) + 2,
            (MAX_VIEWPORT_Y * 2) + 2,
            -3,
            &mut skip,
            known,
            None,
        );
        if skip >= 0 {
            output.add_byte(skip as u8);
            output.add_byte(0xFF);
        }
    }

    output.add_byte(0x66);
    get_map_description(
        output,
        game,
        items,
        i32::from(old_pos.x) + (MAX_VIEWPORT_X + 1),
        i32::from(old_pos.y) - (MAX_VIEWPORT_Y + 1),
        i32::from(new_pos.z),
        1,
        (MAX_VIEWPORT_Y * 2) + 2,
        known,
    );

    output.add_byte(0x67);
    get_map_description(
        output,
        game,
        items,
        old_pos.x as i32 - MAX_VIEWPORT_X,
        i32::from(old_pos.y) + (MAX_VIEWPORT_Y + 1),
        i32::from(new_pos.z),
        (MAX_VIEWPORT_X * 2) + 2,
        1,
        known,
    );
}

fn append_walk_map_slices(
    output: &mut OutputMessage,
    game: &crate::game::Game,
    items: &Items,
    known: &mut HashSet<u32>,
    old_pos: Position,
    new_pos: Position,
) {
    if old_pos.y > new_pos.y {
        output.add_byte(0x65);
        get_map_description(
            output,
            game,
            items,
            old_pos.x as i32 - MAX_VIEWPORT_X,
            new_pos.y as i32 - MAX_VIEWPORT_Y,
            i32::from(new_pos.z),
            (MAX_VIEWPORT_X * 2) + 2,
            1,
            known,
        );
    } else if old_pos.y < new_pos.y {
        output.add_byte(0x67);
        get_map_description(
            output,
            game,
            items,
            old_pos.x as i32 - MAX_VIEWPORT_X,
            i32::from(new_pos.y) + (MAX_VIEWPORT_Y + 1),
            i32::from(new_pos.z),
            (MAX_VIEWPORT_X * 2) + 2,
            1,
            known,
        );
    }

    if old_pos.x < new_pos.x {
        output.add_byte(0x66);
        get_map_description(
            output,
            game,
            items,
            i32::from(new_pos.x) + (MAX_VIEWPORT_X + 1),
            new_pos.y as i32 - MAX_VIEWPORT_Y,
            i32::from(new_pos.z),
            1,
            (MAX_VIEWPORT_Y * 2) + 2,
            known,
        );
    } else if old_pos.x > new_pos.x {
        output.add_byte(0x68);
        get_map_description(
            output,
            game,
            items,
            new_pos.x as i32 - MAX_VIEWPORT_X,
            new_pos.y as i32 - MAX_VIEWPORT_Y,
            i32::from(new_pos.z),
            1,
            (MAX_VIEWPORT_Y * 2) + 2,
            known,
        );
    }
}

// ── Stat/skill helpers ────────────────────────────────────────────────────────

pub(crate) fn write_player_stats(output: &mut OutputMessage, player: &Player) {
    output.add_byte(0xA0);
    output.add_u16(player.base.health.min(0xFFFF) as u16);
    output.add_u16(player.base.health_max.min(0xFFFF) as u16);

    let free_cap = player.get_free_capacity();
    let free_cap_capped = if free_cap == u32::MAX { u32::MAX } else { free_cap };
    output.add_u32(free_cap_capped);

    if client_version().is_1098() {
        output.add_u32(free_cap_capped); // totalCapacity (same as free for now)
        output.add_u64(player.experience.min(i64::MAX as u64));
        output.add_u16(player.level as u16);
        output.add_byte(player.level_percent);

        // XP rate fields
        output.add_u16(100); // baseXpGain (100 = 1.0x)
        output.add_u16(0);   // voucherAddend
        output.add_u16(0);   // grindingAddend
        output.add_u16(0);   // storeBoostAddend
        output.add_u16(100); // huntingBoostFactor (100 = stamina 1.0x)

        output.add_u16(player.mana.min(0xFFFF) as u16);
        output.add_u16(player.get_max_mana().min(0xFFFF) as u16);
        output.add_byte(player.get_magic_level().min(0xFF) as u8);
        output.add_byte(player.get_base_magic_level().min(0xFF) as u8); // baseMagicLevel
        output.add_byte(player.mag_level_percent);
        output.add_byte(player.soul);
        output.add_u16(player.stamina_minutes);
        output.add_u16((player.base.base_speed / 2) as u16); // baseSpeed / 2

        // Condition regeneration seconds
        let regen_secs: u16 = player.base.conditions.iter()
            .filter(|c| c.get_type() == crate::combat::condition::ConditionType::Regeneration)
            .map(|c| (c.get_ticks() / 1000) as u16)
            .next()
            .unwrap_or(0);
        output.add_u16(regen_secs);

        output.add_u16(0); // offlineTrainingTime
        output.add_u16(0); // xpBoostRemainingTime
        output.add_byte(0); // xpBoostStoreBuy
    } else {
        output.add_u32(player.experience.min(0x7FFF_FFFF) as u32);
        output.add_u16(player.level as u16);
        output.add_byte(player.level_percent);
        output.add_u16(player.mana.min(0xFFFF) as u16);
        output.add_u16(player.get_max_mana().min(0xFFFF) as u16);
        output.add_byte(player.get_magic_level().min(0xFF) as u8);
        output.add_byte(player.mag_level_percent);
        output.add_byte(player.soul);
        output.add_u16(player.stamina_minutes);
    }
}

fn write_player_skills(output: &mut OutputMessage, player: &Player) {
    output.add_byte(0xA1);
    if client_version().is_1098() {
        for i in 0..SKILL_COUNT {
            output.add_u16(player.get_skill_level(i));
            output.add_u16(player.get_skill_level(i)); // base skill
            output.add_byte(player.get_skill_percent(i));
        }
        // Special skills (critical, life leech, mana leech — 6 pairs of u16 value + u16 base)
        for _ in 0..6 {
            output.add_u16(0); // value
            output.add_u16(0); // base
        }
    } else {
        for i in 0..SKILL_COUNT {
            output.add_byte(player.get_skill_level(i).min(0xFF) as u8);
            output.add_byte(player.get_skill_percent(i));
        }
    }
}

// ── Spectator broadcasting ───────────────────────────────────────────────────

fn sync_known_creatures(creature_id: CreatureId, known: &HashSet<u32>) {
    if let Some(session) = player_sessions().lock().unwrap().get(&creature_id) {
        *session.known_creatures.lock().unwrap() = known.clone();
    }
}

fn load_known_creatures_from_session(creature_id: CreatureId) -> Option<HashSet<u32>> {
    player_sessions().lock().unwrap().get(&creature_id)
        .map(|s| s.known_creatures.lock().unwrap().clone())
}

fn can_see_position(viewer_pos: Position, target: Position) -> bool {
    if viewer_pos.z <= 7 {
        if target.z > 7 {
            return false;
        }
    } else {
        if (viewer_pos.z as i32 - target.z as i32).unsigned_abs() > 2 {
            return false;
        }
    }
    let offsetz = viewer_pos.z as i32 - target.z as i32;
    let x = target.x as i32;
    let y = target.y as i32;
    let mx = viewer_pos.x as i32;
    let my = viewer_pos.y as i32;
    let max_vx = MAX_VIEWPORT_X;
    let max_vy = MAX_VIEWPORT_Y;
    x >= mx - max_vx + offsetz
        && x <= mx + (max_vx + 1) + offsetz
        && y >= my - max_vy + offsetz
        && y <= my + (max_vy + 1) + offsetz
}

fn finalize_and_send(output: &mut OutputMessage, round_keys: &RoundKeys, checksum_enabled: bool, conn: &ConnectionHandle) {
    if DBG_PACKETS.load(std::sync::atomic::Ordering::Relaxed) {
        let buf = output.get_output_buffer();
        let hex: String = buf.iter().map(|b| format!("{:02x}", b)).collect();
        tracing::info!(len = buf.len(), first_op = format!("0x{:02X}", buf.first().copied().unwrap_or(0)), hex, "DBG S2C frame");
    }
    output.write_message_length();
    output.xtea_encrypt(round_keys);
    output.add_crypto_header(checksum_enabled);
    conn.send_bytes(output.get_output_buffer().to_vec());
}

pub static DBG_PACKETS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn send_raw_to_player(creature_id: CreatureId, output: &mut OutputMessage) {
    let sessions = player_sessions().lock().unwrap();
    let Some(session) = sessions.get(&creature_id) else { return };
    finalize_and_send(output, &session.round_keys, session.checksum_enabled, &session.conn);
}

fn with_session<R>(creature_id: CreatureId, f: impl FnOnce(&PlayerSession) -> R) -> Option<R> {
    let sessions = player_sessions().lock().unwrap();
    sessions.get(&creature_id).map(f)
}

fn send_cancel_walk_to_player(creature_id: CreatureId) {
    let dir = {
        let game = g_game().lock().unwrap();
        game.get_player(creature_id)
            .map(|p| p.base.direction)
            .unwrap_or(Direction::South)
    };
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0xB5);
        output.add_byte(dir as u8);
    });
}

pub(crate) fn broadcast_creature_move(mover_id: CreatureId, old_pos: Position, new_pos: Position, old_stackpos: u8) {
    let mut game = g_game().lock().unwrap();
    let spectator_data: Vec<(CreatureId, Position)>;
    let new_stackpos: i32;
    {
        let old_specs = game.map.get_spectators(old_pos, true, true, 0, 0, 0, 0);
        let new_specs = game.map.get_spectators(new_pos, true, true, 0, 0, 0, 0);
        let mut seen = HashSet::new();
        let mut data = Vec::new();
        for id in old_specs.into_iter().chain(new_specs.into_iter()) {
            if id != mover_id && seen.insert(id) {
                if let Some(p) = game.get_player(id) {
                    data.push((id, p.base.position));
                }
            }
        }
        spectator_data = data;
        new_stackpos = game.map.get_tile(new_pos)
            .map(|t| t.get_client_index_of_creature(mover_id))
            .unwrap_or(-1);
    }

    if spectator_data.is_empty() {
        return;
    }

    let mut sessions = player_sessions().lock().unwrap();

    for (spec_id, spec_pos) in &spectator_data {
        let Some(session) = sessions.get_mut(spec_id) else { continue };

        let can_see_old = can_see_position(*spec_pos, old_pos);
        let can_see_new = can_see_position(*spec_pos, new_pos);

        if can_see_old && can_see_new {
            let teleport = old_pos.z != new_pos.z;
            let mut output = OutputMessage::new();
            if teleport {
                write_remove_tile_creature(&mut output, old_pos, old_stackpos, mover_id);
                if (0..10).contains(&new_stackpos) {
                    write_add_creature_packet(&mut output, mover_id, new_pos, new_stackpos as u8, &mut session.known_creatures.lock().unwrap(), &game);
                }
            } else {
                output.add_byte(0x6D);
                write_creature_movement(&mut output, old_pos, new_pos, old_stackpos, mover_id);
            }
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        } else if can_see_old {
            let mut output = OutputMessage::new();
            write_remove_tile_creature(&mut output, old_pos, old_stackpos, mover_id);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        } else if can_see_new && (0..10).contains(&new_stackpos) {
            let mut output = OutputMessage::new();
            write_add_creature_packet(&mut output, mover_id, new_pos, new_stackpos as u8, &mut session.known_creatures.lock().unwrap(), &game);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }
}

fn write_add_creature_packet(
    output: &mut OutputMessage,
    creature_id: CreatureId,
    pos: Position,
    stackpos: u8,
    known: &mut HashSet<u32>,
    game: &crate::game::Game,
) {
    output.add_byte(0x6A);
    output.add_position(pos.x, pos.y, pos.z);
    output.add_byte(stackpos);

    let (is_new, remove_id) = check_creature_as_known(known, creature_id);
    if let Some(creature) = game.get_creature(creature_id) {
        write_creature_data(output, creature, creature_id, is_new, remove_id);
    }
}

pub fn broadcast_creature_turn(creature_id: CreatureId, pos: Position, stackpos: i32, dir: Direction) {
    let spectator_ids: Vec<CreatureId>;
    {
        let mut game = g_game().lock().unwrap();
        spectator_ids = game.map.get_spectators(pos, true, true, 0, 0, 0, 0)
            .into_iter()
            .filter(|&id| id != creature_id)
            .collect();
    }

    if spectator_ids.is_empty() {
        return;
    }

    let sp = if stackpos >= 0 { stackpos as u8 } else { 0 };
    let sessions = player_sessions().lock().unwrap();
    for spec_id in spectator_ids {
        let Some(session) = sessions.get(&spec_id) else { continue };
        let mut output = OutputMessage::new();
        write_creature_turn(&mut output, pos, sp, creature_id, dir);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

pub fn broadcast_creature_say(_speaker_id: CreatureId, pos: Position, name: &str, level: u16, s2c_type: u8, text: &[u8]) {
    use crate::map::{MAX_VIEWPORT_X, MAX_VIEWPORT_Y};
    // TALKTYPE_YELL=3 and TALKTYPE_MONSTER_YELL=20 use expanded range.
    let is_yell = s2c_type == 3 || s2c_type == 20;
    let (wx, wy, multifloor) = if is_yell {
        (MAX_VIEWPORT_X * 2 + 2, MAX_VIEWPORT_Y * 2 + 2, true)
    } else {
        (MAX_VIEWPORT_X, MAX_VIEWPORT_Y, false)
    };
    let spectator_ids: Vec<CreatureId>;
    {
        let mut game = g_game().lock().unwrap();
        spectator_ids = game.map.get_spectators(pos, multifloor, true, wx, wx, wy, wy);
    }

    if spectator_ids.is_empty() {
        return;
    }

    // Bundle-aware: routes the caster's own echo into an open per-player bundle
    // (e.g. a spell cast's frame), other spectators get their own frame.
    for spec_id in spectator_ids {
        send_packet_to_player(spec_id, |output: &mut OutputMessage| {
            write_creature_say(output, name, level, s2c_type, pos, text);
        });
    }
}

/// Notify nearby NPCs about a player's speech, firing their `onCreatureSay` Lua event.
/// Mirrors C++ `Game::creatureSay` broadcasting to all spectators' `onCreatureSay`.
fn notify_nearby_npcs(player_id: CreatureId, pos: Position, speak_type: u8, text: &str) {
    use crate::map::{MAX_VIEWPORT_X, MAX_VIEWPORT_Y};
    let npc_ids: Vec<CreatureId> = {
        let mut game = g_game().lock().unwrap();
        game.map.get_spectators(pos, false, false, MAX_VIEWPORT_X, MAX_VIEWPORT_X, MAX_VIEWPORT_Y, MAX_VIEWPORT_Y)
    };
    for npc_id in npc_ids {
        let is_npc = g_game().lock().unwrap().get_creature(npc_id).map(|c| c.is_npc()).unwrap_or(false);
        if is_npc && npc_id != player_id {
            crate::creatures::npc::fire_npc_creature_say(npc_id, player_id, speak_type, text);
        }
    }
}

fn broadcast_private_message(sender_id: CreatureId, receiver_name: &[u8], sender_name: &str, level: u16, s2c_type: u8, text: &[u8]) {
    let receiver_str = String::from_utf8_lossy(receiver_name);
    let target_id = {
        let game = g_game().lock().unwrap();
        game.get_player_id_by_name(&receiver_str)
    };
    let Some(target_id) = target_id else {
        send_status_message_to_player(sender_id, "A player with this name is not online.");
        return;
    };

    let sessions = player_sessions().lock().unwrap();
    if let Some(session) = sessions.get(&target_id) {
        let mut output = OutputMessage::new();
        write_private_message(&mut output, sender_name, level, s2c_type, text);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
    drop(sessions);

    let display_name = {
        let game = g_game().lock().unwrap();
        game.get_player(target_id).map(|p| p.name.clone()).unwrap_or_else(|| receiver_str.to_string())
    };
    let confirm_msg = format!("Message sent to {}.", display_name);
    send_status_message_to_player(sender_id, &confirm_msg);
}

fn broadcast_whisper(speaker_id: CreatureId, pos: Position, name: &str, level: u16, text: &[u8]) {
    use crate::map::{MAX_VIEWPORT_X, MAX_VIEWPORT_Y};
    let spectator_ids: Vec<CreatureId> = {
        let mut game = g_game().lock().unwrap();
        game.map.get_spectators(pos, false, true, MAX_VIEWPORT_X, MAX_VIEWPORT_X, MAX_VIEWPORT_Y, MAX_VIEWPORT_Y)
    };
    let pspsps = b"pspsps";
    for spec_id in spectator_ids {
        let spec_pos = {
            let game = g_game().lock().unwrap();
            game.get_creature(spec_id).map(|c| c.position())
        };
        let Some(sp) = spec_pos else { continue };
        let adjacent = (pos.x as i32 - sp.x as i32).abs() <= 1
            && (pos.y as i32 - sp.y as i32).abs() <= 1
            && pos.z == sp.z;
        let msg: &[u8] = if adjacent { text } else { pspsps.as_slice() };
        send_packet_to_player(spec_id, |output: &mut OutputMessage| {
            write_creature_say(output, name, level, 2, pos, msg);
        });
    }
    let _ = speaker_id;
}

fn broadcast_to_all_players(sender_name: &str, level: u16, speak_type: u8, text: &[u8]) {
    let sessions = player_sessions().lock().unwrap();
    for (_id, session) in sessions.iter() {
        let mut output = OutputMessage::new();
        write_private_message(&mut output, sender_name, level, speak_type, text);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

pub(crate) fn broadcast_channel_message(channel_id: u16, name: &str, level: u16, s2c_type: u8, text: &[u8]) {
    // Collect the creature IDs of channel members.
    let member_ids: Vec<CreatureId> = {
        let chat = crate::chat::g_chat().lock().unwrap();
        if let Some(ch) = chat.get_channel_by_id(channel_id) {
            ch.get_user_ids().collect()
        } else {
            // Unknown channel — broadcast to everyone (public).
            let sessions = player_sessions().lock().unwrap();
            let ids: Vec<CreatureId> = sessions.keys().copied().collect();
            drop(sessions);
            ids
        }
    };

    let sessions = player_sessions().lock().unwrap();
    for id in &member_ids {
        if let Some(session) = sessions.get(id) {
            let mut output = OutputMessage::new();
            write_channel_message(&mut output, channel_id, name, level, s2c_type, text);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }
}

pub(crate) fn broadcast_creature_appear(creature_id: CreatureId, pos: Position) {
    let mut game = g_game().lock().unwrap();
    let spectator_ids: Vec<CreatureId> = game.map.get_spectators(pos, true, true, 0, 0, 0, 0)
        .into_iter()
        .filter(|&id| id != creature_id)
        .collect();

    if spectator_ids.is_empty() {
        return;
    }

    let stackpos = game.map.get_tile(pos)
        .map(|t| t.get_client_index_of_creature(creature_id))
        .unwrap_or(-1);
    if !(0..10).contains(&stackpos) {
        return;
    }

    let mut sessions = player_sessions().lock().unwrap();
    for spec_id in spectator_ids {
        let Some(session) = sessions.get_mut(&spec_id) else { continue };
        let mut output = OutputMessage::new();
        write_add_creature_packet(&mut output, creature_id, pos, stackpos as u8, &mut session.known_creatures.lock().unwrap(), &game);
        write_magic_effect(&mut output, pos, 0x0A); // CONST_ME_TELEPORT
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

fn broadcast_creature_disappear(creature_id: CreatureId, pos: Position, stackpos: u8) {
    let spectator_ids: Vec<CreatureId>;
    {
        let mut game = g_game().lock().unwrap();
        spectator_ids = game.map.get_spectators(pos, true, true, 0, 0, 0, 0)
            .into_iter()
            .filter(|&id| id != creature_id)
            .collect();
    }

    if spectator_ids.is_empty() {
        return;
    }

    let sessions = player_sessions().lock().unwrap();
    for spec_id in spectator_ids {
        let Some(session) = sessions.get(&spec_id) else { continue };
        let mut output = OutputMessage::new();
        write_remove_tile_creature(&mut output, pos, stackpos, creature_id);
        write_magic_effect(&mut output, pos, 0x02); // CONST_ME_POFF
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

fn broadcast_creature_teleport(creature_id: CreatureId, old_pos: Position, old_stackpos: u8, new_pos: Position) {
    let mut game = g_game().lock().unwrap();
    let spectator_data: Vec<(CreatureId, Position)>;
    let new_stackpos: i32;
    {
        let old_specs = game.map.get_spectators(old_pos, true, true, 0, 0, 0, 0);
        let new_specs = game.map.get_spectators(new_pos, true, true, 0, 0, 0, 0);
        let mut seen = HashSet::new();
        let mut data = Vec::new();
        for id in old_specs.into_iter().chain(new_specs.into_iter()) {
            if id != creature_id && seen.insert(id) {
                if let Some(p) = game.get_player(id) {
                    data.push((id, p.base.position));
                }
            }
        }
        spectator_data = data;
        new_stackpos = game.map.get_tile(new_pos)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1);
    }

    if spectator_data.is_empty() {
        return;
    }

    let mut sessions = player_sessions().lock().unwrap();

    for (spec_id, spec_pos) in &spectator_data {
        let Some(session) = sessions.get_mut(spec_id) else { continue };

        let can_see_old = can_see_position(*spec_pos, old_pos);
        let can_see_new = can_see_position(*spec_pos, new_pos);

        if can_see_old && can_see_new {
            let mut output = OutputMessage::new();
            write_remove_tile_creature(&mut output, old_pos, old_stackpos, creature_id);
            if (0..10).contains(&new_stackpos) {
                write_add_creature_packet(&mut output, creature_id, new_pos, new_stackpos as u8, &mut session.known_creatures.lock().unwrap(), &game);
            }
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        } else if can_see_old {
            let mut output = OutputMessage::new();
            write_remove_tile_creature(&mut output, old_pos, old_stackpos, creature_id);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        } else if can_see_new && (0..10).contains(&new_stackpos) {
            let mut output = OutputMessage::new();
            write_add_creature_packet(&mut output, creature_id, new_pos, new_stackpos as u8, &mut session.known_creatures.lock().unwrap(), &game);
            write_magic_effect(&mut output, new_pos, 0x0A);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }
}

// ── Movement helpers ──────────────────────────────────────────────────────────

struct BedUseInfo {
    house_id: Option<u32>,
    is_pz: bool,
    sleeper_guid: u32,
    transform_to_free: u16,
    transform_male: u16,
    transform_female: u16,
    partner_dir: u8,
    p_guid: u32,
    p_sex: crate::creatures::player::PlayerSex,
    premium: bool,
    house_owner: u32,
    my_access: crate::map::houses::AccessHouseLevel,
}

/// Offset a position by a direction (no floor-change resolution). Port of
/// `getNextPosition(Direction, Position&)` from tools.cpp.
pub(crate) fn next_position(dir: u8, pos: Position) -> Position {
    let (dx, dy): (i32, i32) = match dir {
        0 => (0, -1),  // north
        1 => (1, 0),   // east
        2 => (0, 1),   // south
        3 => (-1, 0),  // west
        4 => (-1, 1),  // southwest
        5 => (1, 1),   // southeast
        6 => (-1, -1), // northwest
        7 => (1, -1),  // northeast
        _ => (0, 0),
    };
    Position {
        x: pos.x.wrapping_add(dx as u16),
        y: pos.y.wrapping_add(dy as u16),
        z: pos.z,
    }
}

fn step_position(map: &Map, pos: Position, dir: Direction) -> Option<Position> {
    let (dx, dy): (i32, i32) = match dir {
        Direction::North => (0, -1),
        Direction::South => (0, 1),
        Direction::East => (1, 0),
        Direction::West => (-1, 0),
        Direction::NorthEast => (1, -1),
        Direction::SouthEast => (1, 1),
        Direction::SouthWest => (-1, 1),
        Direction::NorthWest => (-1, -1),
    };
    let stepped = Position {
        x: pos.x.wrapping_add(dx as u16),
        y: pos.y.wrapping_add(dy as u16),
        z: pos.z,
    };
    map.resolve_floor_change_destination(stepped)
}

/// Notify all online players who have `changed_guid` in their VIP list.
/// `online=true` sends 0xD3 (VIPSTATUS_ONLINE), `online=false` sends 0xD4 (VIPSTATUS_OFFLINE).
fn broadcast_vip_status(changed_guid: u32, online: bool) {
    let observer_ids: Vec<CreatureId> = {
        let game = g_game().lock().unwrap();
        game.get_players_online()
            .filter(|(_, p)| p.guid != changed_guid && p.vip_list.contains(&changed_guid))
            .map(|(id, _)| *id)
            .collect()
    };
    if observer_ids.is_empty() {
        return;
    }
    let sessions = player_sessions().lock().unwrap();
    for id in &observer_ids {
        let Some(session) = sessions.get(id) else { continue };
        let mut output = OutputMessage::new();
        output.add_byte(if online { 0xD3 } else { 0xD4 });
        output.add_u32(changed_guid);
        finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
    }
}

/// 0x9F: sendBasicData — premium status, vocation, known spells (10.98 only)
fn write_basic_data(output: &mut OutputMessage, player: &Player) {
    output.add_byte(0x9F);
    output.add_byte(if player.is_premium() { 1 } else { 0 });
    output.add_u32(0); // premium time (unix timestamp, 0=none)
    output.add_byte(player.vocation_id as u8);

    // Spell list — send all known spell IDs (C++ iterates player->learnedInstantSpellList)
    // For now, send 0 spells — Lua self-registers them on the client side.
    output.add_u16(0); // spell count
}

/// Broadcast a category-changing item transform (down<->top) as a remove of
/// the old stack slot followed by an add of the new item at its new slot,
/// mirroring C++ `Game::transformItem` when `alwaysOnTop` changes. Sending an
/// in-place 0x6B here would leave the client's stack order disagreeing with the
/// server (e.g. an opened door becoming always-on-top), corrupting stackpos.
pub fn broadcast_tile_item_repartition(
    pos: Position,
    old_stackpos: u8,
    new_stackpos: u8,
    new_server_id: u16,
    count: u8,
    items: &Items,
) {
    if items.get_item_type(new_server_id as usize).client_id == 0 {
        return;
    }
    let spectator_ids: Vec<CreatureId> = {
        let mut game = g_game().lock().unwrap();
        game.map.get_spectators(pos, true, true, 0, 0, 0, 0)
    };
    for spec_id in spectator_ids {
        let items_ref = items;
        send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
            write_remove_tile_thing(output, pos, old_stackpos);
            output.add_byte(0x6A); // AddTileItem
            output.add_position(pos.x, pos.y, pos.z);
            output.add_byte(new_stackpos);
            write_item(output, items_ref, new_server_id, count);
        });
    }
}

pub fn broadcast_tile_item_transform(pos: Position, stackpos: u8, new_server_id: u16, count: u8, items: &Items) {
    let spectator_ids: Vec<CreatureId> = {
        let mut game = g_game().lock().unwrap();
        game.map.get_spectators(pos, true, true, 0, 0, 0, 0)
    };
    let cid = items.get_item_type(new_server_id as usize).client_id;
    tracing::info!(?pos, stackpos, new_server_id, client_id = cid, n_spec = spectator_ids.len(), "DBG transform 0x6B");
    if items.get_item_type(new_server_id as usize).client_id == 0 {
        return;
    }
    for spec_id in spectator_ids {
        let items_ref = items;
        send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
            output.add_byte(0x6B); // UpdateTileItem
            output.add_position(pos.x, pos.y, pos.z);
            output.add_byte(stackpos);
            write_item(output, items_ref, new_server_id, count);
        });
    }
}

// ── Send helper functions (S2C packets) ──────────────────────────────────────

fn write_add_tile_item(output: &mut OutputMessage, pos: Position, stackpos: u8, item: &crate::map::tile::MapItem, items: &Items) {
    output.add_byte(0x6A);
    output.add_position(pos.x, pos.y, pos.z);
    output.add_byte(stackpos);
    write_item(output, items, item.server_id, item.count.min(255) as u8);
}

fn write_update_tile_item(output: &mut OutputMessage, pos: Position, stackpos: u8, item: &crate::map::tile::MapItem, items: &Items) {
    output.add_byte(0x6B);
    output.add_position(pos.x, pos.y, pos.z);
    output.add_byte(stackpos);
    write_item(output, items, item.server_id, item.count.min(255) as u8);
}

fn write_remove_tile_thing(output: &mut OutputMessage, pos: Position, stackpos: u8) {
    if stackpos >= 10 {
        return;
    }
    output.add_byte(0x6C);
    output.add_position(pos.x, pos.y, pos.z);
    output.add_byte(stackpos);
}

fn handle_ground_to_inventory(creature_id: CreatureId, from_pos: Position, to_pos: Position, from_stackpos: u8, sprite_id: u16, _proto: &mut ProtocolGame, _conn: &ConnectionHandle) {
    // to_pos.y is the inventory slot (1-10).
    let slot = to_pos.y as usize;
    if !(crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST).contains(&slot) {
        return;
    }

    let (item, item_idx, player_pos) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let ppos = player.base.position;

        let dx = (ppos.x as i32 - from_pos.x as i32).unsigned_abs();
        let dy = (ppos.y as i32 - from_pos.y as i32).unsigned_abs();
        if dx > 1 || dy > 1 || ppos.z != from_pos.z { return; }

        let Some(tile) = game.map.get_tile(from_pos) else { return };
        let ground_off: usize = if tile.ground.is_some() { 1 } else { 0 };
        let idx_in_tile = (from_stackpos as usize).saturating_sub(ground_off + tile.creature_ids.len());
        let Some(item) = tile.items.get(idx_in_tile) else { return };
        let it = game.items.get_item_type(item.server_id as usize);
        if it.client_id != sprite_id { return; }
        if !it.moveable { return; }
        (item.clone(), idx_in_tile, ppos)
    };

    let server_id = item.server_id;
    let client_id = {
        let game = g_game().lock().unwrap();
        game.items.get_item_type(server_id as usize).client_id
    };

    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.inventory[slot] = Some(server_id);
            player.inventory_count[slot] = item.count.max(1);
            // Preserve the full item (attributes + container tree) for the slot
            // so its attributes persist on save.
            player.inventory_items[slot] = Some(item.clone());
        }
        if let Some(tile) = game.map.get_tile_mut(from_pos) {
            if item_idx < tile.items.len() {
                tile.items.remove(item_idx);
            }
        }
    }

    // Tell all spectators about the tile change (remove).
    let spectators = {
        let mut game = g_game().lock().unwrap();
        game.map.get_spectators(from_pos, true, true, 0, 0, 0, 0)
    };
    for spec_id in spectators {
        send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
            write_remove_tile_thing(output, from_pos, from_stackpos);
        });
    }

    // Fire onEquip movement event.
    crate::events::dispatch::execute_equip_event(
        creature_id,
        server_id,
        from_pos,
        item_idx as i32,
        item.action_id,
        item.unique_id,
        1u32 << (slot.saturating_sub(1)),
        true,
        false,
    );

    // Tell the player their inventory changed.
    let s = slot as u8;
    let cid = client_id;
    let item_count = item.count.max(1) as u8;
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0x78);
        output.add_byte(s);
        output.add_u16(cid);
        output.add_byte(item_count);
    });

    let _ = player_pos;
}

fn handle_inventory_to_ground(creature_id: CreatureId, from_pos: Position, to_pos: Position, _from_stackpos: u8, _proto: &mut ProtocolGame, _conn: &ConnectionHandle) {
    let slot = from_pos.y as usize;
    if !(crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST).contains(&slot) {
        return;
    }

    let (server_id, player_pos, dropped_tree) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let Some(sid) = player.inventory[slot] else { return };
        let tree = player.inventory_items.get(slot).and_then(|o| o.clone());
        (sid, player.base.position, tree)
    };

    let dx = (player_pos.x as i32 - to_pos.x as i32).unsigned_abs();
    let dy = (player_pos.y as i32 - to_pos.y as i32).unsigned_abs();
    if dx > 7 || dy > 5 || player_pos.z != to_pos.z { return; }

    let (stackpos, client_id) = {
        let mut game = g_game().lock().unwrap();
        let items_arc = game.items.clone();
        let Some(tile) = game.map.get_tile_mut(to_pos) else { return };
        if tile.ground.is_none() { return; }
        // Carry the full item (container children) onto the ground when present.
        let item = dropped_tree.clone()
            .unwrap_or_else(|| crate::map::tile::MapItem { server_id, ..crate::map::tile::MapItem::default() });
        tile.internal_add_item(item, &items_arc);
        let sp = (if tile.ground.is_some() { 1u8 } else { 0u8 })
            .saturating_add(tile.get_top_item_count().saturating_sub(1) as u8);
        let cid = items_arc.get_item_type(server_id as usize).client_id;
        (sp, cid)
    };

    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.inventory[slot] = None;
            if slot < player.inventory_items.len() {
                player.inventory_items[slot] = None;
            }
        }
    }

    // Fire onDeEquip movement event.
    crate::events::dispatch::execute_equip_event(
        creature_id,
        server_id,
        to_pos,
        0,
        0,
        0,
        1u32 << (slot.saturating_sub(1)),
        false,
        false,
    );

    let s = slot as u8;
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0x79);
        output.add_byte(s);
    });

    let spectators = {
        let mut game = g_game().lock().unwrap();
        game.map.get_spectators(to_pos, true, true, 0, 0, 0, 0)
    };
    for spec_id in spectators {
        send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
            output.add_byte(0x6A);
            output.add_position(to_pos.x, to_pos.y, to_pos.z);
            output.add_byte(stackpos);
            output.add_u16(client_id);
            output.add_byte(1);
        });
    }
}

fn handle_inventory_to_inventory(creature_id: CreatureId, from_pos: Position, to_pos: Position, _proto: &mut ProtocolGame, _conn: &ConnectionHandle) {
    let from_slot = from_pos.y as usize;
    let to_slot = to_pos.y as usize;
    if !(crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST).contains(&from_slot) { return; }
    if !(crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST).contains(&to_slot) { return; }
    if from_slot == to_slot { return; }

    let (from_id, to_id) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        (player.inventory[from_slot], player.inventory[to_slot])
    };

    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(creature_id) {
            player.inventory.swap(from_slot, to_slot);
            player.inventory_count.swap(from_slot, to_slot);
            player.inventory_items.swap(from_slot, to_slot);
        }
    }

    let items_arc = g_game().lock().unwrap().items.clone();
    let fs = from_slot as u8;
    let ts = to_slot as u8;
    let from_cid = from_id.map(|id| items_arc.get_item_type(id as usize).client_id);
    let to_cid = to_id.map(|id| items_arc.get_item_type(id as usize).client_id);

    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        match to_cid {
            Some(cid) => { output.add_byte(0x78); output.add_byte(fs); output.add_u16(cid); output.add_byte(1); }
            None => { output.add_byte(0x79); output.add_byte(fs); }
        }
        match from_cid {
            Some(cid) => { output.add_byte(0x78); output.add_byte(ts); output.add_u16(cid); output.add_byte(1); }
            None => { output.add_byte(0x79); output.add_byte(ts); }
        }
    });
}

#[allow(dead_code)]
fn write_creature_health(output: &mut OutputMessage, creature_id: u32, health_percent: u8) {
    output.add_byte(0x8C);
    output.add_u32(creature_id);
    output.add_byte(health_percent);
}

fn write_creature_turn(output: &mut OutputMessage, pos: Position, stackpos: u8, creature_id: u32, direction: Direction) {
    if stackpos >= 10 {
        output.add_byte(0x6B);
        output.add_u16(0xFFFF);
        output.add_u32(creature_id);
    } else {
        output.add_byte(0x6B);
        output.add_position(pos.x, pos.y, pos.z);
        output.add_byte(stackpos);
    }
    output.add_u16(0x63);
    output.add_u32(creature_id);
    output.add_byte(direction as u8);
    if client_version().is_1098() {
        output.add_byte(0x00); // walkthrough
    }
}

fn write_creature_say(output: &mut OutputMessage, name: &str, level: u16, speak_type: u8, pos: Position, text: &[u8]) {
    output.add_byte(0xAA);
    output.add_u32(0x00); // statement id
    output.add_string(name.as_bytes());
    output.add_u16(level);
    output.add_byte(translate_speak_class_to_client(speak_type));
    output.add_position(pos.x, pos.y, pos.z);
    output.add_string(text);
}

fn write_creature_outfit(output: &mut OutputMessage, creature_id: u32, outfit: &Outfit) {
    output.add_byte(0x8E);
    output.add_u32(creature_id);
    add_outfit(output, outfit);
}

#[allow(dead_code)]
fn write_creature_walkthrough(output: &mut OutputMessage, creature_id: u32, passable: bool) {
    output.add_byte(0x92);
    output.add_u32(creature_id);
    output.add_byte(if passable { 0x00 } else { 0x01 });
}

#[allow(dead_code)]
fn write_creature_shield(output: &mut OutputMessage, creature_id: u32, party_shield: u8) {
    output.add_byte(0x91);
    output.add_u32(creature_id);
    output.add_byte(party_shield);
}

#[allow(dead_code)]
fn write_creature_skull(output: &mut OutputMessage, creature_id: u32, skull: Skull) {
    output.add_byte(0x90);
    output.add_u32(creature_id);
    output.add_byte(skull as u8);
}

#[allow(dead_code)]
fn write_creature_light(output: &mut OutputMessage, creature_id: u32, level: u8, color: u8) {
    output.add_byte(0x8D);
    output.add_u32(creature_id);
    output.add_byte(level);
    output.add_byte(color);
}

#[allow(dead_code)]
fn write_change_speed(output: &mut OutputMessage, creature_id: u32, base_speed: u16, speed: u16) {
    output.add_byte(0x8F);
    output.add_u32(creature_id);
    if client_version().is_1098() {
        output.add_u16(base_speed / 2);
        output.add_u16(speed / 2);
    } else {
        output.add_u16(speed);
    }
}

fn write_icons(output: &mut OutputMessage, icons: u16) {
    output.add_byte(0xA2);
    output.add_u16(icons);
}

#[allow(dead_code)]
fn write_world_light(output: &mut OutputMessage, level: u8, color: u8) {
    output.add_byte(0x82);
    output.add_byte(level);
    output.add_byte(color);
}

#[allow(dead_code)]
fn write_text_message(output: &mut OutputMessage, msg_type: u8, text: &[u8]) {
    output.add_byte(0xB4);
    output.add_byte(translate_message_class_to_client(msg_type));
    output.add_string(text);
}

#[allow(dead_code)]
fn write_inventory_item(output: &mut OutputMessage, slot: u8, item: Option<&crate::map::tile::MapItem>, items: &Items) {
    match item {
        Some(it) => {
            output.add_byte(0x78);
            output.add_byte(slot);
            write_item(output, items, it.server_id, it.count.min(255) as u8);
        }
        None => {
            output.add_byte(0x79);
            output.add_byte(slot);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn write_container(output: &mut OutputMessage, cid: u8, container_item: &crate::map::tile::MapItem, items: &Items, name: &str, capacity: u8, has_parent: bool, contents: &[crate::map::tile::MapItem]) {
    output.add_byte(0x6E);
    output.add_byte(cid);
    write_item(output, items, container_item.server_id, container_item.count.min(255) as u8);
    output.add_string(name.as_bytes());
    output.add_byte(capacity);
    output.add_byte(if has_parent { 0x01 } else { 0x00 });
    let count = contents.len().min(255);
    output.add_byte(count as u8);
    for item in contents.iter().take(count) {
        write_item(output, items, item.server_id, item.count.min(255) as u8);
    }
}

#[allow(dead_code)]
fn write_add_container_item(output: &mut OutputMessage, cid: u8, item: &crate::map::tile::MapItem, items: &Items) {
    output.add_byte(0x70);
    output.add_byte(cid);
    write_item(output, items, item.server_id, item.count.min(255) as u8);
}

#[allow(dead_code)]
fn write_update_container_item(output: &mut OutputMessage, cid: u8, slot: u8, item: &crate::map::tile::MapItem, items: &Items) {
    output.add_byte(0x71);
    output.add_byte(cid);
    output.add_byte(slot);
    write_item(output, items, item.server_id, item.count.min(255) as u8);
}

#[allow(dead_code)]
fn write_remove_container_item(output: &mut OutputMessage, cid: u8, slot: u8) {
    output.add_byte(0x72);
    output.add_byte(cid);
    output.add_byte(slot);
}

fn write_close_container(output: &mut OutputMessage, cid: u8) {
    output.add_byte(0x6F);
    output.add_byte(cid);
}

#[allow(dead_code)]
fn write_distance_shoot(output: &mut OutputMessage, from: Position, to: Position, effect_type: u8) {
    output.add_byte(0x85);
    output.add_position(from.x, from.y, from.z);
    output.add_position(to.x, to.y, to.z);
    output.add_byte(effect_type);
}

fn write_magic_effect(output: &mut OutputMessage, pos: Position, effect_type: u8) {
    output.add_byte(0x83);
    output.add_position(pos.x, pos.y, pos.z);
    output.add_byte(effect_type);
}

fn write_animated_text(output: &mut OutputMessage, pos: Position, color: u8, text: &[u8]) {
    output.add_byte(0x84);
    output.add_position(pos.x, pos.y, pos.z);
    output.add_byte(color);
    output.add_string(text);
}

pub(crate) fn broadcast_animated_text(pos: Position, color: u8, text: Vec<u8>) {
    broadcast_effect_to_spectators(pos, move |output: &mut OutputMessage| {
        write_animated_text(output, pos, color, &text);
    });
}

#[allow(dead_code)]
fn write_cancel_target(output: &mut OutputMessage) {
    output.add_byte(0xA3);
    output.add_u32(0x00000000);
}

#[allow(dead_code)]
fn write_fight_modes(output: &mut OutputMessage, fight_mode: u8, chase_mode: u8, secure_mode: u8) {
    output.add_byte(0xA7);
    output.add_byte(fight_mode);
    output.add_byte(chase_mode);
    output.add_byte(secure_mode);
}

#[allow(dead_code)]
fn write_fyi_box(output: &mut OutputMessage, text: &[u8]) {
    output.add_byte(0x15);
    output.add_string(text);
}

#[allow(dead_code)]
fn write_cancel_walk(output: &mut OutputMessage, direction: Direction) {
    output.add_byte(0xB5);
    output.add_byte(direction as u8);
}

#[allow(dead_code)]
fn write_relogin_window(output: &mut OutputMessage) {
    output.add_byte(0x28);
}

fn add_outfit(output: &mut OutputMessage, outfit: &Outfit) {
    write_outfit(output, outfit);
}

// ── Trade packets ────────────────────────────────────────────────────────────

#[allow(dead_code)]
fn write_trade_item_request(output: &mut OutputMessage, name: &str, items_list: &[(crate::map::tile::MapItem, Items)], _first_time: bool) {
    output.add_byte(0x7D);
    output.add_string(name.as_bytes());
    output.add_byte(items_list.len().min(255) as u8);
}

#[allow(dead_code)]
fn write_close_trade(output: &mut OutputMessage) {
    output.add_byte(0x7F);
}

// ── Inventory update helpers ──────────────────────────────────────────────────

/// Send 0x78 (set inventory slot) for a given slot and item.
pub(crate) fn send_inventory_item_to_player(player_id: u32, slot: u8, server_id: u16, count: u16) {
    let game = g_game().lock().unwrap();
    let client_id = game.items.get_item_type(server_id as usize).client_id;
    let it_stackable = game.items.get_item_type(server_id as usize).stackable;
    drop(game);
    send_packet_to_player(player_id, move |output: &mut OutputMessage| {
        output.add_byte(0x78);
        output.add_byte(slot);
        output.add_u16(client_id);
        if it_stackable {
            output.add_byte(count.min(100) as u8);
        } else {
            output.add_byte(1);
        }
    });
}

/// Send 0x79 (clear inventory slot).
pub(crate) fn send_clear_inventory_slot(player_id: u32, slot: u8) {
    send_packet_to_player(player_id, move |output: &mut OutputMessage| {
        output.add_byte(0x79);
        output.add_byte(slot);
    });
}

/// Re-send all equipment slots (0x78 filled / 0x79 empty) for a player.
fn send_full_inventory(creature_id: CreatureId) {
    use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST};
    let slots: Vec<(u8, Option<(u16, u16)>)> = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        (CONST_SLOT_FIRST..=CONST_SLOT_LAST)
            .map(|s| (s as u8, player.inventory[s].map(|sid| (sid, player.inventory_count[s]))))
            .collect()
    };
    for (slot, entry) in slots {
        match entry {
            Some((sid, count)) => send_inventory_item_to_player(creature_id, slot, sid, count.max(1)),
            None => send_clear_inventory_slot(creature_id, slot),
        }
    }
}

// ── Shop packets ─────────────────────────────────────────────────────────────

pub(crate) fn send_shop_to_player(player_id: u32, items: &[crate::creatures::player::ShopInfo]) {
    let game = g_game().lock().unwrap();
    let items_ref: Vec<_> = items.iter().map(|s| {
        let it = game.items.get_item_id_by_client_id(0); // placeholder
        let _ = it;
        s.clone()
    }).collect();
    drop(game);
    let items_owned = items_ref;
    send_packet_to_player(player_id, move |output: &mut OutputMessage| {
        let game = g_game().lock().unwrap();
        output.add_byte(0x7A);
        let to_send = items_owned.len().min(u8::MAX as usize) as u8;
        output.add_byte(to_send);
        for item in items_owned.iter().take(to_send as usize) {
            let it = game.items.get_item_type(item.item_id as usize);
            output.add_u16(it.client_id);
            if it.is_fluid_container() || it.is_splash() {
                output.add_byte(crate::util::server_fluid_to_client(item.sub_type as u8));
            } else {
                output.add_byte(0x00);
            }
            output.add_string(item.real_name.as_bytes());
            output.add_u32(it.weight as u32);
            output.add_u32(item.buy_price);
            output.add_u32(item.sell_price);
        }
    });
}

pub(crate) fn send_sale_item_list(player_id: u32, shop_items: &[crate::creatures::player::ShopInfo]) {
    let game = g_game().lock().unwrap();
    let money = game.get_player(player_id).map(|p| p.get_money()).unwrap_or(0);
    // Build sale map: item_id -> count_available
    let mut sale_map: std::collections::BTreeMap<u16, u32> = std::collections::BTreeMap::new();
    if let Some(player) = game.get_player(player_id) {
        for si in shop_items {
            if si.sell_price == 0 { continue; }
            let it = game.items.get_item_type(si.item_id as usize);
            let sub_type = if it.has_sub_type() && !it.stackable {
                if si.sub_type == 0 { -1i32 } else { si.sub_type }
            } else { -1i32 };
            let count = player.count_items_of_type(si.item_id as u16, sub_type);
            if count > 0 {
                sale_map.insert(si.item_id as u16, count);
            }
        }
    }
    drop(game);
    let sale_entries: Vec<(u16, u32)> = sale_map.into_iter().collect();
    send_packet_to_player(player_id, move |output: &mut OutputMessage| {
        output.add_byte(0x7B);
        output.add_u32(money as u32);
        let to_send = sale_entries.len().min(u8::MAX as usize) as u8;
        output.add_byte(to_send);
        let game = g_game().lock().unwrap();
        for (id, count) in sale_entries.iter().take(to_send as usize) {
            let it = game.items.get_item_type(*id as usize);
            output.add_u16(it.client_id);
            output.add_byte((*count).min(u8::MAX as u32) as u8);
        }
    });
}

pub(crate) fn send_close_shop(player_id: u32) {
    send_packet_to_player(player_id, |output: &mut OutputMessage| {
        output.add_byte(0x7C);
    });
}

// ── VIP packets ──────────────────────────────────────────────────────────────

fn write_vip(output: &mut OutputMessage, guid: u32, name: &str, online: bool) {
    output.add_byte(0xD2);
    output.add_u32(guid);
    output.add_string(name.as_bytes());
    if client_version().is_1098() {
        output.add_string(b""); // description
        output.add_u32(0);      // icon
        output.add_byte(0x00);  // notify
    }
    output.add_byte(if online { 0x01 } else { 0x00 });
}

#[allow(dead_code)]
fn write_updated_vip_status(output: &mut OutputMessage, guid: u32, online: bool) {
    output.add_byte(if online { 0xD3 } else { 0xD4 });
    output.add_u32(guid);
}

// ── Quest packets ────────────────────────────────────────────────────────────

#[allow(dead_code)]
fn write_quest_log(output: &mut OutputMessage, quests: &[(u16, &str, bool)]) {
    output.add_byte(0xF0);
    output.add_u16(quests.len() as u16);
    for &(id, name, completed) in quests {
        output.add_u16(id);
        output.add_string(name.as_bytes());
        output.add_byte(if completed { 0x01 } else { 0x00 });
    }
}

#[allow(dead_code)]
fn write_quest_line(output: &mut OutputMessage, quest_id: u16, missions: &[(String, String)]) {
    output.add_byte(0xF1);
    output.add_u16(quest_id);
    output.add_byte(missions.len().min(255) as u8);
    for (name, desc) in missions {
        output.add_string(name.as_bytes());
        output.add_string(desc.as_bytes());
    }
}

// ── Tutorial/map mark ────────────────────────────────────────────────────────

#[allow(dead_code)]
fn write_tutorial(output: &mut OutputMessage, tutorial_id: u8) {
    output.add_byte(0xDC);
    output.add_byte(tutorial_id);
}

#[allow(dead_code)]
fn write_add_marker(output: &mut OutputMessage, pos: Position, mark_type: u8, description: &str) {
    output.add_byte(0xDD);
    output.add_position(pos.x, pos.y, pos.z);
    output.add_byte(mark_type);
    output.add_string(description.as_bytes());
}

// ── Channel packets ──────────────────────────────────────────────────────────

#[allow(dead_code)]
fn write_channel(output: &mut OutputMessage, channel_id: u16, channel_name: &str) {
    output.add_byte(0xAC);
    output.add_u16(channel_id);
    output.add_string(channel_name.as_bytes());
}

fn write_channel_message(output: &mut OutputMessage, channel_id: u16, name: &str, level: u16, speak_type: u8, text: &[u8]) {
    output.add_byte(0xAA);
    output.add_u32(0x00);
    output.add_string(name.as_bytes());
    output.add_u16(level);
    output.add_byte(speak_type);
    output.add_u16(channel_id);
    output.add_string(text);
}

#[allow(dead_code)]
fn write_open_private_channel(output: &mut OutputMessage, name: &str) {
    output.add_byte(0xAD);
    output.add_string(name.as_bytes());
}

#[allow(dead_code)]
fn write_create_private_channel(output: &mut OutputMessage, channel_id: u16, channel_name: &str) {
    output.add_byte(0xB2);
    output.add_u16(channel_id);
    output.add_string(channel_name.as_bytes());
}

#[allow(dead_code)]
fn write_close_private(output: &mut OutputMessage, channel_id: u16) {
    output.add_byte(0xB3);
    output.add_u16(channel_id);
}

fn write_private_message(output: &mut OutputMessage, name: &str, level: u16, speak_type: u8, text: &[u8]) {
    output.add_byte(0xAA);
    output.add_u32(0x00);
    output.add_string(name.as_bytes());
    output.add_u16(level);
    output.add_byte(speak_type);
    output.add_string(text);
}

