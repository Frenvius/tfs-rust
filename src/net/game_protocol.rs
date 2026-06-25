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

    /// Port of the `BedItem` branch of `Actions::internalUseItem` + `BedItem::canUse/trySleep/sleep`.
    /// Port of `BedItem::wakeUp` for the online-owner re-use path (regen + clear).
    fn bed_wake_up(&mut self, pos: Position, server_id: u16) {
        g_game().lock().unwrap().wake_bed_at(pos, server_id);
    }

    /// Mirrors C++ `Game::playerRequestTrade` + `internalStartTrade`.
    /// Open a player's depot chest for `depot_id` as a container, showing the
    /// stored items. Mirrors C++ actions.cpp opening the depot locker. (We open
    /// the chest contents directly rather than the locker→chest wrapper.)
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

    // Inventory / open-container look: x==0xFFFF marks a non-map location.
    if pos.x == 0xFFFF {
        let Some(player) = game.get_player(creature_id) else { return };
        let server_id = if pos.y & 0x40 != 0 {
            // open container: cid = y & 0x0F, child index = z
            let cid = (pos.y & 0x0F) as u8;
            player.get_container_by_id(cid)
                .and_then(|_| resolve_container_storage(player, cid))
                .and_then(|(root, path, _)| container_item_ref(&game, creature_id, &root, &path))
                .and_then(|c| c.children.get(usize::from(pos.z)).map(|it| it.server_id))
        } else {
            // equipment slot = y
            let slot = usize::from(pos.y);
            player.inventory_items.get(slot).and_then(|o| o.as_ref()).map(|it| it.server_id)
                .or_else(|| player.inventory.get(slot).copied().flatten())
        };
        let Some(server_id) = server_id else { return };
        let count = 1u32;
        drop(game);
        crate::events::g_events().lock().unwrap().event_player_on_look(
            creature_id, crate::events::LookThingType::Item(server_id, count), pos, stackpos, -1,
        );
        return;
    }

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
            // Topmost visible item: highest client stackpos first — down items
            // (reverse), then top items (reverse), then ground. Mirrors C++
            // Tile::getTopVisibleThing.
            let down = tile.get_down_item_count();
            let item = tile.items[..down].iter().rev()
                .chain(tile.items[down..].iter().rev())
                .find(|item| {
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

fn game_parse_look_in_trade(creature_id: CreatureId, counter_offer: bool, _index: u8) {
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

fn game_parse_rotate_item(_creature_id: CreatureId, pos: Position, _sprite_id: u16, stackpos: u8) {
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
    use crate::creatures::player::TradeState;
    if creature_id == 0 { return; }
    let trade_player_id = target_id;

    let (my_name, partner_name, item, loc, partner_was_idle) = {
        let game = g_game().lock().unwrap();
        let Some(player) = game.get_player(creature_id) else { return };
        let player_pos = player.base.position;
        let my_name = player.name.clone();

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

        let ep = MoveEndpoint::decode(pos, stackpos);
        let Some((item, loc)) = peek_trade_item(&game, creature_id, &ep) else {
            drop(game);
            send_status_message_to_player(creature_id, "Sorry, not possible.");
            return;
        };

        let it = game.items.get_item_type(usize::from(item.server_id));
        if it.client_id != sprite_id || !it.pickupable || item.unique_id != 0 {
            drop(game);
            send_status_message_to_player(creature_id, "Sorry, not possible.");
            return;
        }

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

    send_trade_offer(creature_id, &my_name, &item, true);

    if partner_was_idle {
        {
            let mut game = g_game().lock().unwrap();
            if let Some(partner) = game.get_player_mut(trade_player_id) {
                partner.trade_state = TradeState::Acknowledge;
                partner.trade_partner_id = Some(creature_id);
            }
        }
        send_status_message_to_player(trade_player_id, &format!("{my_name} wants to trade with you."));
    } else {
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
        return;
    }

    let param = text_str.split_once(' ').map(|(_, p)| p).unwrap_or("");
    if crate::events::dispatch::execute_talk_action(creature_id, &text_str, param, speak_type) {
        return;
    }

    match speak_type {
        1..=3 => {
            broadcast_creature_say(creature_id, pos, &name, level as u16, speak_type, text_str.as_bytes());
            notify_nearby_npcs(creature_id, pos, speak_type, &text_str);
        }
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

fn game_parse_update_container(_creature_id: CreatureId, _container_id: u8) {
    // Up-arrow parent navigation: the live container model opens children
    // directly, so re-sending a parent view is a no-op here.
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
                Some(item) => (item.action_id, item.unique_id, tile.use_item_vec_index(stackpos)),
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
            game_handle_use_bed(creature_id, pos, server_id);
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
            if is_depot && pos.x != 0xFFFF {
                open_depot_free(creature_id, server_id, depot_id);
            } else if pos.x == 0xFFFF {
                open_container_in_inventory_free(creature_id, pos.y as u8, server_id);
            } else {
                let tile_item_index = if item_index >= 0 { item_index as usize } else { 0 };
                open_container_on_tile_free(creature_id, pos, tile_item_index, server_id);
            }
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
fn game_parse_throw(creature_id: CreatureId, from_pos: Position, sprite_id: u16, from_stackpos: u8, to_pos: Position, _count: u8) {
    if creature_id == 0 || from_pos == to_pos {
        return;
    }

    // x==0xFFFF marks a non-map location. (y & 0x40) => open container
    // (cid = y & 0x0F, slot = z); otherwise an equipment slot (slot = y).
    let from_is_container = from_pos.x == 0xFFFF && (from_pos.y & 0x40) != 0;
    let to_is_container = to_pos.x == 0xFFFF && (to_pos.y & 0x40) != 0;

    if from_is_container || to_is_container {
        handle_container_move_free(creature_id, from_pos, from_stackpos, sprite_id, to_pos);
        return;
    }

    let from_is_inv = from_pos.x == 0xFFFF;
    let to_is_inv = to_pos.x == 0xFFFF;

    if from_is_inv && to_is_inv {
        handle_inventory_to_inventory(creature_id, from_pos, to_pos);
        return;
    }
    if from_is_inv && !to_is_inv {
        handle_inventory_to_ground(creature_id, from_pos, to_pos, from_stackpos);
        return;
    }
    if !from_is_inv && to_is_inv {
        handle_ground_to_inventory(creature_id, from_pos, to_pos, from_stackpos, sprite_id);
        return;
    }

    // Map tile -> map tile.
    let game = g_game().lock().unwrap();
    let player_pos = match game.get_player(creature_id) {
        Some(p) => p.base.position,
        None => return,
    };

    if player_pos.z != from_pos.z {
        drop(game);
        let text = if from_pos.z > player_pos.z { "First go downstairs." } else { "First go upstairs." };
        send_status_message_to_player(creature_id, text);
        return;
    }

    let dx = (player_pos.x as i32 - from_pos.x as i32).unsigned_abs();
    let dy = (player_pos.y as i32 - from_pos.y as i32).unsigned_abs();
    if dx > 1 || dy > 1 {
        return;
    }

    let throw_dx = (from_pos.x as i32 - to_pos.x as i32).unsigned_abs();
    let throw_dy = (from_pos.y as i32 - to_pos.y as i32).unsigned_abs();
    if throw_dx > 7 || throw_dy > 5 || from_pos.z != to_pos.z {
        drop(game);
        send_status_message_to_player(creature_id, "Destination is out of reach.");
        return;
    }

    let from_tile = match game.map.get_tile(from_pos) {
        Some(t) => t,
        None => return,
    };

    // Resolve the clicked thing in client stack order: ground, top, creature, down.
    let g = if from_tile.ground.is_some() { 1usize } else { 0 };
    let down_count = from_tile.get_down_item_count();
    let top_count = from_tile.items.len().saturating_sub(down_count);
    let ncre = from_tile.creature_ids.len();
    let s = from_stackpos as usize;

    if s >= g + top_count && s < g + top_count + ncre {
        let cre_off = s - (g + top_count);
        let pushed_creature_id = from_tile.creature_ids[ncre - 1 - cre_off];
        drop(game);
        handle_push_creature_free(creature_id, pushed_creature_id, to_pos);
        return;
    }

    let item_idx = if s >= g && s < g + top_count {
        Some(down_count + (s - g)) // top item
    } else if s >= g + top_count + ncre {
        let di = s - (g + top_count + ncre);
        if di < down_count { Some(di) } else { None } // down item
    } else {
        None // ground or out of range
    };

    let item = match item_idx {
        Some(idx) if idx < from_tile.items.len() => from_tile.items[idx].clone(),
        _ => return,
    };

    let it = game.items.get_item_type(item.server_id as usize);
    if it.client_id != sprite_id {
        return;
    }
    if !it.moveable {
        drop(game);
        send_status_message_to_player(creature_id, "You cannot move this object.");
        return;
    }

    let to_tile = match game.map.get_tile(to_pos) {
        Some(t) => t,
        None => return,
    };
    if to_tile.ground.is_none() {
        drop(game);
        send_status_message_to_player(creature_id, "There is no way.");
        return;
    }
    drop(game);

    let items_arc = g_game().lock().unwrap().items.clone();

    let (removed_stackpos, add_stackpos, delivered_to_mailbox) = {
        let mut game = g_game().lock().unwrap();
        let has_mailbox = game.map.get_tile(to_pos)
            .map(|t| t.has_flag(crate::map::tile::TILESTATE_MAILBOX))
            .unwrap_or(false);
        let delivered = has_mailbox
            && crate::items::special::mailbox::Mailbox::can_send(item.server_id)
            && mailbox_deliver(&mut game, &item);

        // Remove from source keeping the [down|top] partition (down_item_count)
        // consistent, and capture the real client stackpos it occupied.
        let removed_sp = match (game.map.get_tile_mut(from_pos), item_idx) {
            (Some(from_t), Some(idx)) => from_t.remove_item_at(idx).map(|(_, sp)| sp).unwrap_or(from_stackpos),
            _ => from_stackpos,
        };

        // Add to destination through the partition-aware path so the broadcast
        // stackpos matches the client's own getThingIndex.
        let add_sp = if !delivered {
            game.map.get_tile_mut(to_pos).map(|t| t.add_item_get_stackpos(item.clone(), &items_arc))
        } else {
            None
        };

        (removed_sp, add_sp, delivered)
    };

    // Collect spectators, then drop g_game BEFORE sending: send_packet_to_player
    // locks player_sessions, and holding g_game across that lock inverts the
    // g_game→player_sessions order used everywhere else, wedging the server.
    let (from_spectators, to_spectators) = {
        let mut game = g_game().lock().unwrap();
        (
            game.map.get_spectators(from_pos, true, true, 0, 0, 0, 0),
            game.map.get_spectators(to_pos, true, true, 0, 0, 0, 0),
        )
    };

    for spec_id in from_spectators {
        send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
            write_remove_tile_thing(output, from_pos, removed_stackpos);
        });
    }

    if delivered_to_mailbox {
        return;
    }

    let Some(add_stackpos) = add_stackpos else { return };
    for spec_id in to_spectators {
        let item = item.clone();
        let items_ref = items_arc.clone();
        send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
            write_add_tile_item(output, to_pos, add_stackpos, &item, &items_ref);
        });
    }
}

fn handle_container_move_free(player_id: CreatureId, from_pos: Position, from_stackpos: u8, sprite_id: u16, to_pos: Position) {
    let from = MoveEndpoint::decode(from_pos, from_stackpos);
    let to = MoveEndpoint::decode(to_pos, 0);

    let mut game = g_game().lock().unwrap();
    let item = match extract_move_item(&mut game, player_id, &from) {
        Some(i) => i,
        None => {
            drop(game);
            send_status_message_to_player(player_id, "Sorry, not possible.");
            return;
        }
    };

    if sprite_id != 0 {
        let client_id = game.items.get_item_type(usize::from(item.server_id)).client_id;
        if client_id != sprite_id {
            insert_move_item(&mut game, player_id, &from, item);
            drop(game);
            send_status_message_to_player(player_id, "Sorry, not possible.");
            return;
        }
    }

    if !insert_move_item(&mut game, player_id, &to, item.clone()) {
        insert_move_item(&mut game, player_id, &from, item);
        drop(game);
        send_status_message_to_player(player_id, "Sorry, not possible.");
        return;
    }
    drop(game);

    refresh_move_endpoint_free(player_id, &from);
    if to != from {
        refresh_move_endpoint_free(player_id, &to);
    }
}

fn refresh_move_endpoint_free(player_id: CreatureId, ep: &MoveEndpoint) {
    match *ep {
        MoveEndpoint::Container { cid, .. } => resend_open_container_free(player_id, cid),
        MoveEndpoint::Inventory { slot } => {
            let (sid, count) = {
                let game = g_game().lock().unwrap();
                match game.get_player(player_id) {
                    Some(p) => (p.inventory[slot], p.inventory_count[slot]),
                    None => (None, 0),
                }
            };
            let client_id = sid.map(|s| g_game().lock().unwrap().items.get_item_type(usize::from(s)).client_id);
            let s = slot as u8;
            let c = count.max(1) as u8;
            send_packet_to_player(player_id, move |output: &mut OutputMessage| {
                match client_id {
                    Some(cid) => { output.add_byte(0x78); output.add_byte(s); output.add_u16(cid); output.add_byte(c); }
                    None => { output.add_byte(0x79); output.add_byte(s); }
                }
            });
        }
        MoveEndpoint::Ground { pos, .. } => {
            let sessions = player_sessions().lock().unwrap();
            let Some(session) = sessions.get(&player_id) else { return };
            let known = &mut session.known_creatures.lock().unwrap();
            let game = g_game().lock().unwrap();
            let mut output = OutputMessage::new();
            output.add_byte(0x69);
            output.add_position(pos.x, pos.y, pos.z);
            if let Some(tile) = game.map.get_tile(pos) {
                write_tile_description(&mut output, &game, tile, game.get_items(), known, None);
                output.add_byte(0x00);
                output.add_byte(0xFF);
            } else {
                output.add_byte(0x01);
                output.add_byte(0xFF);
            }
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }
}

fn resend_open_container_free(player_id: CreatureId, cid: u8) {
    use crate::creatures::player::ContainerParent;
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(player_id) else { return };
    let Some(oc) = player.get_container_by_id(cid) else { return };
    let parent = oc.parent.clone();

    if let ContainerParent::Depot(depot_id) = parent {
        let children: Vec<crate::map::tile::MapItem> =
            player.depot_items.get(&depot_id).cloned().unwrap_or_default();
        let items_ref = game.items.clone();
        let chest = crate::map::tile::MapItem { server_id: 2594, ..crate::map::tile::MapItem::default() };
        drop(game);
        send_packet_to_player(player_id, move |output: &mut OutputMessage| {
            write_container(output, cid, &chest, &items_ref, "Depot chest", 255, false, &children);
        });
        return;
    }

    let Some((root, path, _scroll)) = resolve_container_storage(player, cid) else { return };
    let container_item = match container_item_ref(&game, player_id, &root, &path) {
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

    send_packet_to_player(player_id, move |output: &mut OutputMessage| {
        write_container(output, cid, &container_item, &items_ref, &name, capacity, has_parent, &children);
    });
}

fn game_handle_use_bed(creature_id: CreatureId, pos: Position, server_id: u16) {
    use crate::map::tile::TileKind;

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
        let can_edit = player.group_flags & crate::creatures::player::PLAYER_FLAG_CAN_EDIT_HOUSES != 0;
        let p_name = player.name.clone();

        let owned_by_account = g_config().get_boolean(BooleanConfig::HouseOwnedByAccount);
        let (house_owner, my_access) = match house_id.and_then(|hid| game.map.houses.get_house(hid)) {
            Some(h) => (
                h.get_owner(),
                h.access_level_for(p_guid, p_account, can_edit, owned_by_account, &p_name, "", ""),
            ),
            None => (0, crate::map::houses::AccessHouseLevel::NotInvited),
        };

        BedUseInfo {
            house_id, is_pz, sleeper_guid, transform_to_free, transform_male, transform_female,
            partner_dir, p_guid, p_sex, premium, house_owner, my_access,
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
        let msg = if !has_house {
            "You can not use this bed."
        } else if !info.premium {
            "You need a premium account."
        } else {
            "You cannot use this object."
        };
        send_status_message_to_player(creature_id, msg);
        return;
    }

    if info.sleeper_guid != 0 {
        if info.transform_to_free != 0 && info.house_owner == info.p_guid {
            bed_wake_up_free(pos, server_id);
        }
        let mut game = g_game().lock().unwrap();
        let ppos = game.get_player(creature_id).map(|p| p.base.position).unwrap_or(pos);
        game.add_magic_effect(ppos, crate::game::CONST_ME_POFF);
        return;
    }

    bed_sleep_free(creature_id, pos, server_id, &info);
}

fn bed_sleep_free(creature_id: CreatureId, pos: Position, server_id: u16, info: &BedUseInfo) {
    use crate::creatures::player::PlayerSex;
    let now = (crate::util::otsys_time() / 1000) as u32;
    let partner_pos = next_position(info.partner_dir, pos);

    {
        let mut game = g_game().lock().unwrap();
        let pname = game.get_player(creature_id).map(|p| p.name.clone()).unwrap_or_default();
        let desc = format!("{} is sleeping there.", pname);
        let guid = info.p_guid;

        let partner_sid = game.map.get_tile(partner_pos).and_then(|t| {
            t.items.iter()
                .find(|it| game.items.get_item_type(usize::from(it.server_id)).kind == crate::items::ItemKind::Bed)
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

        let old_pos = game.get_player(creature_id).map(|p| p.base.position).unwrap_or(pos);
        if old_pos != pos {
            game.move_creature_position(creature_id, old_pos, pos);
        }
        game.add_magic_effect(pos, crate::game::CONST_ME_SLEEP);

        let sex_transform = match info.p_sex {
            PlayerSex::Male => info.transform_male,
            PlayerSex::Female => info.transform_female,
        };
        let new_id = if sex_transform != 0 { sex_transform } else { info.transform_to_free };
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

fn bed_wake_up_free(pos: Position, server_id: u16) {
    g_game().lock().unwrap().wake_bed_at(pos, server_id);
}

fn open_container_on_tile_free(player_id: CreatureId, pos: Position, tile_item_index: usize, server_id: u16) {
    use crate::creatures::player::ContainerParent;
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(player_id) else { return };

    if let Some(existing_cid) = player.get_container_id_by_tile(pos, tile_item_index) {
        drop(game);
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(player_id) { player.close_container(existing_cid); }
        drop(game);
        send_packet_to_player(player_id, move |o: &mut OutputMessage| write_close_container(o, existing_cid));
        return;
    }

    let Some(cid) = player.get_free_container_id() else {
        drop(game);
        send_status_message_to_player(player_id, "You cannot open any more containers.");
        return;
    };
    let Some(tile) = game.map.get_tile(pos) else { return };
    let Some(container_item) = tile.items.get(tile_item_index) else { return };

    let item_type = game.items.get_item_type(usize::from(server_id));
    let name = if container_item.name.is_empty() { item_type.name.clone() } else { container_item.name.clone() };
    let capacity = item_type.max_items.min(255) as u8;
    let container_item_clone = container_item.clone();
    let children_clone = container_item.children.clone();
    let items_ref = game.items.clone();
    drop(game);

    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(player_id) {
            player.add_container(cid, ContainerParent::Tile(pos, tile_item_index));
        }
    }
    send_packet_to_player(player_id, move |o: &mut OutputMessage| {
        write_container(o, cid, &container_item_clone, &items_ref, &name, capacity, false, &children_clone);
    });
}

fn open_depot_free(player_id: CreatureId, server_id: u16, depot_id: u32) {
    use crate::creatures::player::ContainerParent;
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(player_id) else { return };

    let existing = player.open_containers.iter()
        .find(|(_, oc)| matches!(oc.parent, ContainerParent::Depot(d) if d == depot_id))
        .map(|(&cid, _)| cid);
    if let Some(existing_cid) = existing {
        drop(game);
        let mut game = g_game().lock().unwrap();
        if let Some(p) = game.get_player_mut(player_id) { p.close_container(existing_cid); }
        drop(game);
        send_packet_to_player(player_id, move |o: &mut OutputMessage| write_close_container(o, existing_cid));
        return;
    }

    let Some(cid) = player.get_free_container_id() else {
        drop(game);
        send_status_message_to_player(player_id, "You cannot open any more containers.");
        return;
    };
    let item_type = game.items.get_item_type(usize::from(server_id));
    let capacity = item_type.max_items.clamp(1, 255) as u8;
    let children: Vec<crate::map::tile::MapItem> = player.depot_items.get(&depot_id).cloned().unwrap_or_default();
    let chest = crate::map::tile::MapItem { server_id, ..crate::map::tile::MapItem::default() };
    let items_ref = game.items.clone();
    drop(game);

    {
        let mut game = g_game().lock().unwrap();
        if let Some(p) = game.get_player_mut(player_id) {
            p.add_container(cid, ContainerParent::Depot(depot_id));
            p.set_last_depot_id(depot_id as i16);
        }
    }
    send_packet_to_player(player_id, move |o: &mut OutputMessage| {
        write_container(o, cid, &chest, &items_ref, "Depot chest", capacity, false, &children);
    });
}

fn open_container_in_inventory_free(player_id: CreatureId, slot: u8, server_id: u16) {
    use crate::creatures::player::ContainerParent;
    let game = g_game().lock().unwrap();
    let Some(player) = game.get_player(player_id) else { return };

    if let Some(existing_cid) = player.get_container_id_by_inventory(slot) {
        drop(game);
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(player_id) { player.close_container(existing_cid); }
        drop(game);
        send_packet_to_player(player_id, move |o: &mut OutputMessage| write_close_container(o, existing_cid));
        return;
    }

    let Some(cid) = player.get_free_container_id() else {
        drop(game);
        send_status_message_to_player(player_id, "You cannot open any more containers.");
        return;
    };
    let Some(Some(container_item)) = player.inventory_items.get(usize::from(slot)) else { return };

    let item_type = game.items.get_item_type(usize::from(server_id));
    let name = if container_item.name.is_empty() { item_type.name.clone() } else { container_item.name.clone() };
    let capacity = item_type.max_items.min(255) as u8;
    let container_item_clone = container_item.clone();
    let children_clone = container_item.children.clone();
    let items_ref = game.items.clone();
    drop(game);

    {
        let mut game = g_game().lock().unwrap();
        if let Some(player) = game.get_player_mut(player_id) {
            player.add_container(cid, ContainerParent::Inventory(slot));
        }
    }
    send_packet_to_player(player_id, move |o: &mut OutputMessage| {
        write_container(o, cid, &container_item_clone, &items_ref, &name, capacity, false, &children_clone);
    });
}

fn handle_push_creature_free(player_id: CreatureId, pushed_creature_id: CreatureId, to_pos: Position) {
    use crate::creatures::player::PLAYER_FLAG_CAN_PUSH_ALL_CREATURES;
    use crate::map::tile::{TILESTATE_BLOCKPATH, TILESTATE_PROTECTIONZONE, TILESTATE_NOPVPZONE};

    let old_pos: Position;
    let old_stackpos: u8;
    {
        let game = g_game().lock().unwrap();
        let player = match game.get_player(player_id) { Some(p) => p, None => return };
        let player_pos = player.base.position;
        let can_push_all = player.has_flag(PLAYER_FLAG_CAN_PUSH_ALL_CREATURES);

        let creature: &crate::creatures::Creature = match game.get_creature(pushed_creature_id) {
            Some(c) => c,
            None => return,
        };

        if creature.base().movement_blocked {
            send_status_message_to_player(player_id, "You cannot move this object.");
            return;
        }

        let creature_pos = creature.position();
        let is_pushable = match creature {
            crate::creatures::Creature::Player(p) => p.is_pushable(),
            crate::creatures::Creature::Monster(m) => m.is_pushable(),
            crate::creatures::Creature::Npc(_) => true,
        };

        if !is_pushable && !can_push_all {
            send_status_message_to_player(player_id, "You cannot move this object.");
            return;
        }

        if creature.is_in_ghost_mode() {
            send_status_message_to_player(player_id, "You cannot move this object.");
            return;
        }

        let dx = (creature_pos.x as i32 - player_pos.x as i32).unsigned_abs();
        let dy = (creature_pos.y as i32 - player_pos.y as i32).unsigned_abs();
        let dz = if creature_pos.z > player_pos.z { (creature_pos.z - player_pos.z) as u32 } else { (player_pos.z - creature_pos.z) as u32 };
        if dx > 1 || dy > 1 || dz > 0 {
            send_status_message_to_player(player_id, "There is no way.");
            return;
        }

        let throw_dx = (creature_pos.x as i32 - to_pos.x as i32).unsigned_abs();
        let throw_dy = (creature_pos.y as i32 - to_pos.y as i32).unsigned_abs();
        let throw_dz = if creature_pos.z > to_pos.z { (creature_pos.z - to_pos.z) as u32 } else { (to_pos.z - creature_pos.z) as u32 };
        if throw_dx > 1 || throw_dy > 1 || throw_dz * 4 > 1 {
            send_status_message_to_player(player_id, "Destination is out of range.");
            return;
        }

        let to_tile = match game.map.get_tile(to_pos) {
            Some(t) => t,
            None => {
                send_status_message_to_player(player_id, "Sorry, not possible.");
                return;
            }
        };

        if player_id != pushed_creature_id {
            if to_tile.has_flag(TILESTATE_BLOCKPATH) {
                send_status_message_to_player(player_id, "There is not enough room.");
                return;
            }

            let creature_tile = game.map.get_tile(creature_pos);
            let creature_on_pz = creature_tile.map(|t| t.has_flag(TILESTATE_PROTECTIONZONE)).unwrap_or(false);
            let creature_on_nopvp = creature_tile.map(|t| t.has_flag(TILESTATE_NOPVPZONE)).unwrap_or(false);
            let dest_is_pz = to_tile.has_flag(TILESTATE_PROTECTIONZONE);
            let dest_is_nopvp = to_tile.has_flag(TILESTATE_NOPVPZONE);

            if (creature_on_pz && !dest_is_pz) || (creature_on_nopvp && !dest_is_nopvp) {
                send_status_message_to_player(player_id, "Sorry, not possible.");
                return;
            }

            for &cid in to_tile.get_creatures() {
                let Some(tc) = game.get_creature(cid) else { continue };
                if !tc.is_in_ghost_mode() {
                    send_status_message_to_player(player_id, "There is not enough room.");
                    return;
                }
            }
        }

        if to_tile.ground.is_none() || to_tile.has_flag(TILESTATE_BLOCKSOLID) {
            send_status_message_to_player(player_id, "There is not enough room.");
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
        if !events.event_player_on_move_creature(player_id, pushed_creature_id, creature_class, old_pos, to_pos) {
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

    let pushed_is_player = {
        let game = g_game().lock().unwrap();
        game.get_creature(pushed_creature_id).map(|c| c.is_player()).unwrap_or(false)
    };
    if pushed_is_player && pushed_creature_id != player_id {
        // Lock player_sessions BEFORE g_game (the order used everywhere else);
        // inverting it wedges the server.
        let sessions = player_sessions().lock().unwrap();
        if let Some(session) = sessions.get(&pushed_creature_id) {
            let known = &mut session.known_creatures.lock().unwrap();
            let game = g_game().lock().unwrap();
            let mut output = OutputMessage::new();
            output.add_byte(0x6D);
            write_creature_movement(&mut output, old_pos, to_pos, old_stackpos, pushed_creature_id);
            append_walk_map_slices(&mut output, &game, game.get_items(), known, old_pos, to_pos);
            finalize_and_send(&mut output, &session.round_keys, session.checksum_enabled, &session.conn);
        }
    }
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
        let server_id = if pos.y & 0x40 != 0 {
            // item inside an open container: cid = y & 0x0F, child index = z
            let cid = (pos.y & 0x0F) as u8;
            let (root, path, _) = resolve_container_storage(player, cid)?;
            let container = container_item_ref(&game, creature_id, &root, &path)?;
            container.children.get(usize::from(pos.z))?.server_id
        } else {
            let slot = usize::from(pos.y);
            player.get_inventory_item(slot)?
        };
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

/// Remove (or decrement) an item the Lua layer holds by an inventory/container
/// encoded position (x==0xFFFF). Returns true when handled. Refreshes the
/// affected equipment slot or open container for the owner.
pub(crate) fn lua_remove_inventory_item(owner_cid: u32, pos: Position, count: i32) -> bool {
    if pos.x != 0xFFFF {
        return false;
    }
    if pos.y & 0x40 != 0 {
        let cid = (pos.y & 0x0F) as u8;
        let child_idx = usize::from(pos.z);
        {
            let mut game = g_game().lock().unwrap();
            let resolved = game.get_player(owner_cid).and_then(|p| resolve_container_storage(p, cid));
            if let Some((root, path, _)) = resolved {
                if let Some(children) = container_children_mut(&mut game, owner_cid, &root, &path) {
                    if let Some(it) = children.get_mut(child_idx) {
                        if count > 0 && (it.count as i32) > count {
                            it.count -= count as u16;
                        } else {
                            children.remove(child_idx);
                        }
                    }
                }
            }
        }
        resend_open_container_free(owner_cid, cid);
        true
    } else {
        let slot = usize::from(pos.y);
        {
            let mut game = g_game().lock().unwrap();
            if let Some(player) = game.get_player_mut(owner_cid) {
                if slot < player.inventory.len() {
                    player.inventory[slot] = None;
                    player.inventory_count[slot] = 0;
                    if slot < player.inventory_items.len() {
                        player.inventory_items[slot] = None;
                    }
                }
            }
        }
        send_clear_inventory_slot(owner_cid, slot as u8);
        true
    }
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
            let vi = tile.use_item_vec_index(stackpos);
            if vi < 0 { return None; }
            tile.remove_item_at(vi as usize).map(|(it, _)| it)
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
            let vi = tile.use_item_vec_index(stackpos);
            if vi < 0 { return None; }
            let idx = vi as usize;
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
            tile.remove_item_at(idx).map(|(it, _)| it)
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

    let light = creature.as_player().map(|p| p.get_creature_light()).unwrap_or(base.internal_light);
    output.add_byte(light.level);
    output.add_byte(light.color);

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

pub(crate) fn write_item(output: &mut OutputMessage, items: &Items, server_id: u16, count: u8) -> bool {
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

/// Send a single inventory-slot update (0x78) to a player, using the full
/// 10.98 item encoding (mark byte + animation byte) via `write_item`. Used when
/// a carried item transforms (e.g. lighting a torch in an equipment slot).
pub fn send_inventory_slot_update(creature_id: CreatureId, slot: u8, server_id: u16, count: u8, items: &Items) {
    if items.get_item_type(server_id as usize).client_id == 0 {
        return;
    }
    let items_ref = items;
    send_packet_to_player(creature_id, move |output: &mut OutputMessage| {
        output.add_byte(0x78);
        output.add_byte(slot);
        write_item(output, items_ref, server_id, count);
    });
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

fn handle_ground_to_inventory(creature_id: CreatureId, from_pos: Position, to_pos: Position, from_stackpos: u8, sprite_id: u16) {
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
        let vi = tile.use_item_vec_index(from_stackpos);
        if vi < 0 { return; }
        let idx_in_tile = vi as usize;
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
            tile.remove_item_at(item_idx);
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

fn handle_inventory_to_ground(creature_id: CreatureId, from_pos: Position, to_pos: Position, _from_stackpos: u8) {
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

    let items_arc = g_game().lock().unwrap().items.clone();
    let (stackpos, drop_count) = {
        let mut game = g_game().lock().unwrap();
        let Some(tile) = game.map.get_tile_mut(to_pos) else { return };
        if tile.ground.is_none() { return; }
        // Carry the full item (container children) onto the ground when present.
        let item = dropped_tree.clone()
            .unwrap_or_else(|| crate::map::tile::MapItem { server_id, ..crate::map::tile::MapItem::default() });
        let cnt = item.count.min(255).max(1) as u8;
        let sp = tile.add_item_get_stackpos(item, &items_arc);
        (sp, cnt)
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
        let items_ref = items_arc.clone();
        send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
            output.add_byte(0x6A);
            output.add_position(to_pos.x, to_pos.y, to_pos.z);
            output.add_byte(stackpos);
            write_item(output, &items_ref, server_id, drop_count);
        });
    }
}

fn handle_inventory_to_inventory(creature_id: CreatureId, from_pos: Position, to_pos: Position) {
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

    if client_version().is_1098() {
        output.add_byte(0x01); // drag and drop (unlocked)
        output.add_byte(0x00); // pagination disabled
        let size = contents.len().min(0xFFFF) as u16;
        output.add_u16(size);  // container size
        output.add_u16(0);     // first index
        if size > 0 {
            let to_send = (capacity as usize).min(size as usize).min(255);
            output.add_byte(to_send as u8);
            for item in contents.iter().take(to_send) {
                write_item(output, items, item.server_id, item.count.min(255) as u8);
            }
        } else {
            output.add_byte(0x00);
        }
    } else {
        let count = contents.len().min(255);
        output.add_byte(count as u8);
        for item in contents.iter().take(count) {
            write_item(output, items, item.server_id, item.count.min(255) as u8);
        }
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

