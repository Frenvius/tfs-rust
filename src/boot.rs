use std::ffi::OsString;

use anyhow::{anyhow, bail, Result};

use crate::config::{g_config, init_config, ConfigManager, IntegerConfig, StringConfig};
use crate::crypto::rsa::{init_rsa, Rsa};
use crate::db::{init_database, Database};
use crate::game::{init_game, Game, GameState};
use crate::items::Items;
use crate::map::otbm;
use crate::net::server::{ProtocolKind, ServiceInfo, ServiceManager};
use crate::runtime::dispatcher::Dispatcher;
use crate::runtime::scheduler::Scheduler;
use crate::runtime::{g_dispatcher, g_scheduler, init_runtime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub config_file: String,
    pub ip: Option<String>,
    pub login_port: Option<u16>,
    pub game_port: Option<u16>,
}

/// Expected items.otb `minor_version` (the CLIENT_VERSION enum from
/// `itemloader.h`) for a given Tibia client protocol version.
fn expected_otb_minor(client_version: u16) -> Option<u16> {
    match client_version {
        860 => Some(20),         // CLIENT_VERSION_860
        1097 | 1098 => Some(57), // CLIENT_VERSION_1098
        _ => None,
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            config_file: String::from("config.lua"),
            ip: None,
            login_port: None,
            game_port: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Success,
    Failure,
}

pub fn run<I>(args: I) -> Result<ExitStatus>
where
    I: IntoIterator<Item = OsString>,
{
    match parse_arguments(args)? {
        ParsedArguments::Run(options) => {
            initialize_tracing()?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async_run(options))
        }
        ParsedArguments::Exit(status) => Ok(status),
    }
}

