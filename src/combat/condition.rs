
// ---------------------------------------------------------------------------
// Icon bit-mask constants (const.h)
// ---------------------------------------------------------------------------

pub const ICON_POISON: u32 = 1 << 0;
pub const ICON_BURN: u32 = 1 << 1;
pub const ICON_ENERGY: u32 = 1 << 2;
pub const ICON_DRUNK: u32 = 1 << 3;
pub const ICON_MANASHIELD: u32 = 1 << 4;
pub const ICON_PARALYZE: u32 = 1 << 5;
pub const ICON_HASTE: u32 = 1 << 6;
pub const ICON_SWORDS: u32 = 1 << 7;
pub const ICON_DROWNING: u32 = 1 << 8;
pub const ICON_FREEZING: u32 = 1 << 9;
pub const ICON_DAZZLED: u32 = 1 << 10;
pub const ICON_CURSED: u32 = 1 << 11;
pub const ICON_PARTY_BUFF: u32 = 1 << 12;
pub const ICON_PZBLOCK: u32 = 1 << 13;
pub const ICON_PZ: u32 = 1 << 14;
pub const ICON_BLEEDING: u32 = 1 << 15;
pub const ICON_LESSERHEX: u32 = 1 << 16;
pub const ICON_INTENSEHEX: u32 = 1 << 17;
pub const ICON_GREATERHEX: u32 = 1 << 18;

// ---------------------------------------------------------------------------
// Skill / stat index constants (enums.h)
// ---------------------------------------------------------------------------

pub const SKILL_FIST: usize = 0;
pub const SKILL_CLUB: usize = 1;
pub const SKILL_SWORD: usize = 2;
pub const SKILL_AXE: usize = 3;
pub const SKILL_DISTANCE: usize = 4;
pub const SKILL_SHIELD: usize = 5;
pub const SKILL_FISHING: usize = 6;
pub const SKILL_FIRST: usize = SKILL_FIST;
pub const SKILL_LAST: usize = SKILL_FISHING;
pub const SKILL_COUNT: usize = SKILL_LAST + 1;

pub const STAT_MAXHITPOINTS: usize = 0;
pub const STAT_MAXMANAPOINTS: usize = 1;
pub const STAT_SOULPOINTS: usize = 2;
pub const STAT_MAGICPOINTS: usize = 3;
pub const STAT_FIRST: usize = STAT_MAXHITPOINTS;
pub const STAT_LAST: usize = STAT_MAGICPOINTS;
pub const STAT_COUNT: usize = STAT_LAST + 1;

pub const SPECIALSKILL_CRITICALHITCHANCE: usize = 0;
pub const SPECIALSKILL_CRITICALHITAMOUNT: usize = 1;
pub const SPECIALSKILL_LIFELEECHCHANCE: usize = 2;
pub const SPECIALSKILL_LIFELEECHAMOUNT: usize = 3;
pub const SPECIALSKILL_MANALEECHCHANCE: usize = 4;
pub const SPECIALSKILL_MANALEECHAMOUNT: usize = 5;
pub const SPECIALSKILL_FIRST: usize = SPECIALSKILL_CRITICALHITCHANCE;
pub const SPECIALSKILL_LAST: usize = SPECIALSKILL_MANALEECHAMOUNT;
pub const SPECIALSKILL_COUNT: usize = SPECIALSKILL_LAST + 1;

// ---------------------------------------------------------------------------
// ConditionEffect — side effects returned by condition lifecycle methods.
// The caller (tick.rs / registrations.rs / combat.rs) applies these to the
// creature and broadcasts the resulting network packets.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ConditionEffect {
    ChangeSpeed(i32),
    SetDrunkenness(u8),
    SetCreatureLight(LightInfo),
    RevertCreatureLight,
    ChangeOutfit(OutfitInfo),
    RevertOutfit,
    SetVisible(bool),
    SetUseDefense(bool),
    AddSkills([i32; SKILL_COUNT]),
    AddSpecialSkills([i32; SPECIALSKILL_COUNT]),
    AddStats([i32; STAT_COUNT]),
    SendStats,
    SendSkills,
    SendIcons,
    ChangeSoul(i32),
}

// ---------------------------------------------------------------------------
// ConditionType_t — u32 bit flags (enums.h)
// Must remain a repr(u32) enum so existing `condition_type as u32` casts compile.
// ---------------------------------------------------------------------------

/// ConditionType_t — bit-flag enum matching enums.h verbatim.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConditionType {
    #[default]
    None = 0,
    Poison = 1 << 0,
    Fire = 1 << 1,
    Energy = 1 << 2,
    Bleeding = 1 << 3,
    Haste = 1 << 4,
    Paralyze = 1 << 5,
    Outfit = 1 << 6,
    Invisible = 1 << 7,
    Light = 1 << 8,
    ManaShield = 1 << 9,
    InFight = 1 << 10,
    Drunk = 1 << 11,
    ExhaustWeapon = 1 << 12,
    Regeneration = 1 << 13,
    Soul = 1 << 14,
    Drown = 1 << 15,
    Muted = 1 << 16,
    ChannelMutedTicks = 1 << 17,
    YellTicks = 1 << 18,
    Attributes = 1 << 19,
    Freezing = 1 << 20,
    Dazzled = 1 << 21,
    Cursed = 1 << 22,
    ExhaustCombat = 1 << 23,
    ExhaustHeal = 1 << 24,
    Pacified = 1 << 25,
}


impl ConditionType {
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::None,
            x if x == Self::Poison as u32 => Self::Poison,
            x if x == Self::Fire as u32 => Self::Fire,
            x if x == Self::Energy as u32 => Self::Energy,
            x if x == Self::Bleeding as u32 => Self::Bleeding,
            x if x == Self::Haste as u32 => Self::Haste,
            x if x == Self::Paralyze as u32 => Self::Paralyze,
            x if x == Self::Outfit as u32 => Self::Outfit,
            x if x == Self::Invisible as u32 => Self::Invisible,
            x if x == Self::Light as u32 => Self::Light,
            x if x == Self::ManaShield as u32 => Self::ManaShield,
            x if x == Self::InFight as u32 => Self::InFight,
            x if x == Self::Drunk as u32 => Self::Drunk,
            x if x == Self::ExhaustWeapon as u32 => Self::ExhaustWeapon,
            x if x == Self::Regeneration as u32 => Self::Regeneration,
            x if x == Self::Soul as u32 => Self::Soul,
            x if x == Self::Drown as u32 => Self::Drown,
            x if x == Self::Muted as u32 => Self::Muted,
            x if x == Self::ChannelMutedTicks as u32 => Self::ChannelMutedTicks,
            x if x == Self::YellTicks as u32 => Self::YellTicks,
            x if x == Self::Attributes as u32 => Self::Attributes,
            x if x == Self::Freezing as u32 => Self::Freezing,
            x if x == Self::Dazzled as u32 => Self::Dazzled,
            x if x == Self::Cursed as u32 => Self::Cursed,
            x if x == Self::ExhaustCombat as u32 => Self::ExhaustCombat,
            x if x == Self::ExhaustHeal as u32 => Self::ExhaustHeal,
            x if x == Self::Pacified as u32 => Self::Pacified,
            _ => Self::None,
        }
    }
}

/// ConditionId_t — matching enums.h verbatim.
#[repr(i8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConditionId {
    #[default]
    Default = -1,
    Combat = 0,
    Head = 1,
    Necklace = 2,
    Backpack = 3,
    Armor = 4,
    Right = 5,
    Left = 6,
    Legs = 7,
    Feet = 8,
    Ring = 9,
    Ammo = 10,
}

