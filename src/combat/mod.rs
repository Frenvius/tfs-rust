#[allow(clippy::module_inception)]
pub mod combat;
pub mod condition;

pub use combat::{AreaCombat, Combat, CombatParams, MatrixArea};
pub use condition::{
    create_condition,
    Condition, ConditionAttributes, ConditionDamage, ConditionDrunk, ConditionGeneric,
    ConditionInvisible, ConditionLight, ConditionOutfit, ConditionRegeneration, ConditionSoul,
    ConditionSpeed,
};

/// CombatType_t — u16 bit flags.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum CombatType {
    #[default]
    None = 0,
    PhysicalDamage = 1 << 0,
    EnergyDamage = 1 << 1,
    EarthDamage = 1 << 2,
    FireDamage = 1 << 3,
    UndefinedDamage = 1 << 4,
    LifeDrain = 1 << 5,
    ManaDrain = 1 << 6,
    Healing = 1 << 7,
    DrownDamage = 1 << 8,
    IceDamage = 1 << 9,
    HolyDamage = 1 << 10,
    DeathDamage = 1 << 11,
}

pub const COMBAT_COUNT: usize = 12;

impl CombatType {
    /// Convert from an items element-type index (used by `ItemType::element_type`).
    /// Items store element type as an index into combat types: 2=energy, 3=earth,
    /// 4=fire, 10=ice, 11=holy, 12=death.  The formula is `1 << (index - 1)`.
    pub fn from_element_index(v: u8) -> Self {
        if v == 0 { return CombatType::None; }
        CombatType::from_u16(1u16 << (v as u16 - 1))
    }

    pub fn from_u16(v: u16) -> Self {
        match v {
            x if x == CombatType::PhysicalDamage as u16 => CombatType::PhysicalDamage,
            x if x == CombatType::EnergyDamage as u16 => CombatType::EnergyDamage,
            x if x == CombatType::EarthDamage as u16 => CombatType::EarthDamage,
            x if x == CombatType::FireDamage as u16 => CombatType::FireDamage,
            x if x == CombatType::UndefinedDamage as u16 => CombatType::UndefinedDamage,
            x if x == CombatType::LifeDrain as u16 => CombatType::LifeDrain,
            x if x == CombatType::ManaDrain as u16 => CombatType::ManaDrain,
            x if x == CombatType::Healing as u16 => CombatType::Healing,
            x if x == CombatType::DrownDamage as u16 => CombatType::DrownDamage,
            x if x == CombatType::IceDamage as u16 => CombatType::IceDamage,
            x if x == CombatType::HolyDamage as u16 => CombatType::HolyDamage,
            x if x == CombatType::DeathDamage as u16 => CombatType::DeathDamage,
            _ => CombatType::None,
        }
    }

    pub fn from_u32(v: u32) -> Self {
        Self::from_u16(v as u16)
    }
}

/// CombatOrigin
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CombatOrigin {
    #[default]
    None = 0,
    Condition = 1,
    Spell = 2,
    Melee = 3,
    Ranged = 4,
    Wand = 5,
}

/// BlockType_t
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlockType {
    #[default]
    None = 0,
    Defense = 1,
    Armor = 2,
    Immunity = 3,
}

/// CombatDamage — primary + secondary damage values with metadata.
#[derive(Debug, Clone, Copy, Default)]
pub struct CombatDamage {
    pub primary_type: CombatType,
    pub primary_value: i32,
    pub secondary_type: CombatType,
    pub secondary_value: i32,
    pub origin: CombatOrigin,
    pub block_type: BlockType,
    pub critical: bool,
    pub leeched: bool,
}

impl CombatDamage {
    pub fn new(combat_type: CombatType) -> Self {
        Self {
            primary_type: combat_type,
            ..Default::default()
        }
    }
}


/// formulaType_t
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormulaType {
    #[default]
    Undefined = 0,
    LevelMagic = 1,
    Skill = 2,
    Damage = 3,
}

/// CallBackParam_t
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallBackParam {
    LevelMagicValue = 0,
    SkillValue = 1,
    TargetTile = 2,
    TargetCreature = 3,
}

/// CombatParam_t
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatParam {
    Type = 0,
    Effect = 1,
    DistanceEffect = 2,
    BlockShield = 3,
    BlockArmor = 4,
    TargetCasterOrTopMost = 5,
    CreateItem = 6,
    Aggressive = 7,
    Dispel = 8,
    UseCharges = 9,
}

/// ReturnValue — result codes used throughout game logic.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnValue {
    NoError = 0,
    NotPossible,
    NotEnoughRoom,
    PlayerIsPzLocked,
    PlayerIsNotInvited,
    CannotThrow,
    ThereIsNoWay,
    DestinationOutOfReach,
    CreatureBlock,
    NotMoveable,
    DropTwoHandedItem,
    BothHandsNeedToBeFree,
    CanOnlyUseOneWeapon,
    NeedExchange,
    CannotBeDressed,
    PutThisObjectInYourHand,
    PutThisObjectInBothHands,
    TooFarAway,
    FirstGoDownStairs,
    FirstGoUpStairs,
    ContainerNotEnoughRoom,
    NotEnoughCapacity,
    CannotPickup,
    ThisIsImpossible,
    DepotIsFull,
    CreatureDoesNotExist,
    CannotUseThisObject,
    PlayerWithThisNameIsNotOnline,
    NotRequiredLevelToUseRune,
    YouAreAlreadyTrading,
    ThisPlayerIsAlreadyTrading,
    YouMayNotLogoutDuringAFight,
    DirectPlayerShoot,
    NotEnoughLevel,
    NotEnoughMagicLevel,
    NotEnoughMana,
    NotEnoughSoul,
    YouAreExhausted,
    YouCannotUseObjectsThatFast,
    PlayerIsNotReachable,
    CanOnlyUseThisRuneOnCreatures,
    ActionNotPermittedInProtectionZone,
    YouMayNotAttackThisPlayer,
    YouMayNotAttackAPersonInProtectionZone,
    YouMayNotAttackAPersonWhileInProtectionZone,
    YouMayNotAttackThisCreature,
    YouCanOnlyUseItOnCreatures,
    CreatureIsNotReachable,
    TurnSecureModeToAttackUnmarkedPlayers,
    YouNeedPremiumAccount,
    YouNeedToLearnThisSpell,
    YourVocationCannotUseThisSpell,
    YouNeedAWeaponToUseThisSpell,
    PlayerIsPzLockedLeavePvpZone,
    PlayerIsPzLockedEnterPvpZone,
    ActionNotPermittedInANoPvpZone,
    YouCannotLogoutHere,
    YouNeedAMagicItemToCastSpell,
    CannotConjureItemHere,
    YouNeedToSplitYourSpears,
    NameIsTooAmbiguous,
    CanOnlyUseOneShield,
    NoPartyMembersInRange,
    YouAreNotTheOwner,
    NoSuchRaidExists,
    AnotherRaidIsAlreadyExecuting,
    TradePlayerFarAway,
    YouDontOwnThisHouse,
    TradePlayerAlreadyOwnsAHouse,
    TradePlayerHighestBidder,
    YouCannotTradeThisHouse,
    YouDontHaveRequiredProfession,
    ItemCannotBeMovedThere,
    YouCannotUseThisBed,
}