async fn async_run(options: Options) -> Result<ExitStatus> {
    let boot_start = std::time::Instant::now();

    // Start dispatcher and scheduler.
    let (dispatcher, worker) = Dispatcher::new();
    let scheduler = Scheduler::new(dispatcher.sender());
    let handle = tokio::spawn(worker.run());
    init_runtime(dispatcher, scheduler, handle);

    // Load configuration.
    println!(">> Loading config");
    let mut config = ConfigManager::load_from_path(&options.config_file)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    if let Some(ip) = options.ip {
        config.set_string(crate::config::StringConfig::IpString, ip).ok();
    }
    if let Some(port) = options.login_port {
        config.set_number(IntegerConfig::LoginPort, port as i32).ok();
    }
    if let Some(port) = options.game_port {
        config.set_number(IntegerConfig::GamePort, port as i32).ok();
    }
    init_config(config);

    // Determine protocol version from config.
    let version_min = g_config().get_number(IntegerConfig::ClientVersionMin) as u16;
    crate::net::protocol_version::init_client_version(version_min);

    // Load RSA key.
    let rsa = Rsa::load_pem("key.pem").map_err(|e| anyhow!("{e}"))?;
    init_rsa(rsa);

    // Connect to database.
    print!(">> Establishing database connection...");
    let db = Database::connect(g_config()).await?;
    init_database(db);
    println!(" MySQL");

    // Initialise Lua environment (must happen before scripts are loaded).
    crate::lua::init_lua_env().map_err(|e| anyhow!("{e}"))?;

    // Load items from OTB (then optional XML overlay).
    print!(">> Loading items... ");
    let mut items = Items::new();
    items.load_from_otb("data/items/items.otb").map_err(|e| anyhow!("{e}"))?;
    let xml_path = "data/items/items.xml";
    if std::path::Path::new(xml_path).exists() {
        items.load_from_xml(xml_path).map_err(|e| anyhow!("{e}"))?;
    }
    println!("OTB v{}.{}.{}", items.major_version(), items.minor_version(), items.build_number());

    // Verify the items.otb client version matches the configured client version.
    // OTB `minor_version` is the CLIENT_VERSION enum (8.60 = 20, 10.98 = 57).
    if let Some(expected_minor) = expected_otb_minor(version_min) {
        if items.minor_version() != expected_minor as u32 {
            return Err(anyhow!(
                "items.otb client mismatch: OTB minor={} but config clientVersion={} expects OTB minor={}. \
                 Use the items.otb/items.xml that match client {}.",
                items.minor_version(), version_min, expected_minor, version_min
            ));
        }
    } else {
        println!(
            ">> Warning: no known items.otb version for client {} — skipping OTB version check",
            version_min
        );
    }

    let items_arc = std::sync::Arc::new(items);
    crate::items::init_items(items_arc.clone());

    // Load monster types.
    print!(">> Loading monsters... ");
    let mut monsters = crate::creatures::monsters::Monsters::new();
    if std::path::Path::new("data/monster/monsters.xml").exists() {
        monsters.load_from_xml(std::path::Path::new("data"))
            .map_err(|e| anyhow!("{e}"))?;
    }
    let n_monsters = monsters.monsters.len();
    crate::creatures::monsters::init_monsters(monsters);
    println!("{} types", n_monsters);

    // Load NPC types.
    print!(">> Loading npcs... ");
    let mut npcs = crate::creatures::npc::Npcs::new();
    let npc_dir = std::path::Path::new("data/npc");
    if npc_dir.exists() {
        npcs.load_from_dir(npc_dir).map_err(|e| anyhow!("{e}"))?;
    }
    let n_npcs = npcs.npc_types.len();
    crate::creatures::npc::init_npcs(npcs);
    println!("{} types", n_npcs);

    // Initialise game state early (C++ g_game exists from program start).
    // Scripts need access to g_game().items during loading.
    let mut game = Game::new();
    game.items = items_arc;
    game.set_game_state(GameState::Startup);
    init_game(game);

    // Load script systems (events, spells, actions, etc.).
    println!(">> Loading lua scripts");
    crate::lua::ScriptingManager::load_script_systems().map_err(|e| anyhow!("{e}"))?;

    // Load vocations from data/XML/, mirroring the C++ data/XML/*.xml layout;
    // fall back to data/ root for either layout.
    let voc_path = first_existing(&["data/XML/vocations.xml", "data/vocations.xml"]);
    if let Some(voc_path) = voc_path {
        let vocations = crate::world::vocation::Vocations::load_from_xml(voc_path)
            .map_err(|e| anyhow!("{e}"))?;
        crate::world::vocation::init_vocations(vocations);
    } else {
        crate::world::vocation::init_vocations(crate::world::vocation::Vocations::default());
    }

    // Load groups.
    let groups_path = first_existing(&["data/XML/groups.xml", "data/groups.xml"]);
    if let Some(groups_path) = groups_path {
        match crate::world::groups::Groups::load_from_xml(groups_path) {
            Ok(groups) => crate::world::groups::init_groups(groups),
            Err(e) => tracing::warn!("Groups::load_from_xml failed: {e}"),
        }
    }

    // Load quests.
    let quests_path = first_existing(&["data/XML/quests.xml", "data/quests.xml"]);
    if let Some(quests_path) = quests_path {
        let quests = crate::world::quests::Quests::load_from_xml(quests_path)
            .map_err(|e| anyhow!("{e}"))?;
        crate::world::quests::init_quests(quests);
    } else {
        crate::world::quests::init_quests(crate::world::quests::Quests::default());
    }

    // Load outfits.
    let outfit_path = first_existing(&["data/XML/outfits.xml", "data/outfits.xml"]);
    if let Some(outfit_path) = outfit_path {
        let outfits = crate::world::outfit::Outfits::load_from_xml(outfit_path)
            .map_err(|e| anyhow!("{e}"))?;
        crate::world::outfit::init_outfits(outfits);
    } else {
        crate::world::outfit::init_outfits(crate::world::outfit::Outfits::default());
    }

    // Load map from OTBM.
    println!(">> Loading map");
    let map_name = g_config().get_string(StringConfig::MapName);
    let map_path = format!("data/world/{map_name}.otbm");
    let items_ref = crate::game::g_game().lock().unwrap().items.clone();
    let map = otbm::load_from_path(&map_path, &items_ref).map_err(|e| anyhow!("{e}"))?;

    // Set map and finalize game state.
    println!(">> Initializing gamestate");
    crate::game::g_game().lock().unwrap().map = map;
    crate::game::g_game().lock().unwrap().set_game_state(GameState::Init);

    // Load persisted house ownership + access lists + movable items (tile_store).
    {
        let db = crate::db::g_database();
        if let Err(e) = crate::map::serialize::IOMapSerialize::load_house_info(db).await {
            tracing::warn!("loadHouseInfo failed: {e}");
        }
        if let Err(e) = crate::map::serialize::IOMapSerialize::load_house_items(db).await {
            tracing::warn!("loadHouseItems failed: {e}");
        }
        // Register bed sleepers from loaded house beds (C++ BedItem::readAttr
        // calls Game::setBedSleeper while deserializing the sleeper guid).
        {
            let mut game = crate::game::g_game().lock().unwrap();
            let bed_positions: Vec<crate::map::Position> = game
                .map
                .houses
                .get_houses()
                .values()
                .flat_map(|h| h.beds.clone())
                .collect();
            for pos in bed_positions {
                if let Some(tile) = game.map.get_tile(pos) {
                    let sleeper = tile
                        .items
                        .iter()
                        .find(|it| {
                            game.items.get_item_type(usize::from(it.server_id)).kind
                                == crate::items::ItemKind::Bed
                        })
                        .map(|it| it.sleeper_guid)
                        .unwrap_or(0);
                    if sleeper != 0 {
                        game.set_bed_sleeper(sleeper, pos);
                    }
                }
            }
        }
    }

    // Load and start spawns.
    {
        let spawn_file_name = g_config().get_string(crate::config::StringConfig::MapName).to_owned();
        let spawn_path = format!("data/world/{spawn_file_name}-spawn.xml");
        if std::path::Path::new(&spawn_path).exists() {
            let mut spawns = crate::world::spawn::Spawns::new();
            spawns.load_from_xml(std::path::Path::new(&spawn_path))
                .map_err(|e| anyhow!("{e}"))?;
            let n_spawns = spawns.spawn_list.len();
            spawns.startup();
            crate::game::g_game().lock().unwrap().spawns = spawns;
            println!(">> Loaded {} spawn zones", n_spawns);
        }
    }

    // Load MOTD counter and players record from server_config.
    load_motd_num().await;
    load_players_record().await;

    // Set initial world time from real clock.
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let os_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            % 86400;
        let world_time = (os_secs as f32 / 2.5) as i16;
        crate::game::g_game().lock().unwrap().set_world_time(world_time);
    }

    // Apply world type from config (pvp / nopvp / pvpenforced).
    {
        use crate::game::WorldType;
        let wt_str = g_config().get_string(crate::config::StringConfig::WorldType).to_lowercase();
        let world_type = match wt_str.as_str() {
            "nopvp" | "no-pvp" => WorldType::NoPvp,
            "pvpenforced" | "pvp-enforced" => WorldType::PvpEnforced,
            _ => WorldType::Pvp,
        };
        crate::game::g_game().lock().unwrap().set_world_type(world_type);
    }

    // Fire startup global events (mirrors g_globalEvents->startup() in C++).
    crate::events::registry::g_script_registry().lock().unwrap().global_events.startup();

    // Transition to Normal game state — clients can now log in.
    crate::game::g_game().lock().unwrap().set_game_state(GameState::Normal);

    println!(">> Loaded all modules, server starting up...");

    // Wire up service ports.
    let login_port = g_config().get_number(IntegerConfig::LoginPort) as u16;
    let game_port  = g_config().get_number(IntegerConfig::GamePort)  as u16;
    let status_port = g_config().get_number(IntegerConfig::StatusPort) as u16;

    let mut services = ServiceManager::new();

    services.add_service(status_port, ServiceInfo {
        protocol_id: 0xFF,
        checksummed: false,
        server_sends_first: false,
        kind: ProtocolKind::Status,
    });

    services.add_service(login_port, ServiceInfo {
        protocol_id: 0x01,
        checksummed: true,
        server_sends_first: false,
        kind: ProtocolKind::Login,
    });

    services.add_service(game_port, ServiceInfo {
        protocol_id: 0x00,
        checksummed: true,
        server_sends_first: true,
        kind: ProtocolKind::Game,
    });

    services.start();

    // Start game tick (checkCreatures, checkLight, etc.) matching C++ Game::start().
    crate::game::tick::schedule_tick_events();
    crate::game::tick::schedule_rent_loop();

    let server_name = g_config().get_string(StringConfig::ServerName).to_owned();
    println!(
        "\n>> {} Server Online! (started in {:.2}s)\n",
        server_name,
        boot_start.elapsed().as_secs_f64()
    );

    let _ = g_dispatcher();
    let _ = g_scheduler();

    // Wait for Ctrl-C, then persist game state before exiting (C++ saveGameState).
    if tokio::signal::ctrl_c().await.is_ok() {
        println!("\n>> Saving server...");
        save_game_state().await;
        println!(">> Server saved. Shutting down.");
    }

    Ok(ExitStatus::Success)
}