/// ConditionParam_t — matching enums.h verbatim.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionParam {
    Owner = 1,
    Ticks = 2,
    HealthGain = 4,
    HealthTicks = 5,
    ManaGain = 6,
    ManaTicks = 7,
    Delayed = 8,
    Speed = 9,
    LightLevel = 10,
    LightColor = 11,
    SoulGain = 12,
    SoulTicks = 13,
    MinValue = 14,
    MaxValue = 15,
    StartValue = 16,
    TickInterval = 17,
    ForceUpdate = 18,
    SkillMelee = 19,
    SkillFist = 20,
    SkillClub = 21,
    SkillSword = 22,
    SkillAxe = 23,
    SkillDistance = 24,
    SkillShield = 25,
    SkillFishing = 26,
    StatMaxHitPoints = 27,
    StatMaxManaPoints = 28,
    StatMagicPoints = 30,
    StatMaxHitPointsPercent = 31,
    StatMaxManaPointsPercent = 32,
    StatMagicPointsPercent = 34,
    PeriodicDamage = 35,
    SkillMeleePercent = 36,
    SkillFistPercent = 37,
    SkillClubPercent = 38,
    SkillSwordPercent = 39,
    SkillAxePercent = 40,
    SkillDistancePercent = 41,
    SkillShieldPercent = 42,
    SkillFishingPercent = 43,
    IsBuff = 44,
    SubId = 45,
    Field = 46,
    DisableDefense = 47,
    SpecialSkillCriticalHitChance = 48,
    SpecialSkillCriticalHitAmount = 49,
    SpecialSkillLifeLeechChance = 50,
    SpecialSkillLifeLeechAmount = 51,
    SpecialSkillManaLeechChance = 52,
    SpecialSkillManaLeechAmount = 53,
    IsAggressive = 54,
    Drunkenness = 55,
}

/// Core trait for all condition types — matches the C++ abstract `Condition` interface.
pub trait Condition: Send + Sync + std::fmt::Debug {
    fn get_type(&self) -> ConditionType;
    fn get_id(&self) -> ConditionId;
    fn get_sub_id(&self) -> u32;
    fn get_ticks(&self) -> i32;
    fn get_end_time(&self) -> i64;
    fn is_buff(&self) -> bool;
    fn is_aggressive(&self) -> bool;
    fn set_param(&mut self, param: ConditionParam, value: i32);
    fn get_param(&self, param: ConditionParam) -> i32;
    fn clone_condition(&self) -> Box<dyn Condition>;
    fn get_icons(&self) -> u32;

    /// start_condition — base: sets end_time = ticks + now if ticks > 0.
    /// Concrete overrides require Game integration for side effects.
    fn start_condition(&mut self) -> bool {
        if self.get_ticks() > 0 {
            let end_time = self.get_ticks() as i64 + crate::util::otsys_time();
            self.set_end_time(end_time);
        }
        true
    }

    /// execute_condition — base: decrements ticks, returns true while still active.
    fn execute_condition(&mut self, interval: i32) -> bool {
        if self.get_ticks() == -1 {
            return true;
        }
        let new_ticks = std::cmp::max(0, self.get_ticks() - interval);
        self.set_ticks_raw(new_ticks);
        self.get_end_time() >= crate::util::otsys_time()
    }

    /// end_condition — pure virtual in C++; concrete types handle their cleanup via Game.
    fn end_condition(&mut self) {}

    fn on_start(&mut self, _creature_base_speed: i32) -> Vec<ConditionEffect> { vec![ConditionEffect::SendIcons] }
    fn on_end(&self) -> Vec<ConditionEffect> { vec![ConditionEffect::SendIcons] }
    fn on_tick(&mut self, _interval: i32) -> Vec<ConditionEffect> { vec![] }
    fn on_add(&mut self, _other: &dyn Condition, _creature_base_speed: i32) -> Vec<ConditionEffect> { vec![] }

    fn get_outfit_info(&self) -> Option<OutfitInfo> { None }
    fn get_light_info(&self) -> Option<LightInfo> { None }
    fn get_drunkenness(&self) -> Option<u8> { None }
    fn get_soul_gain(&self) -> Option<u32> { None }
    fn get_soul_ticks(&self) -> Option<u32> { None }
    fn get_speed_delta(&self) -> Option<i32> { None }
    fn get_disable_defense(&self) -> bool { false }
    fn get_skills_snapshot(&self) -> Option<([i32; SKILL_COUNT], [i32; SKILL_COUNT])> { None }
    fn get_special_skills_snapshot(&self) -> Option<[i32; SPECIALSKILL_COUNT]> { None }
    fn get_stats_snapshot(&self) -> Option<([i32; STAT_COUNT], [i32; STAT_COUNT])> { None }

    /// tick_regen — called each creature think interval; returns (health_delta, mana_delta).
    /// Only ConditionRegeneration returns non-zero. Default is no-op.
    fn tick_regen(&mut self, _interval: i32) -> (i32, i32) { (0, 0) }

    /// add_condition — pure virtual; concrete types handle stacking / merge.
    fn add_condition(&mut self, _other: &dyn Condition) {}

    /// update_condition — base merge eligibility check (C++ Condition::updateCondition).
    fn update_condition(&self, other: &dyn Condition) -> bool {
        if self.get_type() != other.get_type() {
            return false;
        }
        if self.get_ticks() == -1 && other.get_ticks() > 0 {
            return false;
        }
        if other.get_ticks() >= 0
            && self.get_end_time() > (crate::util::otsys_time() + other.get_ticks() as i64)
        {
            return false;
        }
        true
    }

    /// is_persistent — non-virtual in C++; saved to DB only for default/combat conditions.
    fn is_persistent(&self) -> bool {
        if self.get_ticks() == -1 {
            return false;
        }
        matches!(
            self.get_id(),
            ConditionId::Default | ConditionId::Combat
        ) || self.get_type() == ConditionType::Muted
    }

    /// set_ticks — port of Condition::setTicks; updates end_time.
    fn set_ticks(&mut self, new_ticks: i32) {
        self.set_ticks_raw(new_ticks);
        let end = new_ticks as i64 + crate::util::otsys_time();
        self.set_end_time(end);
    }

    fn get_total_damage(&self) -> i32 {
        0
    }

    fn base_mut(&mut self) -> &mut ConditionBase;

    fn set_ticks_raw(&mut self, ticks: i32) {
        self.base_mut().ticks = ticks;
    }

    fn set_end_time(&mut self, end_time: i64) {
        self.base_mut().end_time = end_time;
    }
}

/// Shared base data for all condition concrete types.
#[derive(Debug, Clone)]
pub struct ConditionBase {
    pub id: ConditionId,
    pub condition_type: ConditionType,
    pub ticks: i32,
    pub end_time: i64,
    pub sub_id: u32,
    pub is_buff: bool,
    pub aggressive: bool,
    pub owner: u32,
}

impl ConditionBase {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        aggressive: bool,
    ) -> Self {
        let end_time = if ticks == -1 { i64::MAX } else { 0 };
        Self {
            id,
            condition_type,
            ticks,
            end_time,
            sub_id,
            is_buff: buff,
            aggressive,
            owner: 0,
        }
    }
}

/// ConditionGeneric — matches `class ConditionGeneric` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionGeneric {
    pub base: ConditionBase,
}

impl ConditionGeneric {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
        }
    }
}

