use std::array;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static G_CONFIG: OnceLock<ConfigManager> = OnceLock::new();

pub fn g_config() -> &'static ConfigManager {
    G_CONFIG.get().expect("config not initialized")
}

pub(crate) fn init_config(config: ConfigManager) {
    G_CONFIG
        .set(config)
        .unwrap_or_else(|_| panic!("config already initialized"));
}

use mlua::{Lua, Value};
use thiserror::Error;

pub type ExperienceStage = (u32, u32, f32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanConfig {
    AllowChangeOutfit,
    OnePlayerOnAccount,
    AimbotHotkeyEnabled,
    RemoveRuneCharges,
    RemoveWeaponAmmo,
    RemoveWeaponCharges,
    RemovePotionCharges,
    PzLockSkullAttacker,
    ExperienceFromPlayers,
    FreePremium,
    ReplaceKickOnLogin,
    AllowClones,
    AllowWalkthrough,
    BindOnlyGlobalAddress,
    OptimizeDatabase,
    EmoteSpells,
    StaminaSystem,
    WarnUnsafeScripts,
    ConvertUnsafeScripts,
    ClassicEquipmentSlots,
    ClassicAttackSpeed,
    ScriptsConsoleLogs,
    ServerSaveNotifyMessage,
    ServerSaveCleanMap,
    ServerSaveClose,
    ServerSaveShutdown,
    OnlineOfflineCharlist,
    YellAllowPremium,
    PremiumToSendPrivate,
    ForceMonsterTypeLoad,
    DefaultWorldLight,
    HouseOwnedByAccount,
    LuaItemDesc,
    CleanProtectionZones,
    HouseDoorShowPrice,
    OnlyInvitedCanMoveHouseItems,
    RemoveOnDespawn,
    PlayerConsoleLogs,
}

impl BooleanConfig {
    const COUNT: usize = 38;

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringConfig {
    IpString,
    MapName,
    HouseRentPeriod,
    ServerName,
    OwnerName,
    OwnerEmail,
    Url,
    Location,
    Motd,
    WorldType,
    MysqlHost,
    MysqlUser,
    MysqlPass,
    MysqlDb,
    MysqlSock,
    DefaultPriority,
    MapAuthor,
    ConfigFile,
    ClientDatFile,
}

impl StringConfig {
    const COUNT: usize = 19;

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegerConfig {
    Ip,
    SqlPort,
    MaxPlayers,
    PzLocked,
    DefaultDespawnRange,
    DefaultDespawnRadius,
    DefaultWalkToSpawnRadius,
    RateExperience,
    RateSkill,
    RateLoot,
    RateMagic,
    RateSpawn,
    HousePrice,
    KillsToRed,
    KillsToBlack,
    MaxMessageBuffer,
    ActionsDelayInterval,
    ExActionsDelayInterval,
    KickAfterMinutes,
    ProtectionLevel,
    DeathLosePercent,
    StatusQueryTimeout,
    FragTime,
    WhiteSkullTime,
    GamePort,
    LoginPort,
    StatusPort,
    StairhopDelay,
    ExpFromPlayersLevelRange,
    MaxPacketsPerSecond,
    ServerSaveNotifyDuration,
    YellMinimumLevel,
    MinimumLevelToSendPrivate,
    VipFreeLimit,
    VipPremiumLimit,
    DepotFreeLimit,
    DepotPremiumLimit,
    ClientVersionMin,
    ClientVersionMax,
}

impl IntegerConfig {
    const COUNT: usize = 39;

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug)]
pub struct ConfigManager {
    strings: [String; StringConfig::COUNT],
    integers: [i32; IntegerConfig::COUNT],
    booleans: [bool; BooleanConfig::COUNT],
    experience_stages: Vec<ExperienceStage>,
    loaded: bool,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read `{path}`: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse `{path}`: {source}")]
    Parse {
        path: String,
        #[source]
        source: mlua::Error,
    },
    #[error("failed to resolve `{value}` as an IPv4 address")]
    InvalidIpAddress { value: String },
}

impl Default for ConfigManager {
    fn default() -> Self {
        let mut strings = array::from_fn(|_| String::new());
        strings[StringConfig::ConfigFile.index()] = String::from("config.lua");

        Self {
            strings,
            integers: [0; IntegerConfig::COUNT],
            booleans: [false; BooleanConfig::COUNT],
            experience_stages: Vec::new(),
            loaded: false,
        }
    }
}

impl ConfigManager {
    pub async fn load_from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let mut manager = Self::default();
        manager
            .set_string(
                StringConfig::ConfigFile,
                path.as_ref().to_string_lossy().into_owned(),
            )
            .expect("config file index is valid");
        manager.reload().await?;
        Ok(manager)
    }

    pub async fn reload(&mut self) -> Result<(), ConfigError> {
        let path = PathBuf::from(self.get_string(StringConfig::ConfigFile));
        let source =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|source| ConfigError::Read {
                    path: path.display().to_string(),
                    source,
                })?;

        let lua = Lua::new();
        lua.load(&source)
            .set_name(path.to_string_lossy().as_ref())
            .exec()
            .map_err(|source| ConfigError::Parse {
                path: path.display().to_string(),
                source,
            })?;

        if !self.loaded {
            self.booleans[BooleanConfig::BindOnlyGlobalAddress.index()] =
                get_global_boolean(&lua, "bindOnlyGlobalAddress", false)?;
            self.booleans[BooleanConfig::OptimizeDatabase.index()] =
                get_global_boolean(&lua, "startupDatabaseOptimization", true)?;

            if self.strings[StringConfig::IpString.index()].is_empty() {
                self.strings[StringConfig::IpString.index()] =
                    get_global_string(&lua, "ip", "127.0.0.1")?;
            }

            self.strings[StringConfig::MapName.index()] =
                get_global_string(&lua, "mapName", "forgotten")?;
            self.strings[StringConfig::MapAuthor.index()] =
                get_global_string(&lua, "mapAuthor", "Unknown")?;
            self.strings[StringConfig::HouseRentPeriod.index()] =
                get_global_string(&lua, "houseRentPeriod", "never")?;
            self.strings[StringConfig::MysqlHost.index()] =
                get_global_string(&lua, "mysqlHost", "127.0.0.1")?;
            self.strings[StringConfig::MysqlUser.index()] =
                get_global_string(&lua, "mysqlUser", "forgottenserver")?;
            self.strings[StringConfig::MysqlPass.index()] =
                get_global_string(&lua, "mysqlPass", "")?;
            self.strings[StringConfig::MysqlDb.index()] =
                get_global_string(&lua, "mysqlDatabase", "forgottenserver")?;
            self.strings[StringConfig::MysqlSock.index()] =
                get_global_string(&lua, "mysqlSock", "")?;
            self.strings[StringConfig::ClientDatFile.index()] =
                get_global_string(&lua, "clientDatFile", "")?;

            self.integers[IntegerConfig::Ip.index()] =
                resolve_ip_to_u32(self.get_string(StringConfig::IpString))
                    .map(|value| value as i32)?;
            self.integers[IntegerConfig::SqlPort.index()] =
                get_global_number(&lua, "mysqlPort", 3306)?;

            if self.integers[IntegerConfig::GamePort.index()] == 0 {
                self.integers[IntegerConfig::GamePort.index()] =
                    get_global_number(&lua, "gameProtocolPort", 7172)?;
            }

            if self.integers[IntegerConfig::LoginPort.index()] == 0 {
                self.integers[IntegerConfig::LoginPort.index()] =
                    get_global_number(&lua, "loginProtocolPort", 7171)?;
            }

            self.integers[IntegerConfig::StatusPort.index()] =
                get_global_number(&lua, "statusProtocolPort", 7171)?;
        }

        self.booleans[BooleanConfig::AllowChangeOutfit.index()] =
            get_global_boolean(&lua, "allowChangeOutfit", true)?;
        self.booleans[BooleanConfig::OnePlayerOnAccount.index()] =
            get_global_boolean(&lua, "onePlayerOnlinePerAccount", true)?;
        self.booleans[BooleanConfig::AimbotHotkeyEnabled.index()] =
            get_global_boolean(&lua, "hotkeyAimbotEnabled", true)?;
        self.booleans[BooleanConfig::RemoveRuneCharges.index()] =
            get_global_boolean(&lua, "removeChargesFromRunes", true)?;
        self.booleans[BooleanConfig::RemoveWeaponAmmo.index()] =
            get_global_boolean(&lua, "removeWeaponAmmunition", true)?;
        self.booleans[BooleanConfig::RemoveWeaponCharges.index()] =
            get_global_boolean(&lua, "removeWeaponCharges", true)?;
        self.booleans[BooleanConfig::RemovePotionCharges.index()] =
            get_global_boolean(&lua, "removeChargesFromPotions", true)?;
        self.booleans[BooleanConfig::PzLockSkullAttacker.index()] =
            get_global_boolean(&lua, "pzLockSkullAttacker", false)?;
        self.booleans[BooleanConfig::ExperienceFromPlayers.index()] =
            get_global_boolean(&lua, "experienceByKillingPlayers", false)?;
        self.booleans[BooleanConfig::FreePremium.index()] =
            get_global_boolean(&lua, "freePremium", false)?;
        self.booleans[BooleanConfig::ReplaceKickOnLogin.index()] =
            get_global_boolean(&lua, "replaceKickOnLogin", true)?;
        self.booleans[BooleanConfig::AllowClones.index()] =
            get_global_boolean(&lua, "allowClones", false)?;
        self.booleans[BooleanConfig::AllowWalkthrough.index()] =
            get_global_boolean(&lua, "allowWalkthrough", true)?;
        self.booleans[BooleanConfig::EmoteSpells.index()] =
            get_global_boolean(&lua, "emoteSpells", false)?;
        self.booleans[BooleanConfig::StaminaSystem.index()] =
            get_global_boolean(&lua, "staminaSystem", true)?;
        self.booleans[BooleanConfig::WarnUnsafeScripts.index()] =
            get_global_boolean(&lua, "warnUnsafeScripts", true)?;
        self.booleans[BooleanConfig::ConvertUnsafeScripts.index()] =
            get_global_boolean(&lua, "convertUnsafeScripts", true)?;
        self.booleans[BooleanConfig::ClassicEquipmentSlots.index()] =
            get_global_boolean(&lua, "classicEquipmentSlots", false)?;
        self.booleans[BooleanConfig::ClassicAttackSpeed.index()] =
            get_global_boolean(&lua, "classicAttackSpeed", false)?;
        self.booleans[BooleanConfig::ScriptsConsoleLogs.index()] =
            get_global_boolean(&lua, "showScriptsLogInConsole", true)?;
        self.booleans[BooleanConfig::ServerSaveNotifyMessage.index()] =
            get_global_boolean(&lua, "serverSaveNotifyMessage", true)?;
        self.booleans[BooleanConfig::ServerSaveCleanMap.index()] =
            get_global_boolean(&lua, "serverSaveCleanMap", false)?;
        self.booleans[BooleanConfig::ServerSaveClose.index()] =
            get_global_boolean(&lua, "serverSaveClose", false)?;
        self.booleans[BooleanConfig::ServerSaveShutdown.index()] =
            get_global_boolean(&lua, "serverSaveShutdown", true)?;
        self.booleans[BooleanConfig::OnlineOfflineCharlist.index()] =
            get_global_boolean(&lua, "showOnlineStatusInCharlist", false)?;
        self.booleans[BooleanConfig::YellAllowPremium.index()] =
            get_global_boolean(&lua, "yellAlwaysAllowPremium", false)?;
        self.booleans[BooleanConfig::PremiumToSendPrivate.index()] =
            get_global_boolean(&lua, "premiumToSendPrivate", false)?;
        self.booleans[BooleanConfig::ForceMonsterTypeLoad.index()] =
            get_global_boolean(&lua, "forceMonsterTypesOnLoad", true)?;
        self.booleans[BooleanConfig::DefaultWorldLight.index()] =
            get_global_boolean(&lua, "defaultWorldLight", true)?;
        self.booleans[BooleanConfig::HouseOwnedByAccount.index()] =
            get_global_boolean(&lua, "houseOwnedByAccount", false)?;
        self.booleans[BooleanConfig::LuaItemDesc.index()] =
            get_global_boolean(&lua, "luaItemDesc", false)?;
        self.booleans[BooleanConfig::CleanProtectionZones.index()] =
            get_global_boolean(&lua, "cleanProtectionZones", false)?;
        self.booleans[BooleanConfig::HouseDoorShowPrice.index()] =
            get_global_boolean(&lua, "houseDoorShowPrice", true)?;
        self.booleans[BooleanConfig::OnlyInvitedCanMoveHouseItems.index()] =
            get_global_boolean(&lua, "onlyInvitedCanMoveHouseItems", true)?;
        self.booleans[BooleanConfig::RemoveOnDespawn.index()] =
            get_global_boolean(&lua, "removeOnDespawn", true)?;
        self.booleans[BooleanConfig::PlayerConsoleLogs.index()] =
            get_global_boolean(&lua, "showPlayerLogInConsole", true)?;

        self.strings[StringConfig::DefaultPriority.index()] =
            get_global_string(&lua, "defaultPriority", "high")?;
        self.strings[StringConfig::ServerName.index()] = get_global_string(&lua, "serverName", "")?;
        self.strings[StringConfig::OwnerName.index()] = get_global_string(&lua, "ownerName", "")?;
        self.strings[StringConfig::OwnerEmail.index()] = get_global_string(&lua, "ownerEmail", "")?;
        self.strings[StringConfig::Url.index()] = get_global_string(&lua, "url", "")?;
        self.strings[StringConfig::Location.index()] = get_global_string(&lua, "location", "")?;
        self.strings[StringConfig::Motd.index()] = get_global_string(&lua, "motd", "")?;
        self.strings[StringConfig::WorldType.index()] =
            get_global_string(&lua, "worldType", "pvp")?;

        self.integers[IntegerConfig::MaxPlayers.index()] =
            get_global_number(&lua, "maxPlayers", 0)?;
        self.integers[IntegerConfig::PzLocked.index()] =
            get_global_number(&lua, "pzLocked", 60_000)?;
        self.integers[IntegerConfig::DefaultDespawnRange.index()] =
            get_global_number(&lua, "deSpawnRange", 2)?;
        self.integers[IntegerConfig::DefaultDespawnRadius.index()] =
            get_global_number(&lua, "deSpawnRadius", 50)?;
        self.integers[IntegerConfig::DefaultWalkToSpawnRadius.index()] =
            get_global_number(&lua, "walkToSpawnRadius", 15)?;
        self.integers[IntegerConfig::RateExperience.index()] =
            get_global_number(&lua, "rateExp", 5)?;
        self.integers[IntegerConfig::RateSkill.index()] = get_global_number(&lua, "rateSkill", 3)?;
        self.integers[IntegerConfig::RateLoot.index()] = get_global_number(&lua, "rateLoot", 2)?;
        self.integers[IntegerConfig::RateMagic.index()] = get_global_number(&lua, "rateMagic", 3)?;
        self.integers[IntegerConfig::RateSpawn.index()] = get_global_number(&lua, "rateSpawn", 1)?;
        self.integers[IntegerConfig::HousePrice.index()] =
            get_global_number(&lua, "housePriceEachSQM", 1_000)?;
        self.integers[IntegerConfig::KillsToRed.index()] =
            get_global_number(&lua, "killsToRedSkull", 3)?;
        self.integers[IntegerConfig::KillsToBlack.index()] =
            get_global_number(&lua, "killsToBlackSkull", 6)?;
        self.integers[IntegerConfig::ActionsDelayInterval.index()] =
            get_global_number(&lua, "timeBetweenActions", 200)?;
        self.integers[IntegerConfig::ExActionsDelayInterval.index()] =
            get_global_number(&lua, "timeBetweenExActions", 1_000)?;
        self.integers[IntegerConfig::MaxMessageBuffer.index()] =
            get_global_number(&lua, "maxMessageBuffer", 4)?;
        self.integers[IntegerConfig::KickAfterMinutes.index()] =
            get_global_number(&lua, "kickIdlePlayerAfterMinutes", 15)?;
        self.integers[IntegerConfig::ProtectionLevel.index()] =
            get_global_number(&lua, "protectionLevel", 1)?;
        self.integers[IntegerConfig::DeathLosePercent.index()] =
            get_global_number(&lua, "deathLosePercent", -1)?;
        self.integers[IntegerConfig::StatusQueryTimeout.index()] =
            get_global_number(&lua, "statusTimeout", 5_000)?;
        self.integers[IntegerConfig::FragTime.index()] =
            get_global_number(&lua, "timeToDecreaseFrags", 86_400)?;
        self.integers[IntegerConfig::WhiteSkullTime.index()] =
            get_global_number(&lua, "whiteSkullTime", 900)?;
        self.integers[IntegerConfig::StairhopDelay.index()] =
            get_global_number(&lua, "stairJumpExhaustion", 2_000)?;
        self.integers[IntegerConfig::ExpFromPlayersLevelRange.index()] =
            get_global_number(&lua, "expFromPlayersLevelRange", 75)?;
        self.integers[IntegerConfig::MaxPacketsPerSecond.index()] =
            get_global_number(&lua, "maxPacketsPerSecond", 25)?;
        self.integers[IntegerConfig::ServerSaveNotifyDuration.index()] =
            get_global_number(&lua, "serverSaveNotifyDuration", 5)?;
        self.integers[IntegerConfig::YellMinimumLevel.index()] =
            get_global_number(&lua, "yellMinimumLevel", 2)?;
        self.integers[IntegerConfig::MinimumLevelToSendPrivate.index()] =
            get_global_number(&lua, "minimumLevelToSendPrivate", 1)?;
        self.integers[IntegerConfig::VipFreeLimit.index()] =
            get_global_number(&lua, "vipFreeLimit", 20)?;
        self.integers[IntegerConfig::VipPremiumLimit.index()] =
            get_global_number(&lua, "vipPremiumLimit", 100)?;
        self.integers[IntegerConfig::DepotFreeLimit.index()] =
            get_global_number(&lua, "depotFreeLimit", 2_000)?;
        self.integers[IntegerConfig::DepotPremiumLimit.index()] =
            get_global_number(&lua, "depotPremiumLimit", 10_000)?;
        self.integers[IntegerConfig::ClientVersionMin.index()] =
            get_global_number(&lua, "clientVersionMin", 860)?;
        self.integers[IntegerConfig::ClientVersionMax.index()] =
            get_global_number(&lua, "clientVersionMax", 860)?;

        self.experience_stages = load_lua_stages(&lua)?;
        self.loaded = true;
        Ok(())
    }

    pub fn get_string(&self, key: StringConfig) -> &str {
        &self.strings[key.index()]
    }

    pub fn get_number(&self, key: IntegerConfig) -> i32 {
        self.integers[key.index()]
    }

    pub fn get_boolean(&self, key: BooleanConfig) -> bool {
        self.booleans[key.index()]
    }

    pub fn experience_stage(&self, level: u32) -> f32 {
        self.experience_stages
            .iter()
            .find(|(min, max, _)| level >= *min && level <= *max)
            .map(|(_, _, multiplier)| *multiplier)
            .unwrap_or(self.get_number(IntegerConfig::RateExperience) as f32)
    }

    #[allow(clippy::result_unit_err)]
    pub fn set_string(&mut self, key: StringConfig, value: String) -> Result<(), ()> {
        self.strings[key.index()] = value;
        Ok(())
    }

    #[allow(clippy::result_unit_err)]
    pub fn set_number(&mut self, key: IntegerConfig, value: i32) -> Result<(), ()> {
        self.integers[key.index()] = value;
        Ok(())
    }

    #[allow(clippy::result_unit_err)]
    pub fn set_boolean(&mut self, key: BooleanConfig, value: bool) -> Result<(), ()> {
        self.booleans[key.index()] = value;
        Ok(())
    }
}

