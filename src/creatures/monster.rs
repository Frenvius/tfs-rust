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

    // ── Unported C++ methods (stubs — need real implementation) ────────────

    pub fn get_random_step(&self, _creature_pos: Position, _direction: &mut Direction) -> bool {
        // C++ shuffles 4/8 dirs, finds first walkable via canWalkTo. Used for idle wander.
        false
    }

    pub fn get_dance_step(
        &self,
        _creature_pos: Position,
        _direction: &mut Direction,
        _keep_target: bool,
        _keep_distance: bool,
    ) -> bool {
        // C++ complex circling/strafing at attack range. Used by staticAttackChance.
        false
    }

    pub fn push_item(_item_id: u32) -> bool {
        // C++ tries 8 adjacent tiles to push a blocking item.
        false
    }

    pub fn push_items(_tile_pos: Position) {
        // C++ iterates moveable blocking items on tile, pushes up to 20.
    }

    pub fn push_creature(_creature_id: CreatureId) -> bool {
        // C++ tries 4 cardinal dirs to push a blocking creature.
        false
    }

    pub fn push_creatures(_tile_pos: Position) {
        // C++ kills monsters that can't be pushed.
    }

    pub fn update_look_direction(&mut self) {
        // C++ rotates monster to face its attack target. Called on every doAttacking pass.
    }

    pub fn is_in_spawn_range(&self, _pos: Position) -> bool {
        // C++ checks despawnRadius + despawnRange (z-distance). Called per-tick from onThink.
        true
    }

    pub fn get_distance_step(
        &self,
        _target_pos: Position,
        _direction: &mut Direction,
        _flee: bool,
    ) -> bool {
        // C++ 300-line quadrant escape logic for ranged monsters.
        false
    }

    pub fn can_walk_to(&self, _pos: Position, _direction: Direction) -> bool {
        // C++ checks isInSpawnRange + getTopVisibleCreature + tile queryAdd.
        false
    }

    pub fn walk_to_spawn(&mut self) -> bool {
        // C++ pathfinds back to masterPos when target list is empty.
        false
    }

    pub fn on_walk_complete(&mut self) {
        // C++ continues multi-step walk_to_spawn path.
        if self.walking_to_spawn {
            self.walking_to_spawn = false;
        }
    }

    pub fn on_creature_appear(&mut self, _creature_id: CreatureId, _is_login: bool) {
        // C++ adds appearing creature to friend/target list via onCreatureEnter. Fires Lua event.
    }

    pub fn on_remove_creature(&mut self, _creature_id: CreatureId, _is_logout: bool) {
        // C++ removes creature from friend/target lists via onCreatureLeave. Fires Lua event.
    }

    pub fn on_creature_move(
        &mut self,
        _creature_id: CreatureId,
        _from_pos: Position,
        _to_pos: Position,
        _teleport: bool,
    ) {
        // C++ updates friend/target lists on viewport enter/leave. Fires Lua event.
    }

    pub fn on_creature_say(&mut self, _creature_id: CreatureId, _speak_type: u8, _text: &str) {
        // C++ fires Lua creatureSayEvent per-monster.
    }

    pub fn on_creature_enter(&mut self, _creature_id: CreatureId) {
        // C++ sets isMasterInRange, calls onCreatureFound (add to friend/target list).
    }

    pub fn on_creature_leave(&mut self, _creature_id: CreatureId) {
        // C++ clears isMasterInRange, removes from lists, triggers walkToSpawn if empty.
    }

    pub fn on_add_condition(&mut self, _condition_type: u32) {
        // C++ calls updateIdleStatus so monster won't idle while burning/poisoned.
    }

    pub fn on_end_condition(&mut self, _condition_type: u32) {
        // C++ clears ignoreFieldDamage for fire/energy/poison, calls updateIdleStatus.
    }

    pub fn drop_loot(&mut self, _corpse_id: u32, _last_hit_creature: Option<CreatureId>) {
        // C++ fires Lua onDropLoot event for scripted loot customization.
    }

    // ── Ported methods ──────────────────────────────────────────────────────

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

    /// onAttackedCreatureDisappear — port of Monster::onAttackedCreatureDisappear.
    pub fn on_attacked_creature_disappear(&mut self, _is_logout: bool) {
        self.attack_ticks = 0;
    }

    /// onAttackedCreatureChange — port of Monster::onAttackedCreatureChange
    /// (the virtual method in Creature that Monster does not override).
    /// Not overridden — actual logic is in the base class.
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

    pub fn get_path_search_params(&self) -> crate::creatures::FindPathParams {
        let mut fpp = crate::creatures::FindPathParams::default_follow();
        fpp.min_target_dist = 1;
        fpp.max_target_dist = self.mtype_info.target_distance;
        fpp
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

    pub fn use_cache_map(&self) -> bool {
        !self.random_stepping
    }

    pub fn can_see_invisibility(&self) -> bool {
        use crate::combat::condition::ConditionType;
        (self.mtype_info.condition_immunities & ConditionType::Invisible as u32) != 0
    }

    pub fn on_creature_found(&mut self, creature_id: CreatureId, push_front: bool) {
        if push_front {
            self.target_list.push_front(creature_id);
        } else {
            self.target_list.push_back(creature_id);
        }
        self.update_idle_status();
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

// ── Monster AI functions (moved from game/tick.rs) ──────────────────────────

use crate::creatures::EVENT_CREATURE_THINK_INTERVAL;
use crate::game::g_game;
#[allow(unused_imports)]
use crate::net::game_protocol::send_packet_to_player;
#[allow(unused_imports)]
use crate::net::output_message::OutputMessage;

pub fn step_direction_toward(from: Position, to: Position) -> Option<Direction> {
    let dx = to.x as i32 - from.x as i32;
    let dy = to.y as i32 - from.y as i32;
    if dx == 0 && dy == 0 { return None; }
    Some(match (dx.signum(), dy.signum()) {
        (0,  -1) => Direction::North,
        (0,   1) => Direction::South,
        (1,   0) => Direction::East,
        (-1,  0) => Direction::West,
        (1,  -1) => Direction::NorthEast,
        (1,   1) => Direction::SouthEast,
        (-1, -1) => Direction::NorthWest,
        (-1,  1) => Direction::SouthWest,
        _        => return None,
    })
}

pub fn step_duration_ms(base_speed: u32) -> i64 {
    if base_speed == 0 { return i64::MAX; }
    (150_000_u64 / base_speed as u64) as i64
}

pub fn monster_walk(creature_id: u32) {
    use crate::net::game_protocol::broadcast_creature_move;
    use crate::util::otsys_time;

    // Collect: pos, target pos, base_speed, last_walk_time, fleeing, target_distance.
    let (pos, target_pos, base_speed, last_walk_time, fleeing, target_distance) = {
        let game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature(creature_id) else { return };
        let Some(monster) = creature.as_monster() else { return };
        let Some(target_id) = creature.base().attacked_creature_id else { return };
        let Some(tc) = game.get_creature(target_id) else { return };
        let pos = creature.position();
        let target_pos = tc.position();
        let base_speed = monster.base.base_speed;
        let last_walk_time = monster.last_walk_time;
        let fleeing = monster.is_fleeing();
        let target_distance = monster.mtype_info.target_distance.max(1);
        (pos, target_pos, base_speed, last_walk_time, fleeing, target_distance)
    };

    // Rate-limit based on walk speed.
    let now = otsys_time();
    if now - last_walk_time < step_duration_ms(base_speed) {
        return;
    }

    let dx = (pos.x as i32 - target_pos.x as i32).unsigned_abs();
    let dy = (pos.y as i32 - target_pos.y as i32).unsigned_abs();
    let cur_dist = dx.max(dy) as i32;

    // Decide movement intent (C++ Monster::getDanceStep): flee or be too close
    // for a ranged keep-distance monster => step AWAY; too far => approach;
    // at the desired distance => hold position.
    let want_away = fleeing || (target_distance > 1 && cur_dist < target_distance);

    let new_pos = if want_away {
        // Direction pointing from the target to the monster increases distance.
        let Some(away) = step_direction_toward(target_pos, pos) else { return };
        let primary = pos.offset_direction(away);
        let game = g_game().lock().unwrap();
        [away,
         crate::creatures::Direction::North, crate::creatures::Direction::East,
         crate::creatures::Direction::South, crate::creatures::Direction::West]
            .into_iter()
            .map(|d| pos.offset_direction(d))
            .find(|&np| {
                let ndx = (np.x as i32 - target_pos.x as i32).unsigned_abs();
                let ndy = (np.y as i32 - target_pos.y as i32).unsigned_abs();
                (ndx.max(ndy) as i32) >= cur_dist
                    && game.map.get_tile(np).map(|t| t.is_walkable()).unwrap_or(false)
            })
            .unwrap_or(primary)
    } else {
        // Already at/within desired distance — hold.
        let melee_stop = target_distance <= 1 && dx <= 1 && dy <= 1;
        if melee_stop || cur_dist <= target_distance {
            return;
        }
        let Some(dir) = step_direction_toward(pos, target_pos) else { return };
        pos.offset_direction(dir)
    };

    let dir = step_direction_toward(pos, new_pos).unwrap_or(crate::creatures::Direction::South);

    // Check destination tile walkability.
    let (old_stackpos, walkable) = {
        let game = g_game().lock().unwrap();
        let walkable = game.map.get_tile(new_pos)
            .map(|t| t.is_walkable())
            .unwrap_or(false);
        let old_stackpos = game.map.get_tile(pos)
            .map(|t| t.get_creature_client_stackpos())
            .unwrap_or(0);
        (old_stackpos, walkable)
    };

    if !walkable || new_pos == pos {
        return;
    }

    // Commit: update direction, last_walk_time, and map position.
    {
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(creature_id) {
            creature.base_mut().direction = dir;
        }
        if let Some(m) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            m.last_walk_time = now;
        }
        game.move_creature_position(creature_id, pos, new_pos);
    }

    crate::events::dispatch::execute_step_event(creature_id, pos, new_pos, 1);
    crate::events::dispatch::execute_step_event(creature_id, new_pos, pos, 0);

    broadcast_creature_move(creature_id, pos, new_pos, old_stackpos);
}

pub fn monster_think(creature_id: u32) {
    use crate::map::MAX_VIEWPORT_X;
    use crate::map::MAX_VIEWPORT_Y;

    // Read monster state: is_hostile, current target, position.
    let (is_hostile, has_target, pos) = {
        let game = g_game().lock().unwrap();
        let Some(creature) = game.get_creature(creature_id) else { return };
        let Some(monster) = creature.as_monster() else { return };
        let is_hostile = monster.is_hostile();
        let has_target = creature.base().attacked_creature_id.is_some();
        let pos = creature.position();
        (is_hostile, has_target, pos)
    };

    if !is_hostile {
        return;
    }

    // Update target list: find all creatures in the viewport and filter to valid opponents.
    // Mirrors C++ Monster::isOpponent: non-summon monsters target players without
    // IgnoredByMonsters and player-owned summons; player-summon monsters target everyone
    // except their master.
    let (is_summon, master_id) = {
        let game = g_game().lock().unwrap();
        let m = game.get_creature(creature_id).and_then(|c| c.as_monster());
        let is_summon = m.map(|m| m.base.is_summon()).unwrap_or(false);
        let master_id = m.and_then(|m| m.base.master_id);
        (is_summon, master_id)
    };

    let opponent_ids: Vec<u32> = {
        let mut game = g_game().lock().unwrap();
        let nearby = game.map.get_spectators(
            pos,
            true,  // multifloor
            false, // all creatures, not just players
            MAX_VIEWPORT_X,
            MAX_VIEWPORT_X,
            MAX_VIEWPORT_Y,
            MAX_VIEWPORT_Y,
        );
        nearby.into_iter()
            .filter(|&id| {
                if id == creature_id { return false; }
                let Some(creature) = game.get_creature(id) else { return false };
                if is_summon {
                    // Player-summon: everyone except master is an opponent.
                    let master_is_player = master_id
                        .and_then(|mid| game.get_creature(mid))
                        .map(|c| c.is_player())
                        .unwrap_or(false);
                    if master_is_player {
                        return Some(id) != master_id;
                    }
                    return false;
                }
                // Non-summon: players without IgnoredByMonsters, or player-owned summons.
                if let Some(p) = creature.as_player() {
                    return !p.has_flag(crate::creatures::player::PLAYER_FLAG_IGNORED_BY_MONSTERS)
                        && !p.is_in_ghost_mode();
                }
                if let Some(m) = creature.as_monster() {
                    if m.base.is_summon() {
                        return m.base.master_id
                            .and_then(|mid| game.get_creature(mid))
                            .map(|c| c.is_player())
                            .unwrap_or(false);
                    }
                }
                false
            })
            .collect()
    };

    // Add new opponents to the target_list.
    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            for &opp_id in &opponent_ids {
                if !monster.target_list.contains(&opp_id) {
                    monster.target_list.push_back(opp_id);
                }
            }
        }
    }

    // If no attack target yet and we have opponents, set the first one.
    if !has_target && !opponent_ids.is_empty() {
        let first_target = opponent_ids[0];
        let mut game = g_game().lock().unwrap();
        if let Some(creature) = game.get_creature_mut(creature_id) {
            creature.base_mut().attacked_creature_id = Some(first_target);
        }
    }

    // Prune target_list: remove dead or no-longer-opponent targets.
    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.target_list.retain(|id| opponent_ids.contains(id));
        }
    }

    // If current target is dead or no longer a valid opponent, clear it.
    {
        let game = g_game().lock().unwrap();
        let current_target = game.get_creature(creature_id)
            .and_then(|c| c.base().attacked_creature_id);
        if let Some(tid) = current_target {
            let target_valid = game.get_creature(tid)
                .map(|t| t.base().health > 0)
                .unwrap_or(false)
                && opponent_ids.contains(&tid);
            drop(game);

            if !target_valid {
                let mut game = g_game().lock().unwrap();
                if let Some(creature) = game.get_creature_mut(creature_id) {
                    creature.base_mut().attacked_creature_id = None;
                }
            }
        }
    }

    // Walk toward attack target if out of melee range.
    monster_walk(creature_id);

    // Defense spells and voices fire unconditionally (not gated on has_target in C++).
    monster_do_defense(creature_id);
    monster_do_yell(creature_id);
}