impl Condition for ConditionGeneric {
    fn get_type(&self) -> ConditionType {
        self.base.condition_type
    }
    fn get_id(&self) -> ConditionId {
        self.base.id
    }
    fn get_sub_id(&self) -> u32 {
        self.base.sub_id
    }
    fn get_ticks(&self) -> i32 {
        self.base.ticks
    }
    fn get_end_time(&self) -> i64 {
        self.base.end_time
    }
    fn is_buff(&self) -> bool {
        self.base.is_buff
    }
    fn is_aggressive(&self) -> bool {
        self.base.aggressive
    }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            ConditionParam::IsAggressive => self.base.aggressive as i32,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> {
        Box::new(self.clone())
    }
    fn get_icons(&self) -> u32 {
        let icons: u32 = if self.base.is_buff { ICON_PARTY_BUFF } else { 0 };
        match self.base.condition_type {
            ConditionType::ManaShield => icons | ICON_MANASHIELD,
            ConditionType::InFight => icons | ICON_SWORDS,
            _ => icons,
        }
    }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }
}

/// IntervalInfo — matches `struct IntervalInfo` in condition.h.
#[derive(Debug, Clone, Copy, Default)]
pub struct IntervalInfo {
    pub time_left: i32,
    pub value: i32,
    pub interval: i32,
}

/// ConditionDamage — periodic-damage condition (fire, poison, energy, etc.).
#[derive(Debug, Clone)]
pub struct ConditionDamage {
    pub base: ConditionBase,
    pub damage_list: std::collections::VecDeque<IntervalInfo>,
    pub start_damage: i32,
    pub min_damage: i32,
    pub max_damage: i32,
    pub init_damage: i32,
    pub period_damage: i32,
    pub period_damage_tick: i32,
    pub tick_interval: i32,
    pub force_update: bool,
    pub delayed: bool,
    pub field: bool,
}

impl ConditionDamage {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        buff: bool,
        sub_id: u32,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, 0, buff, sub_id, aggressive),
            damage_list: std::collections::VecDeque::new(),
            start_damage: 0,
            min_damage: 0,
            max_damage: 0,
            init_damage: 0,
            period_damage: 0,
            period_damage_tick: 0,
            tick_interval: 2000,
            force_update: false,
            delayed: false,
            field: false,
        }
    }

    pub fn get_total_damage_value(&self) -> i32 {
        if !self.damage_list.is_empty() {
            self.damage_list.iter().map(|d| d.value).sum::<i32>().abs()
        } else {
            (self.min_damage + (self.max_damage - self.min_damage) / 2).abs()
        }
    }

    pub fn add_damage(&mut self, rounds: i32, time: i32, value: i32) -> bool {
        let time = time.max(2000);
        if rounds == -1 {
            self.period_damage = value;
            self.base.ticks = -1;
            self.tick_interval = time;
            return true;
        }
        if self.period_damage > 0 {
            return false;
        }
        for _ in 0..rounds {
            let info = IntervalInfo { interval: time, time_left: time, value };
            self.damage_list.push_back(info);
            if self.base.ticks != -1 {
                self.base.ticks += time;
            }
        }
        true
    }

    pub fn set_init_damage(&mut self, init_damage: i32) {
        self.init_damage = init_damage;
    }

    pub fn do_force_update(&self) -> bool {
        self.force_update
    }

    /// generate_damage_list — port of ConditionDamage::generateDamageList from condition.cpp.
    /// Distributes `amount` damage across a series of decreasing ticks.
    pub fn generate_damage_list(amount: i32, start: i32, list: &mut Vec<i32>) {
        let amount = amount.unsigned_abs() as i32;
        let mut sum: i32 = 0;
        for i in (1..=start).rev() {
            let n = start + 1 - i;
            let med = (n * amount) / start;
            loop {
                sum += i;
                list.push(i);
                let x1 = (1.0_f64 - ((sum + i) as f64 / med as f64)).abs();
                let x2 = (1.0_f64 - (sum as f64 / med as f64)).abs();
                if x1 >= x2 {
                    break;
                }
            }
        }
    }
}

impl Condition for ConditionDamage {
    fn get_type(&self) -> ConditionType {
        self.base.condition_type
    }
    fn get_id(&self) -> ConditionId {
        self.base.id
    }
    fn get_sub_id(&self) -> u32 {
        self.base.sub_id
    }
    fn get_ticks(&self) -> i32 {
        self.base.ticks
    }
    fn get_end_time(&self) -> i64 {
        self.base.end_time
    }
    fn is_buff(&self) -> bool {
        self.base.is_buff
    }
    fn is_aggressive(&self) -> bool {
        self.base.aggressive
    }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            ConditionParam::PeriodicDamage => self.period_damage = value,
            ConditionParam::ForceUpdate => self.force_update = value != 0,
            ConditionParam::Delayed => {}
            ConditionParam::MaxValue => self.max_damage = value.unsigned_abs() as i32,
            ConditionParam::MinValue => self.min_damage = value.unsigned_abs() as i32,
            ConditionParam::StartValue => self.start_damage = value.unsigned_abs() as i32,
            ConditionParam::TickInterval => self.tick_interval = value.unsigned_abs() as i32,
            ConditionParam::Field => {}
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            ConditionParam::IsAggressive => self.base.aggressive as i32,
            ConditionParam::PeriodicDamage => self.period_damage,
            ConditionParam::ForceUpdate => self.force_update as i32,
            ConditionParam::MaxValue => self.max_damage,
            ConditionParam::MinValue => self.min_damage,
            ConditionParam::StartValue => self.start_damage,
            ConditionParam::TickInterval => self.tick_interval,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> {
        Box::new(self.clone())
    }
    fn get_icons(&self) -> u32 {
        let icons: u32 = if self.base.is_buff { ICON_PARTY_BUFF } else { 0 };
        match self.base.condition_type {
            ConditionType::Fire => icons | ICON_BURN,
            ConditionType::Energy => icons | ICON_ENERGY,
            ConditionType::Drown => icons | ICON_DROWNING,
            ConditionType::Poison => icons | ICON_POISON,
            ConditionType::Freezing => icons | ICON_FREEZING,
            ConditionType::Dazzled => icons | ICON_DAZZLED,
            ConditionType::Cursed => icons | ICON_CURSED,
            _ => icons,
        }
    }
    fn get_total_damage(&self) -> i32 {
        self.get_total_damage_value()
    }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }
}

/// ConditionRegeneration — matches `class ConditionRegeneration` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionRegeneration {
    pub base: ConditionBase,
    pub internal_health_ticks: u32,
    pub internal_mana_ticks: u32,
    pub health_ticks: u32,
    pub health_gain: u32,
    pub mana_ticks: u32,
    pub mana_gain: u32,
}

impl ConditionRegeneration {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
            internal_health_ticks: 0,
            internal_mana_ticks: 0,
            health_ticks: 1000,
            health_gain: 0,
            mana_ticks: 1000,
            mana_gain: 0,
        }
    }
}