fn get_global_string(
    lua: &Lua,
    identifier: &str,
    default_value: &str,
) -> Result<String, ConfigError> {
    match lua.globals().get::<Value>(identifier) {
        Ok(Value::String(value)) => Ok(value.to_string_lossy()),
        Ok(_) | Err(_) => Ok(String::from(default_value)),
    }
}

fn get_global_number(lua: &Lua, identifier: &str, default_value: i32) -> Result<i32, ConfigError> {
    match lua.globals().get::<Value>(identifier) {
        Ok(Value::Integer(value)) => Ok(value as i32),
        Ok(Value::Number(value)) => Ok(value as i32),
        Ok(_) | Err(_) => Ok(default_value),
    }
}

fn get_global_boolean(
    lua: &Lua,
    identifier: &str,
    default_value: bool,
) -> Result<bool, ConfigError> {
    match lua.globals().get::<Value>(identifier) {
        Ok(Value::Boolean(value)) => Ok(value),
        Ok(Value::String(value)) => Ok(boolean_string(&value.to_string_lossy())),
        Ok(_) | Err(_) => Ok(default_value),
    }
}

fn load_lua_stages(lua: &Lua) -> Result<Vec<ExperienceStage>, ConfigError> {
    let value = lua
        .globals()
        .get::<Value>("experienceStages")
        .unwrap_or(Value::Nil);
    let mut stages = Vec::new();

    if let Value::Table(table) = value {
        for value in table.sequence_values::<mlua::Table>() {
            let table = value.map_err(|source| ConfigError::Parse {
                path: String::from("experienceStages"),
                source,
            })?;

            let minlevel = table
                .get::<Option<u32>>("minlevel")
                .unwrap_or(Some(1))
                .unwrap_or(1);
            let maxlevel = table
                .get::<Option<u32>>("maxlevel")
                .unwrap_or(Some(u32::MAX))
                .unwrap_or(u32::MAX);
            let multiplier = table
                .get::<Option<f32>>("multiplier")
                .unwrap_or(Some(1.0))
                .unwrap_or(1.0);
            stages.push((minlevel, maxlevel, multiplier));
        }
        stages.sort_by_key(|(minlevel, maxlevel, _)| (*minlevel, *maxlevel));
    }

    Ok(stages)
}