pub fn monster_do_defense(creature_id: u32) {
    use crate::combat::{Combat, CombatDamage, CombatOrigin, CombatParams, CombatType};
    use crate::util::normal_random;
    use crate::net::game_protocol::{broadcast_magic_effect, broadcast_creature_health};

    let (defense_ticks, spells, pos, is_summon, max_summons, summon_blocks, has_target) = {
        let game = g_game().lock().unwrap();
        let Some(monster) = game.get_creature(creature_id).and_then(|c| c.as_monster()) else {
            return;
        };
        (
            monster.defense_ticks,
            monster.mtype_info.defense_spells.clone(),
            monster.base.position,
            monster.base.is_summon(),
            monster.mtype_info.max_summons,
            monster.mtype_info.summons.clone(),
            monster.base.attacked_creature_id.is_some(),
        )
    };

    let interval = EVENT_CREATURE_THINK_INTERVAL as u32;
    let new_ticks = defense_ticks + interval;
    let mut reset_ticks = true;

    for spell in &spells {
        if spell.speed > new_ticks {
            reset_ticks = false;
            continue;
        }

        if new_ticks % spell.speed < interval {
            // already used this round
        } else {
            continue;
        }

        let chance = spell.chance.min(100);
        if chance < 100 && (normal_random(1, 100) as u32) > chance {
            continue;
        }

        let min_val = spell.min_combat_value;
        let max_val = spell.max_combat_value;
        let damage_val = if min_val == max_val {
            min_val
        } else {
            normal_random(min_val.min(max_val), min_val.max(max_val))
        };

        let area_effect = spell.area_effect;
        let combat_type = spell.combat_type;

        if matches!(combat_type, CombatType::Healing) {
            let heal = damage_val.max(0);
            if heal > 0 {
                let (new_hp, hp_max) = {
                    let mut game = g_game().lock().unwrap();
                    if let Some(c) = game.get_creature_mut(creature_id) {
                        let b = c.base_mut();
                        b.health = (b.health + heal).min(b.health_max);
                        (b.health, b.health_max)
                    } else { (0, 0) }
                };
                broadcast_creature_health(creature_id, pos, new_hp, hp_max, false);
            }
            if area_effect > 0 {
                broadcast_magic_effect(pos, area_effect);
            }
        } else {
            let params = CombatParams {
                combat_type,
                blocked_by_armor: false,
                blocked_by_shield: false,
                ..CombatParams::default()
            };
            let mut damage = CombatDamage {
                origin: CombatOrigin::Spell,
                primary_type: combat_type,
                primary_value: damage_val,
                ..CombatDamage::default()
            };
            Combat::do_target_combat(Some(creature_id), creature_id, &mut damage, &params);
            if area_effect > 0 {
                broadcast_magic_effect(pos, area_effect);
            }
        }
    }

    // Summons (port of Monster::onThinkDefense summon loop). Only non-summon
    // monsters that are actively pursuing a target may summon.
    use crate::creatures::monster::Monster;
    use crate::creatures::Creature;
    use crate::game::{CONST_ME_MAGIC_BLUE, CONST_ME_TELEPORT};
    if !is_summon && has_target && !summon_blocks.is_empty() {
        for block in &summon_blocks {
            if block.speed > new_ticks {
                reset_ticks = false;
                continue;
            }
            if new_ticks % block.speed >= interval {
                continue; // already used this round
            }

            // Count current summons + per-name count under the lock.
            let (total_summons, same_name) = {
                let game = g_game().lock().unwrap();
                let Some(monster) = game.get_creature(creature_id).and_then(|c| c.as_monster()) else {
                    break;
                };
                let ids = monster.base.summon_ids.clone();
                let total = ids.len() as u32;
                let same = ids
                    .iter()
                    .filter(|&&sid| {
                        game.get_creature(sid)
                            .and_then(|c| c.as_monster())
                            .map(|m| m.get_name().eq_ignore_ascii_case(&block.name))
                            .unwrap_or(false)
                    })
                    .count() as u32;
                (total, same)
            };

            if total_summons >= max_summons {
                continue;
            }
            if same_name >= block.max {
                continue;
            }
            if block.chance < normal_random(1, 100) as u32 {
                continue;
            }

            let Some(mut summon) = Monster::create_monster(&block.name) else { continue };
            summon.base.position = pos;
            summon.spawn_pos = pos;
            summon.base.master_id = Some(creature_id);
            summon.base.loot_drop = false;
            summon.base.skill_loss = false;
            let summon_id = summon.base.id;

            let placed = {
                let mut game = g_game().lock().unwrap();
                let ok = game.place_creature(Creature::Monster(summon));
                if ok {
                    if let Some(m) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
                        m.base.summon_ids.push(summon_id);
                    }
                }
                ok
            };

            if placed {
                crate::net::game_protocol::broadcast_creature_appear(summon_id, pos);
                broadcast_magic_effect(pos, CONST_ME_MAGIC_BLUE);
                broadcast_magic_effect(pos, CONST_ME_TELEPORT);
            }
        }
    }

    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.defense_ticks = if reset_ticks { 0 } else { new_ticks };
        }
    }
}