impl Condition for ConditionRegeneration {
    fn get_type(&self) -> ConditionType {
        self.base.condition_type
    }
    fn get_id(&self) -> ConditionId {
        self.base.id
    }
    fn get_sub_id(&self) -> u32 {
        self.base.sub_id
    }
    fn get_ticks(&self) -> i32 {
        self.base.ticks
    }
    fn get_end_time(&self) -> i64 {
        self.base.end_time
    }
    fn is_buff(&self) -> bool {
        self.base.is_buff
    }
    fn is_aggressive(&self) -> bool {
        self.base.aggressive
    }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            ConditionParam::HealthTicks => self.health_ticks = value as u32,
            ConditionParam::HealthGain => self.health_gain = value as u32,
            ConditionParam::ManaTicks => self.mana_ticks = value as u32,
            ConditionParam::ManaGain => self.mana_gain = value as u32,
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            ConditionParam::HealthTicks => self.health_ticks as i32,
            ConditionParam::HealthGain => self.health_gain as i32,
            ConditionParam::ManaTicks => self.mana_ticks as i32,
            ConditionParam::ManaGain => self.mana_gain as i32,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> {
        Box::new(self.clone())
    }
    fn get_icons(&self) -> u32 {
        if self.base.is_buff { ICON_PARTY_BUFF } else { 0 }
    }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }

    fn tick_regen(&mut self, interval: i32) -> (i32, i32) {
        let iv = interval as u32;
        let mut hdelta: i32 = 0;
        let mut mdelta: i32 = 0;

        self.internal_health_ticks = self.internal_health_ticks.saturating_add(iv);
        self.internal_mana_ticks = self.internal_mana_ticks.saturating_add(iv);

        if self.internal_health_ticks >= self.health_ticks {
            self.internal_health_ticks = 0;
            hdelta = self.health_gain as i32;
        }
        if self.internal_mana_ticks >= self.mana_ticks {
            self.internal_mana_ticks = 0;
            mdelta = self.mana_gain as i32;
        }
        (hdelta, mdelta)
    }
}

// ---------------------------------------------------------------------------
// OutfitInfo / LightInfo
// ---------------------------------------------------------------------------

/// OutfitInfo — matches Outfit_t in enums.h.
#[derive(Debug, Clone, Copy, Default)]
pub struct OutfitInfo {
    pub look_type: u16,
    pub look_type_ex: u16,
    pub look_head: u8,
    pub look_body: u8,
    pub look_legs: u8,
    pub look_feet: u8,
    pub look_addons: u8,
}

/// LightInfo — matches LightInfo in enums.h.
#[derive(Debug, Clone, Copy, Default)]
pub struct LightInfo {
    pub level: u8,
    pub color: u8,
}

impl LightInfo {
    pub fn new(level: u8, color: u8) -> Self {
        Self { level, color }
    }
}

// ---------------------------------------------------------------------------
// ConditionSoul
// ---------------------------------------------------------------------------

/// ConditionSoul — matches `class ConditionSoul` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionSoul {
    pub base: ConditionBase,
    pub internal_soul_ticks: u32,
    pub soul_ticks: u32,
    pub soul_gain: u32,
}

impl ConditionSoul {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
            internal_soul_ticks: 0,
            soul_ticks: 0,
            soul_gain: 0,
        }
    }

    pub fn execute_condition(&mut self, interval: i32) {
        self.internal_soul_ticks += interval as u32;
        if self.internal_soul_ticks >= self.soul_ticks {
            self.internal_soul_ticks = 0;
        }
    }

    pub fn add_condition(&mut self, other: &ConditionSoul) {
        if self.base.ticks == -1 && other.base.ticks > 0 {
            return;
        }
        if other.base.ticks >= 0 && self.base.end_time > other.base.ticks as i64 {
            return;
        }
        self.base.ticks = other.base.ticks;
        self.soul_ticks = other.soul_ticks;
        self.soul_gain = other.soul_gain;
    }
}
impl Condition for ConditionSoul {
    fn get_type(&self) -> ConditionType { self.base.condition_type }
    fn get_id(&self) -> ConditionId { self.base.id }
    fn get_sub_id(&self) -> u32 { self.base.sub_id }
    fn get_ticks(&self) -> i32 { self.base.ticks }
    fn get_end_time(&self) -> i64 { self.base.end_time }
    fn is_buff(&self) -> bool { self.base.is_buff }
    fn is_aggressive(&self) -> bool { self.base.aggressive }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            ConditionParam::SoulGain => self.soul_gain = value as u32,
            ConditionParam::SoulTicks => self.soul_ticks = value as u32,
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            ConditionParam::SoulGain => self.soul_gain as i32,
            ConditionParam::SoulTicks => self.soul_ticks as i32,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> { Box::new(self.clone()) }
    fn get_icons(&self) -> u32 { if self.base.is_buff { ICON_PARTY_BUFF } else { 0 } }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }
    fn get_soul_gain(&self) -> Option<u32> { Some(self.soul_gain) }
    fn get_soul_ticks(&self) -> Option<u32> { Some(self.soul_ticks) }

    fn on_tick(&mut self, interval: i32) -> Vec<ConditionEffect> {
        self.internal_soul_ticks += interval as u32;
        if self.soul_ticks > 0 && self.internal_soul_ticks >= self.soul_ticks {
            self.internal_soul_ticks = 0;
            return vec![ConditionEffect::ChangeSoul(self.soul_gain as i32), ConditionEffect::SendStats];
        }
        vec![]
    }

    fn on_add(&mut self, other: &dyn Condition, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        if !self.update_condition(other) {
            return vec![];
        }
        self.set_ticks(other.get_ticks());
        if let Some(sg) = other.get_soul_gain() { self.soul_gain = sg; }
        if let Some(st) = other.get_soul_ticks() { self.soul_ticks = st; }
        vec![]
    }
}

// ---------------------------------------------------------------------------
// ConditionInvisible
// ---------------------------------------------------------------------------

/// ConditionInvisible — matches `class ConditionInvisible` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionInvisible {
    pub base: ConditionBase,
}

impl ConditionInvisible {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
        }
    }

    pub fn start_condition(&self) -> bool {
        true
    }

    pub fn end_condition(&self) {
    }

    pub fn add_condition(&mut self, other_ticks: i32) {
        if self.base.ticks == -1 && other_ticks > 0 {
            return;
        }
        if other_ticks >= 0 && self.base.end_time > other_ticks as i64 {
            return;
        }
        self.base.ticks = other_ticks;
    }
}
impl Condition for ConditionInvisible {
    fn get_type(&self) -> ConditionType { self.base.condition_type }
    fn get_id(&self) -> ConditionId { self.base.id }
    fn get_sub_id(&self) -> u32 { self.base.sub_id }
    fn get_ticks(&self) -> i32 { self.base.ticks }
    fn get_end_time(&self) -> i64 { self.base.end_time }
    fn is_buff(&self) -> bool { self.base.is_buff }
    fn is_aggressive(&self) -> bool { self.base.aggressive }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> { Box::new(self.clone()) }
    fn get_icons(&self) -> u32 { if self.base.is_buff { ICON_PARTY_BUFF } else { 0 } }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }

    fn on_start(&mut self, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        vec![ConditionEffect::SetVisible(false), ConditionEffect::SendIcons]
    }

    fn on_end(&self) -> Vec<ConditionEffect> {
        vec![ConditionEffect::SetVisible(true), ConditionEffect::SendIcons]
    }

    fn on_add(&mut self, other: &dyn Condition, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        if self.base.ticks == -1 && other.get_ticks() > 0 {
            return vec![];
        }
        if other.get_ticks() >= 0 && self.base.end_time > other.get_ticks() as i64 {
            return vec![];
        }
        self.set_ticks(other.get_ticks());
        vec![]
    }
}

// ---------------------------------------------------------------------------
// ConditionSpeed
// ---------------------------------------------------------------------------

/// ConditionSpeed — matches `class ConditionSpeed` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionSpeed {
    pub base: ConditionBase,
    pub speed_delta: i32,
    pub mina: f32,
    pub minb: f32,
    pub maxa: f32,
    pub maxb: f32,
}