/// Persist house ownership/items and all online players. Mirrors the relevant
/// parts of C++ `Game::saveGameState` (called on shutdown / autosave).
async fn save_game_state() {
    use crate::db::g_database;

    crate::game::g_game().lock().unwrap().set_game_state(GameState::Shutdown);

    // Snapshot online players without holding the lock across the async saves.
    let snapshots: Vec<crate::db::login::PlayerSaveSnapshot> = {
        let game = crate::game::g_game().lock().unwrap();
        game.get_players_online()
            .map(|(_, p)| crate::db::login::PlayerSaveSnapshot::from_player(p))
            .collect()
    };
    for snap in &snapshots {
        let _ = crate::db::login::save_player(snap).await;
    }

    let db = g_database();
    if let Err(e) = crate::map::serialize::IOMapSerialize::save_house_info(db).await {
        tracing::warn!("saveHouseInfo failed: {e}");
    }
    if let Err(e) = crate::map::serialize::IOMapSerialize::save_house_items(db).await {
        tracing::warn!("saveHouseItems failed: {e}");
    }
}

/// Return the first path in `candidates` that exists on disk, if any.
fn first_existing(candidates: &[&'static str]) -> Option<&'static str> {
    candidates.iter().copied().find(|p| std::path::Path::new(p).exists())
}

async fn load_motd_num() {
    use crate::config::StringConfig;
    use crate::db::{g_database, DatabaseEngine};

    let db = g_database();
    let motd = g_config().get_string(StringConfig::Motd).to_owned();

    let motd_num = match db.store_query("SELECT `value` FROM `server_config` WHERE `config` = 'motd_num'").await {
        Ok(Some(res)) => res.get_i64("value").unwrap_or(0) as u32,
        _ => {
            let _ = db.execute("INSERT IGNORE INTO `server_config` (`config`, `value`) VALUES ('motd_num', '0')").await;
            0u32
        }
    };

    let stored_hash = match db.store_query("SELECT `value` FROM `server_config` WHERE `config` = 'motd_hash'").await {
        Ok(Some(res)) => res.get_string("value").unwrap_or_default(),
        _ => {
            let _ = db.execute("INSERT IGNORE INTO `server_config` (`config`, `value`) VALUES ('motd_hash', '')").await;
            String::new()
        }
    };

    let current_hash = crate::db::login::sha1_hex(motd.as_bytes());
    let motd_num = if stored_hash != current_hash { motd_num + 1 } else { motd_num };

    crate::game::g_game().lock().unwrap().set_motd(motd_num, current_hash);
}

async fn load_players_record() {
    use crate::db::{g_database, DatabaseEngine};
    let db = g_database();
    let record = match db.store_query("SELECT `value` FROM `server_config` WHERE `config` = 'players_record'").await {
        Ok(Some(res)) => res.get_i64("value").unwrap_or(0) as u32,
        _ => {
            let _ = db.execute("INSERT IGNORE INTO `server_config` (`config`, `value`) VALUES ('players_record', '0')").await;
            0u32
        }
    };
    crate::game::g_game().lock().unwrap().set_players_record(record);
}

enum ParsedArguments {
    Run(Options),
    Exit(ExitStatus),
}

fn parse_arguments<I>(args: I) -> Result<ParsedArguments>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = Options::default();

    for arg in args {
        let arg = arg
            .into_string()
            .map_err(|value| anyhow!("non-UTF-8 argument: {:?}", value))?;

        if arg == "--help" {
            print_usage();
            return Ok(ParsedArguments::Exit(ExitStatus::Success));
        }

        if arg == "--version" {
            println!("tfs-rust {}", env!("CARGO_PKG_VERSION"));
            return Ok(ParsedArguments::Exit(ExitStatus::Success));
        }

        let Some((key, value)) = arg.split_once('=') else {
            continue;
        };

        match key {
            "--config" => {
                if value.is_empty() {
                    bail!("--config requires a value");
                }
                options.config_file = value.to_owned();
            }
            "--ip" => {
                if value.is_empty() {
                    bail!("--ip requires a value");
                }
                options.ip = Some(value.to_owned());
            }
            "--login-port" => {
                options.login_port = Some(parse_port(key, value)?);
            }
            "--game-port" => {
                options.game_port = Some(parse_port(key, value)?);
            }
            _ => {}
        }
    }

    Ok(ParsedArguments::Run(options))
}

fn parse_port(key: &str, value: &str) -> Result<u16> {
    if value.is_empty() {
        bail!("{key} requires a value");
    }

    value
        .parse::<u16>()
        .map_err(|source| anyhow!("{key} expects a valid u16 port: {source}"))
}

fn initialize_tracing() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .try_init()
        .map_err(|error| anyhow!("{error}"))?;
    Ok(())
}