pub fn monster_do_yell(creature_id: u32) {
    use crate::util::normal_random;
    use crate::net::game_protocol::broadcast_creature_say;

    let (yell_speed_ticks, yell_chance, yell_ticks, voices, pos, name) = {
        let game = g_game().lock().unwrap();
        let Some(monster) = game.get_creature(creature_id).and_then(|c| c.as_monster()) else {
            return;
        };
        if monster.mtype_info.yell_speed_ticks == 0 {
            return;
        }
        (
            monster.mtype_info.yell_speed_ticks,
            monster.mtype_info.yell_chance,
            monster.yell_ticks,
            monster.mtype_info.voice_vector.clone(),
            monster.base.position,
            monster.get_name().to_owned(),
        )
    };

    let interval = EVENT_CREATURE_THINK_INTERVAL as u32;
    let new_ticks = yell_ticks + interval;

    if new_ticks < yell_speed_ticks {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.yell_ticks = new_ticks;
        }
        return;
    }

    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.yell_ticks = 0;
        }
    }

    if voices.is_empty() {
        return;
    }
    if yell_chance < (normal_random(1, 100) as u32) {
        return;
    }

    let idx = if voices.len() == 1 { 0 } else { normal_random(0, (voices.len() as i32) - 1) as usize };
    let voice = &voices[idx];

    // TALKTYPE_MONSTER_YELL=20, TALKTYPE_MONSTER_SAY=19
    let speak_type: u8 = if voice.yell_text { 20 } else { 19 };

    broadcast_creature_say(creature_id, pos, &name, 0, speak_type, voice.text.as_bytes());
}