impl ConditionSpeed {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        change_speed: i32,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
            speed_delta: change_speed,
            mina: 0.0,
            minb: 0.0,
            maxa: 0.0,
            maxb: 0.0,
        }
    }

    pub fn set_formula_vars(&mut self, mina: f32, minb: f32, maxa: f32, maxb: f32) {
        self.mina = mina;
        self.minb = minb;
        self.maxa = maxa;
        self.maxb = maxb;
    }

    pub fn start_condition(&mut self, creature_base_speed: i32) -> bool {
        if self.speed_delta == 0 {
            let min = (creature_base_speed as f32).mul_add(self.mina, self.minb).round() as i64;
            let max = (creature_base_speed as f32).mul_add(self.maxa, self.maxb).round() as i64;
            self.speed_delta = crate::util::uniform_random(min, max) as i32;
        }
        true
    }

    pub fn end_condition(&self) {
    }

    pub fn add_condition(&mut self, other: &ConditionSpeed) {
        if self.base.condition_type != other.base.condition_type {
            return;
        }
        if self.base.ticks == -1 && other.base.ticks > 0 {
            return;
        }
        self.base.ticks = other.base.ticks;
        let old_speed_delta = self.speed_delta;
        self.speed_delta = other.speed_delta;
        self.mina = other.mina;
        self.maxa = other.maxa;
        self.minb = other.minb;
        self.maxb = other.maxb;
        let _ = old_speed_delta;
    }
}
impl Condition for ConditionSpeed {
    fn get_type(&self) -> ConditionType { self.base.condition_type }
    fn get_id(&self) -> ConditionId { self.base.id }
    fn get_sub_id(&self) -> u32 { self.base.sub_id }
    fn get_ticks(&self) -> i32 { self.base.ticks }
    fn get_end_time(&self) -> i64 { self.base.end_time }
    fn is_buff(&self) -> bool { self.base.is_buff }
    fn is_aggressive(&self) -> bool { self.base.aggressive }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            ConditionParam::Speed => {
                self.speed_delta = value;
                if value > 0 {
                    self.base.condition_type = ConditionType::Haste;
                } else {
                    self.base.condition_type = ConditionType::Paralyze;
                }
            }
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            ConditionParam::Speed => self.speed_delta,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> { Box::new(self.clone()) }
    fn get_icons(&self) -> u32 {
        let icons: u32 = if self.base.is_buff { ICON_PARTY_BUFF } else { 0 };
        match self.base.condition_type {
            ConditionType::Haste => icons | ICON_HASTE,
            ConditionType::Paralyze => icons | ICON_PARALYZE,
            _ => icons,
        }
    }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }
    fn get_speed_delta(&self) -> Option<i32> { Some(self.speed_delta) }

    fn on_start(&mut self, creature_base_speed: i32) -> Vec<ConditionEffect> {
        if self.speed_delta == 0 {
            let min = (creature_base_speed as f32).mul_add(self.mina, self.minb).round() as i64;
            let max = (creature_base_speed as f32).mul_add(self.maxa, self.maxb).round() as i64;
            self.speed_delta = crate::util::uniform_random(min, max) as i32;
        }
        vec![ConditionEffect::ChangeSpeed(self.speed_delta), ConditionEffect::SendIcons]
    }

    fn on_end(&self) -> Vec<ConditionEffect> {
        vec![ConditionEffect::ChangeSpeed(-self.speed_delta), ConditionEffect::SendIcons]
    }

    fn on_add(&mut self, other: &dyn Condition, creature_base_speed: i32) -> Vec<ConditionEffect> {
        if self.base.condition_type != other.get_type() {
            return vec![];
        }
        if self.base.ticks == -1 && other.get_ticks() > 0 {
            return vec![];
        }
        self.set_ticks(other.get_ticks());

        let old_speed_delta = self.speed_delta;
        self.speed_delta = other.get_speed_delta().unwrap_or(0);
        self.mina = 0.0;
        self.maxa = 0.0;
        self.minb = 0.0;
        self.maxb = 0.0;
        if self.speed_delta == 0 {
            let min = (creature_base_speed as f32).mul_add(self.mina, self.minb).round() as i64;
            let max = (creature_base_speed as f32).mul_add(self.maxa, self.maxb).round() as i64;
            self.speed_delta = crate::util::uniform_random(min, max) as i32;
        }
        let change = self.speed_delta - old_speed_delta;
        if change != 0 {
            vec![ConditionEffect::ChangeSpeed(change)]
        } else {
            vec![]
        }
    }
}

// ---------------------------------------------------------------------------
// ConditionOutfit
// ---------------------------------------------------------------------------

/// ConditionOutfit — matches `class ConditionOutfit` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionOutfit {
    pub base: ConditionBase,
    pub outfit: OutfitInfo,
}

impl ConditionOutfit {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
            outfit: OutfitInfo::default(),
        }
    }

    pub fn set_outfit(&mut self, outfit: OutfitInfo) {
        self.outfit = outfit;
    }

    pub fn start_condition(&self) {
    }

    pub fn end_condition(&self) {
    }

    pub fn add_condition(&mut self, other: &ConditionOutfit) {
        if self.base.ticks == -1 && other.base.ticks > 0 {
            return;
        }
        if other.base.ticks >= 0 && self.base.end_time > other.base.ticks as i64 {
            return;
        }
        self.base.ticks = other.base.ticks;
        self.outfit = other.outfit;
    }
}
impl Condition for ConditionOutfit {
    fn get_type(&self) -> ConditionType { self.base.condition_type }
    fn get_id(&self) -> ConditionId { self.base.id }
    fn get_sub_id(&self) -> u32 { self.base.sub_id }
    fn get_ticks(&self) -> i32 { self.base.ticks }
    fn get_end_time(&self) -> i64 { self.base.end_time }
    fn is_buff(&self) -> bool { self.base.is_buff }
    fn is_aggressive(&self) -> bool { self.base.aggressive }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> { Box::new(self.clone()) }
    fn get_icons(&self) -> u32 { if self.base.is_buff { ICON_PARTY_BUFF } else { 0 } }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }
    fn get_outfit_info(&self) -> Option<OutfitInfo> { Some(self.outfit) }

    fn on_start(&mut self, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        vec![ConditionEffect::ChangeOutfit(self.outfit), ConditionEffect::SendIcons]
    }

    fn on_end(&self) -> Vec<ConditionEffect> {
        vec![ConditionEffect::RevertOutfit, ConditionEffect::SendIcons]
    }

    fn on_add(&mut self, other: &dyn Condition, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        if !self.update_condition(other) {
            return vec![];
        }
        self.set_ticks(other.get_ticks());
        if let Some(o) = other.get_outfit_info() {
            self.outfit = o;
        }
        vec![ConditionEffect::ChangeOutfit(self.outfit)]
    }
}

// ---------------------------------------------------------------------------
// ConditionLight
// ---------------------------------------------------------------------------

/// ConditionLight — matches `class ConditionLight` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionLight {
    pub base: ConditionBase,
    pub light_info: LightInfo,
    pub internal_light_ticks: u32,
    pub light_change_interval: u32,
}

