use std::collections::{HashSet, VecDeque};

use crate::combat::{BlockType, CombatType};
use crate::creatures::monsters::MonsterInfo;
use crate::creatures::{CreatureBase, CreatureId, CreatureType, Direction, RaceType};
use crate::map::Position;

pub const DESPAWN_RANGE: i32 = 2;
pub const DESPAWN_RADIUS: i32 = 50;

pub static MONSTER_AUTO_ID: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x40000000);

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetSearchType {
    #[default]
    Default = 0,
    Random = 1,
    AttackRange = 2,
    Nearest = 3,
}

pub struct Monster {
    pub base: CreatureBase,

    /// Key into the Monsters registry.
    pub monster_type: String,

    /// Snapshot of the MonsterType info at creation time (mirrors C++ mType->info access).
    pub mtype_info: MonsterInfo,

    /// Overrideable name (empty = use mtype_info defaults via getName()).
    pub name: String,
    /// Overrideable name description (empty = use mtype_info defaults).
    pub name_description: String,

    pub spawn_pos: Position,
    pub master_pos: Position,

    pub last_melee_attack: i64,
    pub last_walk_time: i64,

    pub attack_ticks: u32,
    pub target_ticks: u32,
    pub target_change_ticks: u32,
    pub defense_ticks: u32,
    pub yell_ticks: u32,

    pub min_combat_value: i32,
    pub max_combat_value: i32,
    pub target_change_cooldown: i32,
    pub challenge_focus_duration: i32,
    pub step_duration: i32,

    pub ignore_field_damage: bool,
    pub is_idle: bool,
    pub is_master_in_range: bool,
    pub random_stepping: bool,
    pub walking_to_spawn: bool,

    pub target_list: VecDeque<CreatureId>,
    pub friend_set: HashSet<CreatureId>,
}

impl Monster {
    pub fn new(monster_type: String, mtype_info: MonsterInfo, base: CreatureBase) -> Self {
        Self {
            base,
            monster_type,
            mtype_info,
            name: String::new(),
            name_description: String::new(),
            spawn_pos: Position::default(),
            master_pos: Position::default(),
            last_melee_attack: 0,
            last_walk_time: 0,
            attack_ticks: 0,
            target_ticks: 0,
            target_change_ticks: 0,
            defense_ticks: 0,
            yell_ticks: 0,
            min_combat_value: 0,
            max_combat_value: 0,
            target_change_cooldown: 0,
            challenge_focus_duration: 0,
            step_duration: 0,
            ignore_field_damage: false,
            is_idle: true,
            is_master_in_range: false,
            random_stepping: false,
            walking_to_spawn: false,
            target_list: VecDeque::new(),
            friend_set: HashSet::new(),
        }
    }