fn boolean_string(value: &str) -> bool {
    value
        .chars()
        .next()
        .map(|ch| {
            let lower = ch.to_ascii_lowercase();
            lower != 'f' && lower != 'n' && lower != '0'
        })
        .unwrap_or(false)
}

fn resolve_ip_to_u32(value: &str) -> Result<u32, ConfigError> {
    if let Ok(IpAddr::V4(ip)) = value.parse::<IpAddr>() {
        return Ok(u32::from_le_bytes(ip.octets()));
    }

    let socket = (value, 0)
        .to_socket_addrs()
        .map_err(|_| ConfigError::InvalidIpAddress {
            value: value.to_owned(),
        })?
        .find_map(|addr| match addr.ip() {
            IpAddr::V4(ip) => Some(ip),
            IpAddr::V6(_) => None,
        })
        .ok_or_else(|| ConfigError::InvalidIpAddress {
            value: value.to_owned(),
        })?;

    Ok(u32::from_le_bytes(socket.octets()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{BooleanConfig, ConfigManager, IntegerConfig, StringConfig};

    #[tokio::test]
    async fn load_from_path_should_apply_values_and_defaults() {
        let path = std::env::temp_dir().join("tfs-rust-config.lua");
        fs::write(
            &path,
            r#"
ip = "127.0.0.1"
serverName = "Forgotten"
mysqlHost = "db.local"
mysqlUser = "root"
mysqlDatabase = "tfs"
allowClones = true
premiumToSendPrivate = "yes"
gameProtocolPort = 7272
experienceStages = {
  { minlevel = 1, maxlevel = 10, multiplier = 7.0 },
}
"#,
        )
        .expect("temp config should be writable");

        let config = ConfigManager::load_from_path(&path)
            .await
            .expect("config should load");

        assert_eq!(config.get_string(StringConfig::ServerName), "Forgotten");
        assert_eq!(config.get_string(StringConfig::MysqlHost), "db.local");
        assert_eq!(config.get_number(IntegerConfig::GamePort), 7272);
        assert_eq!(config.get_number(IntegerConfig::LoginPort), 7171);
        assert!(config.get_boolean(BooleanConfig::AllowClones));
        assert!(config.get_boolean(BooleanConfig::PremiumToSendPrivate));
        assert_eq!(config.experience_stage(5), 7.0);
        assert_eq!(config.get_string(StringConfig::MapName), "forgotten");

        fs::remove_file(path).expect("temp config should be removable");
    }
}