impl ConditionLight {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        light_level: u8,
        light_color: u8,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
            light_info: LightInfo::new(light_level, light_color),
            internal_light_ticks: 0,
            light_change_interval: 0,
        }
    }

    pub fn start_condition(&mut self) {
        self.internal_light_ticks = 0;
        self.light_change_interval = if self.light_info.level > 0 {
            self.base.ticks as u32 / self.light_info.level as u32
        } else {
            0
        };
    }

    pub fn execute_condition(&mut self, interval: i32) {
        self.internal_light_ticks += interval as u32;
        if self.internal_light_ticks >= self.light_change_interval {
            self.internal_light_ticks = 0;
        }
    }

    pub fn end_condition(&self) {
    }

    pub fn add_condition(&mut self, other: &ConditionLight) {
        if self.base.ticks == -1 && other.base.ticks > 0 {
            return;
        }
        if other.base.ticks >= 0 && self.base.end_time > other.base.ticks as i64 {
            return;
        }
        self.base.ticks = other.base.ticks;
        self.light_info.level = other.light_info.level;
        self.light_info.color = other.light_info.color;
        self.light_change_interval = if self.light_info.level > 0 {
            self.base.ticks as u32 / self.light_info.level as u32
        } else {
            0
        };
        self.internal_light_ticks = 0;
    }
}
impl Condition for ConditionLight {
    fn get_type(&self) -> ConditionType { self.base.condition_type }
    fn get_id(&self) -> ConditionId { self.base.id }
    fn get_sub_id(&self) -> u32 { self.base.sub_id }
    fn get_ticks(&self) -> i32 { self.base.ticks }
    fn get_end_time(&self) -> i64 { self.base.end_time }
    fn is_buff(&self) -> bool { self.base.is_buff }
    fn is_aggressive(&self) -> bool { self.base.aggressive }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            ConditionParam::LightLevel => self.light_info.level = (value as u8).max(1),
            ConditionParam::LightColor => self.light_info.color = value as u8,
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            ConditionParam::LightLevel => self.light_info.level as i32,
            ConditionParam::LightColor => self.light_info.color as i32,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> { Box::new(self.clone()) }
    fn get_icons(&self) -> u32 { if self.base.is_buff { ICON_PARTY_BUFF } else { 0 } }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }
    fn get_light_info(&self) -> Option<LightInfo> { Some(self.light_info) }

    fn on_start(&mut self, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        self.internal_light_ticks = 0;
        self.light_change_interval = if self.light_info.level > 0 {
            self.base.ticks as u32 / self.light_info.level as u32
        } else {
            0
        };
        vec![ConditionEffect::SetCreatureLight(self.light_info), ConditionEffect::SendIcons]
    }

    fn on_end(&self) -> Vec<ConditionEffect> {
        vec![ConditionEffect::RevertCreatureLight, ConditionEffect::SendIcons]
    }

    fn on_tick(&mut self, interval: i32) -> Vec<ConditionEffect> {
        self.internal_light_ticks += interval as u32;
        if self.light_change_interval > 0 && self.internal_light_ticks >= self.light_change_interval {
            self.internal_light_ticks = 0;
            if self.light_info.level > 0 {
                self.light_info.level -= 1;
                return vec![ConditionEffect::SetCreatureLight(self.light_info)];
            }
        }
        vec![]
    }

    fn on_add(&mut self, other: &dyn Condition, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        if !self.update_condition(other) {
            return vec![];
        }
        self.set_ticks(other.get_ticks());
        if let Some(li) = other.get_light_info() {
            self.light_info.level = li.level;
            self.light_info.color = li.color;
        }
        self.light_change_interval = if self.light_info.level > 0 {
            self.base.ticks as u32 / self.light_info.level as u32
        } else {
            0
        };
        self.internal_light_ticks = 0;
        vec![ConditionEffect::SetCreatureLight(self.light_info)]
    }
}

// ---------------------------------------------------------------------------
// ConditionDrunk
// ---------------------------------------------------------------------------

/// ConditionDrunk — matches `class ConditionDrunk` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionDrunk {
    pub base: ConditionBase,
    pub drunkenness: u8,
}

impl ConditionDrunk {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        drunkenness: u8,
        aggressive: bool,
    ) -> Self {
        let drunkenness = if drunkenness != 0 { drunkenness } else { 25 };
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
            drunkenness,
        }
    }

    pub fn start_condition(&self) {
    }

    pub fn end_condition(&self) {
    }

    pub fn add_condition(&mut self, other: &ConditionDrunk) {
        if other.drunkenness <= self.drunkenness {
            return;
        }
        self.base.ticks = other.base.ticks;
        self.drunkenness = other.drunkenness;
    }
}
impl Condition for ConditionDrunk {
    fn get_type(&self) -> ConditionType { self.base.condition_type }
    fn get_id(&self) -> ConditionId { self.base.id }
    fn get_sub_id(&self) -> u32 { self.base.sub_id }
    fn get_ticks(&self) -> i32 { self.base.ticks }
    fn get_end_time(&self) -> i64 { self.base.end_time }
    fn is_buff(&self) -> bool { self.base.is_buff }
    fn is_aggressive(&self) -> bool { self.base.aggressive }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            ConditionParam::Drunkenness => self.drunkenness = value as u8,
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            ConditionParam::Drunkenness => self.drunkenness as i32,
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> { Box::new(self.clone()) }
    fn get_icons(&self) -> u32 { ICON_DRUNK }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }
    fn get_drunkenness(&self) -> Option<u8> { Some(self.drunkenness) }

    fn on_start(&mut self, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        vec![ConditionEffect::SetDrunkenness(self.drunkenness), ConditionEffect::SendIcons]
    }

    fn on_end(&self) -> Vec<ConditionEffect> {
        vec![ConditionEffect::SetDrunkenness(0), ConditionEffect::SendIcons]
    }

    fn on_add(&mut self, other: &dyn Condition, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        let other_drunk = other.get_drunkenness().unwrap_or(0);
        if other_drunk <= self.drunkenness {
            return vec![];
        }
        self.set_ticks(other.get_ticks());
        self.drunkenness = other_drunk;
        vec![ConditionEffect::SetDrunkenness(self.drunkenness)]
    }
}

// ---------------------------------------------------------------------------
// ConditionAttributes
// ---------------------------------------------------------------------------

/// ConditionAttributes — matches `class ConditionAttributes` in condition.h.
#[derive(Debug, Clone)]
pub struct ConditionAttributes {
    pub base: ConditionBase,
    pub skills: [i32; SKILL_COUNT],
    pub skills_percent: [i32; SKILL_COUNT],
    pub special_skills: [i32; SPECIALSKILL_COUNT],
    pub stats: [i32; STAT_COUNT],
    pub stats_percent: [i32; STAT_COUNT],
    pub current_skill: usize,
    pub current_special_skill: usize,
    pub current_stat: usize,
    pub disable_defense: bool,
}

impl ConditionAttributes {
    pub fn new(
        id: ConditionId,
        condition_type: ConditionType,
        ticks: i32,
        buff: bool,
        sub_id: u32,
        aggressive: bool,
    ) -> Self {
        Self {
            base: ConditionBase::new(id, condition_type, ticks, buff, sub_id, aggressive),
            skills: [0; SKILL_COUNT],
            skills_percent: [0; SKILL_COUNT],
            special_skills: [0; SPECIALSKILL_COUNT],
            stats: [0; STAT_COUNT],
            stats_percent: [0; STAT_COUNT],
            current_skill: 0,
            current_special_skill: 0,
            current_stat: 0,
            disable_defense: false,
        }
    }

    pub fn start_condition(&self) {
    }

    pub fn end_condition(&self) {
    }