    pub fn allocate_id() -> CreatureId {
        MONSTER_AUTO_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// createMonster — port of Monster::createMonster(name) static factory.
    pub fn create_monster(name: &str) -> Option<Box<Monster>> {
        use crate::creatures::monsters::g_monsters;
        use crate::map::Position;

        let monsters = g_monsters();
        let mt = monsters.get_monster_type(name)?;

        let id = Monster::allocate_id();
        let mut base = CreatureBase::new(id, Position::default());
        base.health = mt.info.health;
        base.health_max = mt.info.health_max;
        base.base_speed = mt.info.base_speed;
        base.skull = mt.info.skull;
        base.current_outfit = mt.info.outfit;
        base.default_outfit = mt.info.outfit;
        base.hidden_health = mt.info.hidden_health;
        base.skill_loss = true;

        let monster = Box::new(Monster::new(
            mt.name.to_lowercase(),
            mt.info.clone(),
            base,
        ));
        Some(monster)
    }

    pub fn can_use_attack(&self, _pos: Position, _target_id: CreatureId) -> bool {
        false
    }

    pub fn can_use_spell(
        &self,
        _pos: Position,
        _target_pos: Position,
        _speed: u32,
        _interval: u32,
        _min_range: i32,
        _max_range: i32,
    ) -> bool {
        false
    }

    pub fn get_random_step(&self, _creature_pos: Position, _direction: &mut Direction) -> bool {
        false
    }

    pub fn get_dance_step(
        &self,
        _creature_pos: Position,
        _direction: &mut Direction,
        _keep_target: bool,
        _keep_distance: bool,
    ) -> bool {
        false
    }

    pub fn push_item(_item_id: u32) -> bool {
        false
    }

    pub fn push_items(_tile_pos: Position) {
    }

    pub fn push_creature(_creature_id: CreatureId) -> bool {
        false
    }

    pub fn push_creatures(_tile_pos: Position) {
    }

    pub fn update_look_direction(&mut self) {
    }

    pub fn update_target_list(&mut self) {
    }

    /// isFriend — port of Monster::isFriend from monster.cpp.
    ///
    /// Returns true if the given creature is in this monster's friend list,
    /// or if the monster is a summon and the creature is its master.
    pub fn is_friend(&self, creature_id: CreatureId) -> bool {
        if self.base.is_summon() {
            return self.base.master_id == Some(creature_id);
        }
        self.friend_set.contains(&creature_id)
    }

    pub fn is_opponent(&self, _creature_id: CreatureId) -> bool {
        false
    }

    pub fn get_name(&self) -> &str {
        if self.name.is_empty() {
            &self.monster_type
        } else {
            &self.name
        }
    }

    pub fn set_name(&mut self, name: String) {
        self.name = name;
    }

    pub fn get_name_description(&self) -> &str {
        if self.name_description.is_empty() {
            &self.monster_type
        } else {
            &self.name_description
        }
    }

    pub fn set_name_description(&mut self, name_description: String) {
        self.name_description = name_description;
    }

    pub fn get_description(&self) -> String {
        format!("{}.", self.get_name_description())
    }

    pub fn get_type(&self) -> CreatureType {
        CreatureType::Monster
    }

    pub fn get_master_pos(&self) -> Position {
        self.master_pos
    }

    pub fn set_master_pos(&mut self, pos: Position) {
        self.master_pos = pos;
    }

    pub fn get_race(&self) -> RaceType {
        self.mtype_info.race
    }

    pub fn get_armor(&self) -> i32 {
        self.mtype_info.armor
    }

    pub fn get_defense(&self) -> i32 {
        self.mtype_info.defense
    }

    pub fn is_pushable(&self) -> bool {
        self.mtype_info.pushable && self.base.base_speed != 0
    }

    pub fn is_attackable(&self) -> bool {
        self.mtype_info.is_attackable
    }

    /// In C++, canPushItems() checks if master (if a monster) has canPushItems,
    /// else falls back to own mType->info.canPushItems.
    pub fn can_push_items(&self, master_can_push_items: Option<bool>) -> bool {
        if let Some(master_val) = master_can_push_items {
            return master_val;
        }
        self.mtype_info.can_push_items
    }

    pub fn can_push_creatures(&self) -> bool {
        self.mtype_info.can_push_creatures
    }

    pub fn is_hostile(&self) -> bool {
        self.mtype_info.is_hostile
    }

    pub fn get_mana_cost(&self) -> u32 {
        self.mtype_info.mana_cost
    }

    pub fn get_damage_immunities(&self) -> u32 {
        self.mtype_info.damage_immunities
    }

    pub fn get_condition_immunities(&self) -> u32 {
        self.mtype_info.condition_immunities
    }

    pub fn get_lost_experience(&self) -> u64 {
        if self.base.skill_loss {
            self.mtype_info.experience
        } else {
            0
        }
    }

    pub fn get_look_corpse(&self) -> u16 {
        self.mtype_info.look_corpse
    }

    pub fn is_fleeing(&self) -> bool {
        !self.base.is_summon()
            && self.base.health <= self.mtype_info.run_away_health
            && self.challenge_focus_duration <= 0
    }

    pub fn is_target_nearby(&self) -> bool {
        self.step_duration >= 1
    }

    pub fn is_ignoring_field_damage(&self) -> bool {
        self.ignore_field_damage
    }

    pub fn get_combat_values(&self, min: &mut i32, max: &mut i32) -> bool {
        if self.min_combat_value == 0 && self.max_combat_value == 0 {
            return false;
        }
        *min = self.min_combat_value;
        *max = self.max_combat_value;
        true
    }

    pub fn has_extra_swing(&self) -> bool {
        self.last_melee_attack == 0
    }

    pub fn can_walk_on_field_type(&self, combat_type: CombatType) -> bool {
        match combat_type {
            CombatType::EnergyDamage => self.mtype_info.can_walk_on_energy,
            CombatType::FireDamage => self.mtype_info.can_walk_on_fire,
            CombatType::EarthDamage => self.mtype_info.can_walk_on_poison,
            _ => true,
        }
    }

    /// Matches isTarget() from monster.cpp.
    /// Caller must supply the target's zone (0=normal, 1=protection, etc.),
    /// and whether the target can be seen by this monster.
    pub fn is_target(
        &self,
        target_pos: Position,
        target_is_removed: bool,
        target_is_attackable: bool,
        target_zone: u8,
        target_can_see: bool,
    ) -> bool {
        if target_is_removed || !target_is_attackable {
            return false;
        }
        if target_zone == 1 {
            return false;
        }
        if !target_can_see {
            return false;
        }
        if target_pos.z != self.base.position.z {
            return false;
        }
        true
    }

    /// blockHit — verbatim port of Monster::blockHit from monster.cpp.
    ///
    /// First applies the base creature defense/armor reduction (Creature::blockHit),
    /// then applies the element modifier from elementMap.
    pub fn block_hit(
        &self,
        combat_type: CombatType,
        damage: &mut i32,
        check_defense: bool,
        check_armor: bool,
        _field: bool,
        _ignore_resistances: bool,
    ) -> BlockType {
        let block_type = creature_block_hit_base(
            self.mtype_info.defense,
            self.mtype_info.armor,
            damage,
            check_defense,
            check_armor,
        );

        if *damage != 0 {
            if let Some(&element_mod) = self.mtype_info.element_map.get(&combat_type) {
                if element_mod != 0 {
                    *damage =
                        (*damage as f64 * ((100 - element_mod) as f64 / 100.0)).round() as i32;
                    if *damage <= 0 {
                        *damage = 0;
                        return BlockType::Armor;
                    }
                }
            }
        }

        block_type
    }

    /// searchTarget — verbatim port of Monster::searchTarget from monster.cpp.
    ///
    /// `get_target_info` returns (pos, is_removed, is_attackable, zone_u8, can_see) for a creature id.
    /// `can_use_attack` returns true if this monster can attack the given creature from the given pos.
    /// `select_fn` performs the actual selection (calls selectTarget logic).
    pub fn search_target(
        &mut self,
        search_type: TargetSearchType,
        my_pos: Position,
        get_target_info: &impl Fn(CreatureId) -> Option<(Position, bool, bool, u8, bool)>,
        can_use_attack: &impl Fn(Position, CreatureId) -> bool,
        mut select_fn: impl FnMut(&mut Monster, CreatureId) -> bool,
    ) -> bool {
        let follow_id = self.base.follow_creature_id;

        let mut result_list: Vec<CreatureId> = Vec::new();

        for &creature_id in &self.target_list {
            if follow_id == Some(creature_id) {
                continue;
            }
            if let Some((pos, is_removed, is_attackable, zone, can_see)) =
                get_target_info(creature_id)
            {
                if !self.is_target(pos, is_removed, is_attackable, zone, can_see) {
                    continue;
                }
                if search_type == TargetSearchType::Random
                    || can_use_attack(my_pos, creature_id)
                {
                    result_list.push(creature_id);
                }
            }
        }

        match search_type {
            TargetSearchType::Nearest => {
                let target = if !result_list.is_empty() {
                    let mut best = result_list[0];
                    let mut min_range = if let Some((pos, ..)) = get_target_info(best) {
                        position_distance(my_pos, pos)
                    } else {
                        i32::MAX
                    };
                    for &id in &result_list[1..] {
                        if let Some((pos, ..)) = get_target_info(id) {
                            let d = position_distance(my_pos, pos);
                            if d < min_range {
                                best = id;
                                min_range = d;
                            }
                        }
                    }
                    Some(best)
                } else {
                    let mut best: Option<CreatureId> = None;
                    let mut min_range = i32::MAX;
                    let snapshot: Vec<CreatureId> = self.target_list.iter().copied().collect();
                    for id in snapshot {
                        if let Some((pos, is_removed, is_attackable, zone, can_see)) =
                            get_target_info(id)
                        {
                            if !self.is_target(pos, is_removed, is_attackable, zone, can_see) {
                                continue;
                            }
                            let d = position_distance(my_pos, pos);
                            if d < min_range {
                                best = Some(id);
                                min_range = d;
                            }
                        }
                    }
                    best
                };

                if let Some(id) = target {
                    if select_fn(self, id) {
                        return true;
                    }
                }
            }

            TargetSearchType::Default
            | TargetSearchType::AttackRange
            | TargetSearchType::Random => {
                if !result_list.is_empty() {
                    let idx = crate::util::uniform_random(0, result_list.len() as i64 - 1) as usize;
                    return select_fn(self, result_list[idx]);
                }

                if search_type == TargetSearchType::AttackRange {
                    return false;
                }
            }
        }

        let snapshot: Vec<CreatureId> = self.target_list.iter().copied().collect();
        for id in snapshot {
            if follow_id == Some(id) {
                continue;
            }
            if select_fn(self, id) {
                return true;
            }
        }
        false
    }

    /// selectTarget — verbatim port of Monster::selectTarget from monster.cpp.
    /// Live monster targeting is handled in game/tick.rs::monster_think.
    pub fn select_target(
        &mut self,
        target_id: CreatureId,
        target_pos: Position,
        target_is_removed: bool,
        target_is_attackable: bool,
        target_zone: u8,
        target_can_see: bool,
    ) -> bool {
        if !self.is_target(
            target_pos,
            target_is_removed,
            target_is_attackable,
            target_zone,
            target_can_see,
        ) {
            return false;
        }

        if !self.target_list.contains(&target_id) {
            return false;
        }

        true
    }

    #[allow(clippy::too_many_arguments)]
    pub fn challenge_creature(
        &mut self,
        creature_id: CreatureId,
        force: bool,
        target_pos: Position,
        target_is_removed: bool,
        target_is_attackable: bool,
        target_zone: u8,
        target_can_see: bool,
    ) -> bool {
        if self.base.is_summon() {
            return false;
        }

        if !self.mtype_info.is_challengeable && !force {
            return false;
        }

        if !self.is_target(
            target_pos,
            target_is_removed,
            target_is_attackable,
            target_zone,
            target_can_see,
        ) {
            return false;
        }

        if !self.target_list.contains(&creature_id) {
            return false;
        }

        true
    }

    pub fn on_think(&mut self, _interval: u32) {
    }

    /// onAttackedCreatureDisappear — port of Monster::onAttackedCreatureDisappear.
    pub fn on_attacked_creature_disappear(&mut self, _is_logout: bool) {
        self.attack_ticks = 0;
    }

    /// onAttackedCreatureChange — port of Monster::onAttackedCreatureChange
    /// (the virtual method in Creature that Monster does not override).
    /// Not overridden — actual logic is in the base class.
    pub fn on_attacked_creature_change(&mut self, _id: CreatureId) {
    }

    pub fn on_creature_appear(&mut self, _creature_id: CreatureId, _is_login: bool) {
    }

    pub fn on_remove_creature(&mut self, _creature_id: CreatureId, _is_logout: bool) {
    }

    pub fn on_creature_move(
        &mut self,
        _creature_id: CreatureId,
        _from_pos: Position,
        _to_pos: Position,
        _teleport: bool,
    ) {
    }

    pub fn on_creature_say(&mut self, _creature_id: CreatureId, _speak_type: u8, _text: &str) {
    }

    /// onThinkTarget — verbatim port of Monster::onThinkTarget from monster.cpp.
    pub fn on_think_target(&mut self, interval: u32) {
        if self.base.is_summon() {
            return;
        }

        if self.mtype_info.change_target_speed != 0 {
            let mut can_change_target = true;

            if self.challenge_focus_duration > 0 {
                self.challenge_focus_duration -= interval as i32;
                if self.challenge_focus_duration <= 0 {
                    self.challenge_focus_duration = 0;
                }
            }

            if self.target_change_cooldown > 0 {
                self.target_change_cooldown -= interval as i32;
                if self.target_change_cooldown <= 0 {
                    self.target_change_cooldown = 0;
                    self.target_change_ticks = self.mtype_info.change_target_speed;
                } else {
                    can_change_target = false;
                }
            }

            if can_change_target {
                self.target_change_ticks += interval;

                if self.target_change_ticks >= self.mtype_info.change_target_speed {
                    self.target_change_ticks = 0;
                    self.target_change_cooldown = self.mtype_info.change_target_speed as i32;

                    if self.challenge_focus_duration > 0 {
                        self.challenge_focus_duration = 0;
                    }

                    // Live target selection handled in game/tick.rs monster_think
                }
            }
        }
    }

    pub fn on_think_yell(&mut self, interval: u32) {
        if self.mtype_info.yell_speed_ticks == 0 {
            return;
        }

        self.yell_ticks += interval;
        if self.yell_ticks >= self.mtype_info.yell_speed_ticks {
            self.yell_ticks = 0;

            if !self.mtype_info.voice_vector.is_empty() {
            }
        }
    }

    pub fn on_think_defense(&mut self, interval: u32) {
        let mut reset_ticks = true;
        self.defense_ticks += interval;

        for spell in &self.mtype_info.defense_spells {
            if spell.speed > self.defense_ticks {
                reset_ticks = false;
                continue;
            }

            if self.defense_ticks % spell.speed >= interval {
                continue;
            }

            // Live defense spells handled in game/tick.rs monster_do_defense
        }

        if reset_ticks {
            self.defense_ticks = 0;
        }
    }

    pub fn do_attacking(&mut self, interval: u32) {
        if self.base.attacked_creature_id.is_none() {
            return;
        }

        self.attack_ticks += interval;
        let _reset_ticks = interval != 0;

        for _spell in &self.mtype_info.attack_spells {
        }

        if interval != 0 {
            self.attack_ticks = 0;
        }
    }

    /// updateIdleStatus — verbatim port of Monster::updateIdleStatus from monster.cpp.
    pub fn update_idle_status(&mut self) {
        let idle = if !self.base.is_summon() && self.target_list.is_empty() {
            !self.base.conditions.iter().any(|c| c.is_aggressive())
        } else {
            false
        };

        self.set_idle(idle);
    }

    /// setIdle — verbatim port of Monster::setIdle from monster.cpp.
    pub fn set_idle(&mut self, idle: bool) {
        if self.base.is_internal_removed || self.base.health <= 0 {
            return;
        }

        self.is_idle = idle;

        if idle {
            self.target_list.clear();
            self.friend_set.clear();
        }
    }

    pub fn get_idle_status(&self) -> bool {
        self.is_idle
    }

    pub fn add_target(&mut self, creature_id: CreatureId, push_front: bool) {
        assert_ne!(creature_id, self.base.id);
        if !self.target_list.contains(&creature_id) {
            if push_front {
                self.target_list.push_front(creature_id);
            } else {
                self.target_list.push_back(creature_id);
            }
        }
    }

    pub fn remove_target(&mut self, creature_id: CreatureId) {
        self.target_list.retain(|&id| id != creature_id);
    }

    pub fn add_friend(&mut self, creature_id: CreatureId) {
        assert_ne!(creature_id, self.base.id);
        self.friend_set.insert(creature_id);
    }

    pub fn remove_friend(&mut self, creature_id: CreatureId) {
        self.friend_set.remove(&creature_id);
    }

    pub fn clear_target_list(&mut self) {
        self.target_list.clear();
    }

    pub fn clear_friend_list(&mut self) {
        self.friend_set.clear();
    }

    pub fn on_add_condition(&mut self, _condition_type: u32) {
    }

    pub fn on_end_condition(&mut self, _condition_type: u32) {
    }

    pub fn set_normal_creature_light(&mut self) {
        self.base.internal_light = self.mtype_info.light;
    }

    /// drainHealth — verbatim port of Monster::drainHealth from monster.cpp.
    pub fn drain_health(&mut self, damage: i32) {
        self.base.health -= damage;

        if damage > 0 && self.random_stepping {
            self.ignore_field_damage = true;
        }
    }

    /// changeHealth — verbatim port of Monster::changeHealth from monster.cpp.
    pub fn change_health(&mut self, health_change: i32) {
        self.set_idle(false);
        self.base.health = (self.base.health + health_change)
            .max(0)
            .min(self.base.health_max);
    }

    pub fn get_next_step(
        &mut self,
        _direction: &mut Direction,
        _flags: &mut u32,
    ) -> bool {
        if !self.walking_to_spawn && (self.is_idle || self.base.health <= 0) {
            self.base.event_walk = 0;
            return false;
        }
        false
    }

    pub fn is_in_spawn_range(&self, _pos: Position) -> bool {
        true
    }

    pub fn get_path_search_params(&self) -> crate::creatures::FindPathParams {
        let mut fpp = crate::creatures::FindPathParams::default_follow();
        fpp.min_target_dist = 1;
        fpp.max_target_dist = self.mtype_info.target_distance;
        fpp
    }

    pub fn walk_to_spawn(&mut self) -> bool {
        if self.walking_to_spawn || self.target_list.is_empty() {
            return false;
        }
        false
    }

    pub fn on_walk(&mut self) {}

    pub fn on_walk_complete(&mut self) {
        if self.walking_to_spawn {
            self.walking_to_spawn = false;
            self.walk_to_spawn();
        }
    }

    /// onFollowCreatureComplete — verbatim port of Monster::onFollowCreatureComplete.
    pub fn on_follow_creature_complete(&mut self, creature_id: Option<CreatureId>) {
        if let Some(id) = creature_id {
            if let Some(pos) = self.target_list.iter().position(|&cid| cid == id) {
                self.target_list.remove(pos);

                if self.base.has_follow_path {
                    self.target_list.push_front(id);
                } else if !self.base.is_summon() {
                    self.target_list.push_back(id);
                }
                // summon case: C++ decrements reference counter; not needed in id-based design
            }
        }
    }

    pub fn get_distance_step(
        &self,
        _target_pos: Position,
        _direction: &mut Direction,
        _flee: bool,
    ) -> bool {
        false
    }

    pub fn use_cache_map(&self) -> bool {
        !self.random_stepping
    }

    pub fn can_walk_to(&self, _pos: Position, _direction: Direction) -> bool {
        false
    }

    pub fn can_see(&self, _pos: Position) -> bool {
        false
    }

    pub fn can_see_invisibility(&self) -> bool {
        use crate::combat::condition::ConditionType;
        (self.mtype_info.condition_immunities & ConditionType::Invisible as u32) != 0
    }

    pub fn add_list(&mut self) {
    }

    pub fn remove_list(&mut self) {
    }

    pub fn on_creature_enter(&mut self, _creature_id: CreatureId) {
    }

    pub fn on_creature_leave(&mut self, _creature_id: CreatureId) {
    }

    pub fn on_creature_found(&mut self, creature_id: CreatureId, push_front: bool) {
        if push_front {
            self.target_list.push_front(creature_id);
        } else {
            self.target_list.push_back(creature_id);
        }
        self.update_idle_status();
    }

    pub fn death(&mut self, _last_hit_creature: Option<CreatureId>) {
    }

    pub fn get_corpse(
        &self,
        _last_hit_creature: Option<CreatureId>,
        _most_damage_creature: Option<CreatureId>,
    ) -> Option<u32> {
        None
    }

    pub fn drop_loot(&mut self, _corpse_id: u32, _last_hit_creature: Option<CreatureId>) {
    }
}

fn position_distance(a: Position, b: Position) -> i32 {
    let dx = (a.x as i32 - b.x as i32).abs();
    let dy = (a.y as i32 - b.y as i32).abs();
    dx + dy
}

/// Creature::blockHit base logic — applies defense then armor reduction.
/// Matches the C++ Creature::blockHit formula for defense and armor rolls.
fn creature_block_hit_base(
    defense: i32,
    armor: i32,
    damage: &mut i32,
    check_defense: bool,
    check_armor: bool,
) -> BlockType {
    if *damage == 0 {
        return BlockType::None;
    }

    if *damage < 0 {
        *damage = 0;
        return BlockType::None;
    }

    if check_defense {
        let rand_val = crate::util::uniform_random(0, defense as i64) as i32;
        *damage -= rand_val;
        if *damage <= 0 {
            *damage = 0;
            return BlockType::Defense;
        }
    }

    if check_armor {
        let base_armor = armor;
        if base_armor > 1 {
            let rand_min = (base_armor / 2) as i64;
            let rand_max =
                base_armor as i64 - (base_armor as f64 / 4.0).floor() as i64;
            let armor_val = crate::util::uniform_random(rand_min, rand_max) as i32;
            *damage -= armor_val;
        } else if base_armor == 1 {
            *damage -= 1;
        }

        if *damage <= 0 {
            *damage = 0;
            return BlockType::Armor;
        }
    }

    BlockType::None
}