pub fn monster_do_attacking(creature_id: u32, target_id: u32, attacker_pos: crate::map::Position, target_pos: crate::map::Position) {
    use crate::combat::{Combat, CombatDamage, CombatOrigin, CombatParams, CombatType};
    use crate::util::normal_random;
    use crate::net::game_protocol::{broadcast_magic_effect, broadcast_distance_effect};

    // Collect attack spells and tick state.
    let (attack_ticks, spells) = {
        let game = g_game().lock().unwrap();
        let Some(monster) = game.get_creature(creature_id).and_then(|c| c.as_monster()) else {
            return;
        };
        (monster.attack_ticks, monster.mtype_info.attack_spells.clone())
    };

    let interval = EVENT_CREATURE_THINK_INTERVAL as u32;
    let new_ticks = attack_ticks + interval;
    let mut reset_ticks = false;

    for spell in &spells {
        let range_ok = if spell.is_melee {
            let dx = (attacker_pos.x as i32 - target_pos.x as i32).unsigned_abs();
            let dy = (attacker_pos.y as i32 - target_pos.y as i32).unsigned_abs();
            dx <= 1 && dy <= 1
        } else if spell.range > 0 {
            let dx = (attacker_pos.x as i32 - target_pos.x as i32).unsigned_abs();
            let dy = (attacker_pos.y as i32 - target_pos.y as i32).unsigned_abs();
            dx <= spell.range && dy <= spell.range
        } else {
            true
        };

        if !range_ok { continue; }

        if new_ticks < spell.speed {
            reset_ticks = false;
            continue;
        }

        reset_ticks = true;

        let chance = spell.chance.min(100);
        if chance < 100 && (normal_random(1, 100) as u32) > chance {
            continue;
        }

        let min_val = spell.min_combat_value;
        let max_val = spell.max_combat_value;
        let damage_val = if min_val == max_val {
            min_val
        } else {
            normal_random(min_val.min(max_val), min_val.max(max_val))
        };

        if spell.is_melee {
            let params = CombatParams {
                combat_type: CombatType::PhysicalDamage,
                blocked_by_armor: true,
                blocked_by_shield: true,
                ..CombatParams::default()
            };
            let mut damage = CombatDamage {
                origin: CombatOrigin::Melee,
                primary_type: CombatType::PhysicalDamage,
                primary_value: damage_val,
                ..CombatDamage::default()
            };
            Combat::do_target_combat(Some(creature_id), target_id, &mut damage, &params);
        } else {
            // Non-melee spell attack.
            let combat_type = spell.combat_type;
            let is_healing = matches!(combat_type, CombatType::Healing);
            let area_effect = spell.area_effect;
            let shoot_effect = spell.shoot_effect;

            if shoot_effect > 0 {
                broadcast_distance_effect(attacker_pos, target_pos, shoot_effect);
            }

            if is_healing {
                // Monster heals itself.
                let heal = damage_val.max(0);
                if heal > 0 {
                    let (new_hp, hp_max) = {
                        let mut game = g_game().lock().unwrap();
                        if let Some(c) = game.get_creature_mut(creature_id) {
                            let b = c.base_mut();
                            b.health = (b.health + heal).min(b.health_max);
                            (b.health, b.health_max)
                        } else { (0, 0) }
                    };
                    crate::net::game_protocol::broadcast_creature_health(creature_id, attacker_pos, new_hp, hp_max, false);
                }
                if area_effect > 0 {
                    broadcast_magic_effect(attacker_pos, area_effect);
                }
            } else if matches!(combat_type, CombatType::ManaDrain) {
                let drain = (-damage_val).max(0);
                if drain > 0 {
                    let mut game = g_game().lock().unwrap();
                    if let Some(p) = game.get_player_mut(target_id) {
                        p.mana = p.mana.saturating_sub(drain as u32);
                    }
                    drop(game);
                    crate::net::game_protocol::send_stats_to_player(target_id);
                }
                if area_effect > 0 {
                    broadcast_magic_effect(target_pos, area_effect);
                }
            } else {
                // Damage spell.
                let params = CombatParams {
                    combat_type,
                    blocked_by_armor: false,
                    blocked_by_shield: false,
                    ..CombatParams::default()
                };
                let mut damage = CombatDamage {
                    origin: CombatOrigin::Spell,
                    primary_type: combat_type,
                    primary_value: damage_val,
                    ..CombatDamage::default()
                };
                Combat::do_target_combat(Some(creature_id), target_id, &mut damage, &params);
                if area_effect > 0 {
                    broadcast_magic_effect(target_pos, area_effect);
                }
            }
        }
    }

    // Update attack_ticks.
    {
        let mut game = g_game().lock().unwrap();
        if let Some(monster) = game.get_creature_mut(creature_id).and_then(|c| c.as_monster_mut()) {
            monster.attack_ticks = if reset_ticks { 0 } else { new_ticks };
        }
    }
}