    pub fn add_condition(&mut self, other: &ConditionAttributes) {
        if self.base.ticks == -1 && other.base.ticks > 0 {
            return;
        }
        if other.base.ticks >= 0 && self.base.end_time > other.base.ticks as i64 {
            return;
        }
        self.base.ticks = other.base.ticks;
        self.skills = other.skills;
        self.special_skills = other.special_skills;
        self.skills_percent = other.skills_percent;
        self.stats = other.stats;
        self.stats_percent = other.stats_percent;
        self.disable_defense = other.disable_defense;
    }
}
impl Condition for ConditionAttributes {
    fn get_type(&self) -> ConditionType { self.base.condition_type }
    fn get_id(&self) -> ConditionId { self.base.id }
    fn get_sub_id(&self) -> u32 { self.base.sub_id }
    fn get_ticks(&self) -> i32 { self.base.ticks }
    fn get_end_time(&self) -> i64 { self.base.end_time }
    fn is_buff(&self) -> bool { self.base.is_buff }
    fn is_aggressive(&self) -> bool { self.base.aggressive }
    fn set_param(&mut self, param: ConditionParam, value: i32) {
        match param {
            ConditionParam::Owner => self.base.owner = value as u32,
            ConditionParam::Ticks => self.base.ticks = value,
            ConditionParam::IsBuff => self.base.is_buff = value != 0,
            ConditionParam::SubId => self.base.sub_id = value as u32,
            ConditionParam::IsAggressive => self.base.aggressive = value != 0,
            ConditionParam::SkillMelee => {
                self.skills[SKILL_CLUB] = value;
                self.skills[SKILL_AXE] = value;
                self.skills[SKILL_SWORD] = value;
            }
            ConditionParam::SkillMeleePercent => {
                self.skills_percent[SKILL_CLUB] = value;
                self.skills_percent[SKILL_AXE] = value;
                self.skills_percent[SKILL_SWORD] = value;
            }
            ConditionParam::SkillFist => self.skills[SKILL_FIST] = value,
            ConditionParam::SkillFistPercent => self.skills_percent[SKILL_FIST] = value,
            ConditionParam::SkillClub => self.skills[SKILL_CLUB] = value,
            ConditionParam::SkillClubPercent => self.skills_percent[SKILL_CLUB] = value,
            ConditionParam::SkillSword => self.skills[SKILL_SWORD] = value,
            ConditionParam::SkillSwordPercent => self.skills_percent[SKILL_SWORD] = value,
            ConditionParam::SkillAxe => self.skills[SKILL_AXE] = value,
            ConditionParam::SkillAxePercent => self.skills_percent[SKILL_AXE] = value,
            ConditionParam::SkillDistance => self.skills[SKILL_DISTANCE] = value,
            ConditionParam::SkillDistancePercent => self.skills_percent[SKILL_DISTANCE] = value,
            ConditionParam::SkillShield => self.skills[SKILL_SHIELD] = value,
            ConditionParam::SkillShieldPercent => self.skills_percent[SKILL_SHIELD] = value,
            ConditionParam::SkillFishing => self.skills[SKILL_FISHING] = value,
            ConditionParam::SkillFishingPercent => self.skills_percent[SKILL_FISHING] = value,
            ConditionParam::SpecialSkillCriticalHitChance => {
                self.special_skills[SPECIALSKILL_CRITICALHITCHANCE] = value;
            }
            ConditionParam::SpecialSkillCriticalHitAmount => {
                self.special_skills[SPECIALSKILL_CRITICALHITAMOUNT] = value;
            }
            ConditionParam::SpecialSkillLifeLeechChance => {
                self.special_skills[SPECIALSKILL_LIFELEECHCHANCE] = value;
            }
            ConditionParam::SpecialSkillLifeLeechAmount => {
                self.special_skills[SPECIALSKILL_LIFELEECHAMOUNT] = value;
            }
            ConditionParam::SpecialSkillManaLeechChance => {
                self.special_skills[SPECIALSKILL_MANALEECHCHANCE] = value;
            }
            ConditionParam::SpecialSkillManaLeechAmount => {
                self.special_skills[SPECIALSKILL_MANALEECHAMOUNT] = value;
            }
            ConditionParam::StatMaxHitPoints => self.stats[STAT_MAXHITPOINTS] = value,
            ConditionParam::StatMaxManaPoints => self.stats[STAT_MAXMANAPOINTS] = value,
            ConditionParam::StatMagicPoints => self.stats[STAT_MAGICPOINTS] = value,
            ConditionParam::StatMaxHitPointsPercent => {
                self.stats_percent[STAT_MAXHITPOINTS] = value.max(0);
            }
            ConditionParam::StatMaxManaPointsPercent => {
                self.stats_percent[STAT_MAXMANAPOINTS] = value.max(0);
            }
            ConditionParam::StatMagicPointsPercent => {
                self.stats_percent[STAT_MAGICPOINTS] = value.max(0);
            }
            ConditionParam::DisableDefense => self.disable_defense = value != 0,
            _ => {}
        }
    }
    fn get_param(&self, param: ConditionParam) -> i32 {
        match param {
            ConditionParam::Owner => self.base.owner as i32,
            ConditionParam::Ticks => self.base.ticks,
            ConditionParam::IsBuff => self.base.is_buff as i32,
            ConditionParam::SubId => self.base.sub_id as i32,
            ConditionParam::SkillFist => self.skills[SKILL_FIST],
            ConditionParam::SkillFistPercent => self.skills_percent[SKILL_FIST],
            ConditionParam::SkillClub => self.skills[SKILL_CLUB],
            ConditionParam::SkillClubPercent => self.skills_percent[SKILL_CLUB],
            ConditionParam::SkillSword => self.skills[SKILL_SWORD],
            ConditionParam::SkillSwordPercent => self.skills_percent[SKILL_SWORD],
            ConditionParam::SkillAxe => self.skills[SKILL_AXE],
            ConditionParam::SkillAxePercent => self.skills_percent[SKILL_AXE],
            ConditionParam::SkillDistance => self.skills[SKILL_DISTANCE],
            ConditionParam::SkillDistancePercent => self.skills_percent[SKILL_DISTANCE],
            ConditionParam::SkillShield => self.skills[SKILL_SHIELD],
            ConditionParam::SkillShieldPercent => self.skills_percent[SKILL_SHIELD],
            ConditionParam::SkillFishing => self.skills[SKILL_FISHING],
            ConditionParam::SkillFishingPercent => self.skills_percent[SKILL_FISHING],
            ConditionParam::StatMaxHitPoints => self.stats[STAT_MAXHITPOINTS],
            ConditionParam::StatMaxManaPoints => self.stats[STAT_MAXMANAPOINTS],
            ConditionParam::StatMagicPoints => self.stats[STAT_MAGICPOINTS],
            ConditionParam::StatMaxHitPointsPercent => self.stats_percent[STAT_MAXHITPOINTS],
            ConditionParam::StatMaxManaPointsPercent => self.stats_percent[STAT_MAXMANAPOINTS],
            ConditionParam::StatMagicPointsPercent => self.stats_percent[STAT_MAGICPOINTS],
            ConditionParam::DisableDefense => self.disable_defense as i32,
            ConditionParam::SpecialSkillCriticalHitChance => {
                self.special_skills[SPECIALSKILL_CRITICALHITCHANCE]
            }
            ConditionParam::SpecialSkillCriticalHitAmount => {
                self.special_skills[SPECIALSKILL_CRITICALHITAMOUNT]
            }
            ConditionParam::SpecialSkillLifeLeechChance => {
                self.special_skills[SPECIALSKILL_LIFELEECHCHANCE]
            }
            ConditionParam::SpecialSkillLifeLeechAmount => {
                self.special_skills[SPECIALSKILL_LIFELEECHAMOUNT]
            }
            ConditionParam::SpecialSkillManaLeechChance => {
                self.special_skills[SPECIALSKILL_MANALEECHCHANCE]
            }
            ConditionParam::SpecialSkillManaLeechAmount => {
                self.special_skills[SPECIALSKILL_MANALEECHAMOUNT]
            }
            _ => i32::MAX,
        }
    }
    fn clone_condition(&self) -> Box<dyn Condition> { Box::new(self.clone()) }
    fn get_icons(&self) -> u32 {
        let icons: u32 = if self.base.is_buff { ICON_PARTY_BUFF } else { 0 };
        match self.base.condition_type {
            ConditionType::ManaShield => icons | ICON_MANASHIELD,
            ConditionType::InFight => icons | ICON_SWORDS,
            _ => icons,
        }
    }
    fn base_mut(&mut self) -> &mut ConditionBase { &mut self.base }
    fn get_disable_defense(&self) -> bool { self.disable_defense }
    fn get_skills_snapshot(&self) -> Option<([i32; SKILL_COUNT], [i32; SKILL_COUNT])> {
        Some((self.skills, self.skills_percent))
    }
    fn get_special_skills_snapshot(&self) -> Option<[i32; SPECIALSKILL_COUNT]> {
        Some(self.special_skills)
    }
    fn get_stats_snapshot(&self) -> Option<([i32; STAT_COUNT], [i32; STAT_COUNT])> {
        Some((self.stats, self.stats_percent))
    }