fn print_usage() {
    eprintln!(
        "Usage:\n\
         \n\
         \t--config=$1\t\tAlternate configuration file path.\n\
         \t--ip=$1\t\t\tIP address of the server.\n\
         \t\t\t\tShould be equal to the global IP.\n\
         \t--login-port=$1\tPort for login server to listen on.\n\
         \t--game-port=$1\tPort for game server to listen on."
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_arguments, ExitStatus, ParsedArguments};

    #[test]
    fn parse_arguments_should_capture_supported_overrides() {
        let parsed = parse_arguments([
            "--config=config.lua".into(),
            "--ip=127.0.0.1".into(),
            "--login-port=7171".into(),
            "--game-port=7172".into(),
        ])
        .expect("arguments should parse");

        match parsed {
            ParsedArguments::Run(options) => {
                assert_eq!(options.config_file, "config.lua");
                assert_eq!(options.ip.as_deref(), Some("127.0.0.1"));
                assert_eq!(options.login_port, Some(7171));
                assert_eq!(options.game_port, Some(7172));
            }
            ParsedArguments::Exit(_) => panic!("expected run arguments"),
        }
    }

    #[test]
    fn parse_arguments_should_exit_on_help() {
        let parsed = parse_arguments(["--help".into()]).expect("help should parse");

        match parsed {
            ParsedArguments::Exit(status) => assert_eq!(status, ExitStatus::Success),
            ParsedArguments::Run(_) => panic!("expected early exit"),
        }
    }
}