    fn on_start(&mut self, _creature_base_speed: i32) -> Vec<ConditionEffect> {
        let mut effects = Vec::new();
        if self.disable_defense {
            effects.push(ConditionEffect::SetUseDefense(false));
        }
        effects.push(ConditionEffect::AddSkills(self.skills));
        effects.push(ConditionEffect::AddSpecialSkills(self.special_skills));
        effects.push(ConditionEffect::AddStats(self.stats));
        effects.push(ConditionEffect::SendStats);
        effects.push(ConditionEffect::SendSkills);
        effects.push(ConditionEffect::SendIcons);
        effects
    }

    fn on_end(&self) -> Vec<ConditionEffect> {
        let mut effects = Vec::new();
        let mut neg_skills = [0i32; SKILL_COUNT];
        for (dst, src) in neg_skills.iter_mut().zip(self.skills.iter()) {
            *dst = -*src;
        }
        let mut neg_special = [0i32; SPECIALSKILL_COUNT];
        for (dst, src) in neg_special.iter_mut().zip(self.special_skills.iter()) {
            *dst = -*src;
        }
        let mut neg_stats = [0i32; STAT_COUNT];
        for (dst, src) in neg_stats.iter_mut().zip(self.stats.iter()) {
            *dst = -*src;
        }
        effects.push(ConditionEffect::AddSkills(neg_skills));
        effects.push(ConditionEffect::AddSpecialSkills(neg_special));
        effects.push(ConditionEffect::AddStats(neg_stats));
        if self.disable_defense {
            effects.push(ConditionEffect::SetUseDefense(true));
        }
        effects.push(ConditionEffect::SendStats);
        effects.push(ConditionEffect::SendSkills);
        effects.push(ConditionEffect::SendIcons);
        effects
    }

    fn on_add(&mut self, other: &dyn Condition, creature_base_speed: i32) -> Vec<ConditionEffect> {
        if !self.update_condition(other) {
            return vec![];
        }
        self.set_ticks(other.get_ticks());
        let mut effects = self.on_end().to_vec();
        if let Some((sk, skp)) = other.get_skills_snapshot() {
            self.skills = sk;
            self.skills_percent = skp;
        }
        if let Some(ss) = other.get_special_skills_snapshot() {
            self.special_skills = ss;
        }
        if let Some((st, stp)) = other.get_stats_snapshot() {
            self.stats = st;
            self.stats_percent = stp;
        }
        self.disable_defense = other.get_disable_defense();
        effects.extend(self.on_start(creature_base_speed));
        effects
    }
}

/// create_condition — port of Condition::createCondition(id, type, ticks, param, buff, subId, aggressive).
pub fn create_condition(
    id: ConditionId,
    condition_type: ConditionType,
    ticks: i32,
    param: i32,
    buff: bool,
    sub_id: u32,
    aggressive: bool,
) -> Option<Box<dyn Condition>> {
    match condition_type {
        ConditionType::Poison
        | ConditionType::Fire
        | ConditionType::Energy
        | ConditionType::Drown
        | ConditionType::Freezing
        | ConditionType::Dazzled
        | ConditionType::Cursed
        | ConditionType::Bleeding => Some(Box::new(ConditionDamage::new(id, condition_type, buff, sub_id, aggressive))),
        ConditionType::Haste | ConditionType::Paralyze => {
            Some(Box::new(ConditionSpeed::new(id, condition_type, ticks, buff, sub_id, param, aggressive)))
        }
        ConditionType::Invisible => {
            Some(Box::new(ConditionInvisible::new(id, condition_type, ticks, buff, sub_id, aggressive)))
        }
        ConditionType::Outfit => {
            Some(Box::new(ConditionOutfit::new(id, condition_type, ticks, buff, sub_id, aggressive)))
        }
        ConditionType::Light => {
            let level = (param & 0xFF) as u8;
            let color = ((param & 0xFF00) >> 8) as u8;
            Some(Box::new(ConditionLight::new(id, condition_type, ticks, buff, sub_id, level, color, aggressive)))
        }
        ConditionType::Regeneration => {
            Some(Box::new(ConditionRegeneration::new(id, condition_type, ticks, buff, sub_id, aggressive)))
        }
        ConditionType::Soul => {
            Some(Box::new(ConditionSoul::new(id, condition_type, ticks, buff, sub_id, aggressive)))
        }
        ConditionType::Attributes => {
            Some(Box::new(ConditionAttributes::new(id, condition_type, ticks, buff, sub_id, aggressive)))
        }
        ConditionType::Drunk => {
            Some(Box::new(ConditionDrunk::new(id, condition_type, ticks, buff, sub_id, param as u8, aggressive)))
        }
        ConditionType::InFight
        | ConditionType::ExhaustWeapon
        | ConditionType::ExhaustCombat
        | ConditionType::ExhaustHeal
        | ConditionType::Muted
        | ConditionType::ChannelMutedTicks
        | ConditionType::YellTicks
        | ConditionType::Pacified
        | ConditionType::ManaShield => {
            Some(Box::new(ConditionGeneric::new(id, condition_type, ticks, buff, sub_id, aggressive)))
        }
        _ => None,
    }
}

pub fn add_condition_to_creature(
    conditions: &mut Vec<Box<dyn Condition>>,
    mut new_cond: Box<dyn Condition>,
    creature_base_speed: i32,
) -> Vec<ConditionEffect> {
    let ctype = new_cond.get_type();
    let cid = new_cond.get_id();
    let csub = new_cond.get_sub_id();

    if let Some(existing) = conditions.iter_mut().find(|c| {
        c.get_type() == ctype && c.get_id() == cid && c.get_sub_id() == csub
    }) {
        return existing.on_add(new_cond.as_ref(), creature_base_speed);
    }

    new_cond.start_condition();
    let effects = new_cond.on_start(creature_base_speed);
    conditions.push(new_cond);
    effects
}
