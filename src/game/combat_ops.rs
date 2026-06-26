use crate::combat::{BlockType, CombatDamage, CombatOrigin, CombatType};
use crate::combat::condition::ConditionType;
use crate::creatures::{Creature, CreatureId, RaceType, Skull};
use crate::creatures::player::{Player, PlayerSex};
use crate::map::Position;
use crate::net::game_protocol::send_packet_to_player;
use crate::net::output_message::OutputMessage;

use super::*;

impl Game {
    /// Send a combat text message carrying the floating damage/heal numbers.
    ///
    /// On 10.98 the floating number is embedded in the 0xB4 text message
    /// (position + value(s) + color(s), keyed off the DAMAGE_*/HEALED* class).
    /// On 8.60 those classes don't exist on the wire — the float is the
    /// separate 0x84 AnimatedText, so fall back to a plain status message.
    #[allow(clippy::too_many_arguments)]
    fn send_combat_text_message(
        &mut self,
        player_id: CreatureId,
        wire_type: u8,
        pos: Position,
        primary_value: u32,
        primary_color: u8,
        secondary_value: u32,
        secondary_color: u8,
        text: String,
    ) {
        if !crate::net::protocol_version::client_version().is_1098() {
            self.send_text_message(player_id, MESSAGE_STATUS_DEFAULT, text);
            return;
        }
        let has_secondary = matches!(wire_type, 23 | 24 | 27);
        send_packet_to_player(player_id, move |output: &mut OutputMessage| {
            output.add_byte(0xB4);
            output.add_byte(wire_type);
            output.add_position(pos.x, pos.y, pos.z);
            output.add_u32(primary_value);
            output.add_byte(primary_color);
            if has_secondary {
                output.add_u32(secondary_value);
                output.add_byte(secondary_color);
            }
            output.add_string(text.as_bytes());
        });
    }

    // ── Block check ──────────────────────────────────────────────────────────

    /// Returns the block effect to play for a given block type and combat type.
    fn send_block_effect(&mut self, block_type: BlockType, combat_type: CombatType, target_pos: Position) {
        let effect = match block_type {
            BlockType::Defense => Some(CONST_ME_POFF),
            BlockType::Armor => Some(CONST_ME_BLOCKHIT),
            BlockType::Immunity => {
                let e = match combat_type {
                    CombatType::UndefinedDamage => return,
                    CombatType::EnergyDamage
                    | CombatType::FireDamage
                    | CombatType::PhysicalDamage
                    | CombatType::IceDamage
                    | CombatType::DeathDamage => CONST_ME_BLOCKHIT,
                    CombatType::EarthDamage => CONST_ME_GREEN_RINGS,
                    CombatType::HolyDamage => CONST_ME_HOLYDAMAGE,
                    _ => CONST_ME_POFF,
                };
                Some(e)
            }
            BlockType::None => None,
        };
        if let Some(eff) = effect {
            self.add_magic_effect(target_pos, eff);
        }
    }

    /// Mirrors C++ `Game::combatBlockHit`. Returns true if damage is fully blocked.
    #[allow(clippy::too_many_arguments)]
    pub fn combat_block_hit(
        &mut self,
        damage: &mut CombatDamage,
        attacker_id: Option<CreatureId>,
        target_id: CreatureId,
        check_defense: bool,
        check_armor: bool,
        field: bool,
        ignore_resistances: bool,
    ) -> bool {
        if damage.primary_type == CombatType::None && damage.secondary_type == CombatType::None {
            return true;
        }
        {
            let target = match self.creatures.get(&target_id) { Some(c) => c, None => return true };
            if target.is_player() && target.is_in_ghost_mode() {
                return true;
            }
            if damage.primary_value > 0 {
                return false;
            }
        }

        let target_pos = self.creatures.get(&target_id).map(|c| c.position()).unwrap_or_default();

        let primary_block = if damage.primary_type != CombatType::None {
            damage.primary_value = -damage.primary_value;
            let bt = match self.creatures.get_mut(&target_id) {
                Some(c) => c.block_hit(attacker_id, damage.primary_type, &mut damage.primary_value, check_defense, check_armor, field, ignore_resistances),
                None => BlockType::None,
            };
            damage.primary_value = -damage.primary_value;
            self.send_block_effect(bt, damage.primary_type, target_pos);
            bt
        } else {
            BlockType::None
        };

        let secondary_block = if damage.secondary_type != CombatType::None {
            damage.secondary_value = -damage.secondary_value;
            let bt = match self.creatures.get_mut(&target_id) {
                Some(c) => c.block_hit(attacker_id, damage.secondary_type, &mut damage.secondary_value, false, false, field, ignore_resistances),
                None => BlockType::None,
            };
            damage.secondary_value = -damage.secondary_value;
            self.send_block_effect(bt, damage.secondary_type, target_pos);
            bt
        } else {
            BlockType::None
        };

        damage.block_type = primary_block;

        primary_block != BlockType::None && secondary_block != BlockType::None
    }

    /// Returns (text_color, magic_effect) for a combat type on a target creature.
    /// Mirrors C++ `Game::combatGetTypeInfo`.
    fn combat_get_type_info(&self, combat_type: CombatType, target_id: CreatureId) -> (u8, u8) {
        match combat_type {
            CombatType::PhysicalDamage => {
                let race = self.creatures.get(&target_id).map(|c| c.get_race()).unwrap_or(RaceType::None);
                match race {
                    RaceType::Venom => (TEXTCOLOR_LIGHTGREEN, CONST_ME_HITBYPOISON),
                    RaceType::Blood => (TEXTCOLOR_RED, CONST_ME_DRAWBLOOD),
                    RaceType::Undead => (TEXTCOLOR_GREY, CONST_ME_HITAREA),
                    RaceType::Fire => (TEXTCOLOR_ORANGE, CONST_ME_DRAWBLOOD),
                    RaceType::Energy => (TEXTCOLOR_PURPLE, CONST_ME_ENERGYHIT),
                    RaceType::None => (TEXTCOLOR_NONE, 0),
                }
            }
            CombatType::EnergyDamage => (TEXTCOLOR_PURPLE, CONST_ME_ENERGYHIT),
            CombatType::EarthDamage => (TEXTCOLOR_LIGHTGREEN, CONST_ME_GREEN_RINGS),
            CombatType::DrownDamage => (TEXTCOLOR_LIGHTBLUE, CONST_ME_LOSEENERGY),
            CombatType::FireDamage => (TEXTCOLOR_ORANGE, CONST_ME_HITBYFIRE),
            CombatType::IceDamage => (TEXTCOLOR_TEAL, CONST_ME_ICEATTACK),
            CombatType::HolyDamage => (TEXTCOLOR_YELLOW, CONST_ME_HOLYDAMAGE),
            CombatType::DeathDamage => (TEXTCOLOR_DARKRED, CONST_ME_SMALLCLOUDS),
            CombatType::LifeDrain => (TEXTCOLOR_RED, CONST_ME_MAGIC_RED),
            _ => (TEXTCOLOR_NONE, 0),
        }
    }

    // ── Health change ─────────────────────────────────────────────────────────

    /// Mirrors C++ `Game::combatChangeHealth`.
    /// True when `creature_id` has at least one registered creature-event of
    /// `etype`. Used to gate the HEALTHCHANGE/MANACHANGE event dispatch: those
    /// `fire_*` functions re-lock `g_game`, so they must never be entered while
    /// the lock is already held (e.g. from inside `combat_change_health`). When
    /// no such event is registered there is nothing to fire and we can skip the
    /// re-locking path entirely. (Firing with the lock held remains a TODO:
    /// custom healthchange/manachange scripts need the event to run with
    /// `g_game` released, matching the step/death event pattern.)
    fn has_creature_event(&self, creature_id: CreatureId, etype: crate::events::creature::CreatureEventType) -> bool {
        self.get_creature(creature_id)
            .map(|c| !c.base().get_creature_event_names(etype).is_empty())
            .unwrap_or(false)
    }

    pub fn combat_change_health(
        &mut self,
        attacker_id: Option<CreatureId>,
        target_id: CreatureId,
        damage: &mut CombatDamage,
    ) -> bool {
        // Collect needed data up-front (read-only) to avoid split borrow issues.
        let (target_pos, target_health, target_is_player, target_attackable, target_in_ghost) = {
            let Some(t) = self.creatures.get(&target_id) else { return false };
            (t.position(), t.get_health(), t.is_player(), t.is_attackable(), t.is_in_ghost_mode())
        };
        let attacker_is_player = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.is_player()).unwrap_or(false);
        let attacker_skull = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_skull()).unwrap_or(Skull::None);
        let attacker_skull_of_target = if let (Some(aid), true) = (attacker_id, target_is_player) {
            let wt = self.get_world_type();
            match (self.get_player(aid), self.get_player(target_id)) {
                (Some(ap), Some(tp)) => ap.get_skull_client_of_player(tp, wt),
                _ => Skull::None,
            }
        } else {
            Skull::None
        };

        // ── Healing branch ────────────────────────────────────────────────────
        if damage.primary_value > 0 {
            if target_health <= 0 { return false; }
            if attacker_is_player && target_is_player
                && attacker_skull == Skull::Black && attacker_skull_of_target == Skull::None {
                return false;
            }

            if damage.origin != CombatOrigin::None
                && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::HealthChange)
                && crate::events::dispatch::fire_health_change_events(target_id, attacker_id, damage)
            {
                damage.origin = CombatOrigin::None;
                return self.combat_change_health(attacker_id, target_id, damage);
            }

            let old_health = target_health;
            let health_max = self.creatures.get(&target_id).map(|c| c.get_max_health()).unwrap_or(1);
            let gained = damage.primary_value.min(health_max - old_health);
            let new_health = old_health + gained;
            if let Some(creature) = self.creatures.get_mut(&target_id) {
                creature.base_mut().health = new_health;
            }
            let real_change = gained;

            if real_change > 0 && !target_in_ghost {
                let damage_string = format!("{} hitpoint{}", real_change, if real_change != 1 { "s" } else { "" });
                self.add_animated_text(target_pos, TEXTCOLOR_MAYABLUE, &real_change.to_string());

                let spectators = self.map.get_spectators(target_pos, false, true, 0, 0, 0, 0);
                let attacker_name = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_name_description());
                let target_name = self.creatures.get(&target_id).map(|c| c.get_name_description()).unwrap_or_default();
                let target_sex_is_female = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_sex() == PlayerSex::Female).unwrap_or(false);

                for spec_id in spectators {
                    let is_attacker = Some(spec_id) == attacker_id;
                    let is_target = spec_id == target_id;
                    let msg = if is_attacker && !is_target {
                        format!("You heal {} for {}.", target_name, damage_string)
                    } else if is_target {
                        match &attacker_name {
                            None => format!("You were healed for {}.", damage_string),
                            Some(an) if attacker_id == Some(target_id) => format!("You healed yourself for {}.", damage_string),
                            Some(an) => format!("You were healed by {} for {}.", an, damage_string),
                        }
                    } else {
                        match &attacker_name {
                            None => format!("{} was healed for {}.", target_name, damage_string),
                            Some(an) if attacker_id == Some(target_id) => {
                                let pron = if target_sex_is_female { "her" } else { "him" };
                                let mut s = format!("{} healed {}self for {}.", an, pron, damage_string);
                                if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                                s
                            }
                            Some(an) => {
                                let mut s = format!("{} healed {} for {}.", an, target_name, damage_string);
                                if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                                s
                            }
                        }
                    };
                    let wire_type = if is_attacker || is_target { 25 } else { 28 };
                    self.send_combat_text_message(spec_id, wire_type, target_pos, real_change as u32, TEXTCOLOR_PASTELRED, 0, TEXTCOLOR_NONE, msg);
                }
            }

            // Creature health bar updates only when health actually changed
            // (C++ Creature::changeHealth → addCreatureHealth on change), but a
            // player target always gets a stats refresh (C++ Player::changeHealth
            // → sendStats() unconditionally).
            if real_change > 0 {
                self.add_creature_health(target_id);
            }
            if target_is_player {
                self.send_player_stats(target_id);
            }
            return true;
        }

        // ── Damage branch ─────────────────────────────────────────────────────
        if !target_attackable {
            if !target_in_ghost {
                self.add_magic_effect(target_pos, CONST_ME_POFF);
            }
            return true;
        }

        if attacker_is_player && target_is_player
            && attacker_skull == Skull::Black && attacker_skull_of_target == Skull::None {
            return false;
        }

        let primary_abs = damage.primary_value.unsigned_abs() as i32;
        let secondary_abs = damage.secondary_value.unsigned_abs() as i32;
        damage.primary_value = -primary_abs;
        damage.secondary_value = -secondary_abs;

        let health_change = primary_abs + secondary_abs;
        if health_change == 0 {
            return true;
        }

        // ── Mana shield ───────────────────────────────────────────────────────
        let spectators = self.map.get_spectators(target_pos, true, true, 0, 0, 0, 0);
        let (target_mana, target_has_manashield) = if target_is_player {
            let p = self.creatures.get(&target_id).and_then(|c| c.as_player());
            let mana = p.map(|p| p.get_mana() as i32).unwrap_or(0);
            let shield = self.creatures.get(&target_id).map(|c| c.base().has_condition(ConditionType::ManaShield)).unwrap_or(false);
            (mana, shield && damage.primary_type != CombatType::UndefinedDamage)
        } else {
            (0, false)
        };

        let mut remaining_damage = health_change;
        if target_has_manashield && target_mana > 0 {
            let mana_damage = target_mana.min(health_change);
            // drain mana
            if let Some(p) = self.creatures.get_mut(&target_id).and_then(|c| c.as_player_mut()) {
                p.mana = (p.mana as i32 - mana_damage).max(0) as u32;
            }
            self.add_magic_effect(target_pos, CONST_ME_LOSEENERGY);
            self.add_animated_text(target_pos, TEXTCOLOR_BLUE, &mana_damage.to_string());

            let attacker_name = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_name_description());
            let target_name = self.creatures.get(&target_id).map(|c| c.get_name_description()).unwrap_or_default();
            let target_sex_is_female = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_sex() == PlayerSex::Female).unwrap_or(false);

            for &spec_id in &spectators {
                if self.creatures.get(&spec_id).map(|c| c.position().z != target_pos.z).unwrap_or(true) { continue; }
                let is_attacker = Some(spec_id) == attacker_id;
                let is_target = spec_id == target_id;
                let msg = if is_attacker && !is_target {
                    let mut s = format!("{} loses {} mana due to your attack.", target_name, mana_damage);
                    if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                    s
                } else if is_target {
                    match &attacker_name {
                        None => format!("You lose {} mana.", mana_damage),
                        Some(an) if attacker_id == Some(target_id) => format!("You lose {} mana due to your own attack.", mana_damage),
                        Some(an) => format!("You lose {} mana due to an attack by {}.", mana_damage, an),
                    }
                } else {
                    match &attacker_name {
                        None => {
                            let mut s = format!("{} loses {} mana.", target_name, mana_damage);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) if attacker_id == Some(target_id) => {
                            let pron = if target_sex_is_female { "her" } else { "his" };
                            let mut s = format!("{} loses {} mana due to {} own attack.", target_name, mana_damage, pron);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) => {
                            let mut s = format!("{} loses {} mana due to an attack by {}.", target_name, mana_damage, an);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                    }
                };
                let wire_type = if is_attacker && !is_target { 23 } else if is_target { 24 } else { 27 };
                self.send_combat_text_message(spec_id, wire_type, target_pos, mana_damage as u32, TEXTCOLOR_BLUE, 0, TEXTCOLOR_NONE, msg);
            }

            // reduce from damage
            damage.primary_value += mana_damage;
            if damage.primary_value > 0 {
                damage.secondary_value = (damage.secondary_value + damage.primary_value).min(0);
                damage.primary_value = 0;
            }
            // re-abs after mana reduction
            let pa = damage.primary_value.unsigned_abs() as i32;
            let sa = damage.secondary_value.unsigned_abs() as i32;
            remaining_damage = pa + sa;
        }

        if remaining_damage == 0 { return true; }

        if damage.origin != CombatOrigin::None
            && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::HealthChange)
            && crate::events::dispatch::fire_health_change_events(target_id, attacker_id, damage)
        {
            damage.origin = CombatOrigin::None;
            return self.combat_change_health(attacker_id, target_id, damage);
        }

        // Clamp damage to actual health
        let cur_health = self.creatures.get(&target_id).map(|c| c.get_health()).unwrap_or(0);
        let pa = damage.primary_value.unsigned_abs() as i32;
        let sa = damage.secondary_value.unsigned_abs() as i32;
        let (clamped_primary, clamped_secondary) = if pa >= cur_health {
            (cur_health, 0i32)
        } else {
            (pa, sa.min(cur_health - pa))
        };
        let real_damage = clamped_primary + clamped_secondary;
        if real_damage == 0 { return true; }

        // Visual: magic effect + animated text per damage type
        let (primary_color, primary_effect) = self.combat_get_type_info(damage.primary_type, target_id);
        let (secondary_color, secondary_effect) = self.combat_get_type_info(damage.secondary_type, target_id);

        if clamped_primary > 0 {
            if primary_effect != 0 {
                for &spec_id in &spectators {
                    let eff = primary_effect;
                    let pos = target_pos;
                    send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                        output.add_byte(0x83);
                        output.add_position(pos.x, pos.y, pos.z);
                        output.add_byte(eff);
                    });
                }
            }
            if primary_color != TEXTCOLOR_NONE {
                self.add_animated_text(target_pos, primary_color, &clamped_primary.to_string());
            }
        }

        if clamped_secondary > 0 {
            if secondary_effect != 0 {
                for &spec_id in &spectators {
                    let eff = secondary_effect;
                    let pos = target_pos;
                    send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                        output.add_byte(0x83);
                        output.add_position(pos.x, pos.y, pos.z);
                        output.add_byte(eff);
                    });
                }
            }
            if secondary_color != TEXTCOLOR_NONE {
                self.add_animated_text(target_pos, secondary_color, &clamped_secondary.to_string());
            }
        }

        // Text messages to spectators
        if primary_color != TEXTCOLOR_NONE || secondary_color != TEXTCOLOR_NONE {
            let damage_string = format!("{} hitpoint{}", real_damage, if real_damage != 1 { "s" } else { "" });
            let attacker_name = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_name_description());
            let target_name = self.creatures.get(&target_id).map(|c| c.get_name_description()).unwrap_or_default();
            let target_sex_is_female = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_sex() == PlayerSex::Female).unwrap_or(false);

            for &spec_id in &spectators {
                if self.creatures.get(&spec_id).map(|c| c.position().z != target_pos.z).unwrap_or(true) { continue; }
                let is_attacker = Some(spec_id) == attacker_id;
                let is_target = spec_id == target_id;
                let msg = if is_attacker && !is_target {
                    let mut s = format!("{} loses {} due to your attack.", target_name, damage_string);
                    if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                    s
                } else if is_target {
                    match &attacker_name {
                        None => format!("You lose {}.", damage_string),
                        Some(_) if attacker_id == Some(target_id) => format!("You lose {} due to your own attack.", damage_string),
                        Some(an) => format!("You lose {} due to an attack by {}.", damage_string, an),
                    }
                } else {
                    match &attacker_name {
                        None => {
                            let mut s = format!("{} loses {}.", target_name, damage_string);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) if attacker_id == Some(target_id) => {
                            let its = if target_is_player {
                                if target_sex_is_female { "her" } else { "his" }
                            } else { "its" };
                            let mut s = format!("{} loses {} due to {} own attack.", target_name, damage_string, its);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) => {
                            let mut s = format!("{} loses {} due to an attack by {}.", target_name, damage_string, an);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                    }
                };
                let wire_type = if is_attacker && !is_target { 23 } else if is_target { 24 } else { 27 };
                let p_color = if clamped_primary > 0 { primary_color } else { TEXTCOLOR_NONE };
                let s_color = if clamped_secondary > 0 { secondary_color } else { TEXTCOLOR_NONE };
                self.send_combat_text_message(spec_id, wire_type, target_pos, clamped_primary as u32, p_color, clamped_secondary as u32, s_color, msg);
            }
        }

        if real_damage >= cur_health
            && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::PrepareDeath)
            && !crate::events::dispatch::fire_prepare_death_events(target_id, attacker_id)
        {
            return false;
        }

        // Apply damage
        let new_health = (cur_health - real_damage).max(0);
        if let Some(creature) = self.creatures.get_mut(&target_id) {
            creature.base_mut().health = new_health;
        }

        // Track damage for experience distribution and kill attribution.
        if let (Some(aid), true) = (attacker_id, real_damage > 0) {
            if aid != target_id {
                if let Some(creature) = self.creatures.get_mut(&target_id) {
                    creature.base_mut().add_damage_points(aid, real_damage);
                    creature.base_mut().last_hit_creature_id = aid;
                }

                // onAttackedCreature + onAttacked — mirrors C++ player.cpp:3347-3406 verbatim.
                let attacker_is_player_here = self.get_player(aid).is_some();
                if attacker_is_player_here {
                    // target->getZone() == ZONE_PVP → skip skull logic entirely
                    let target_in_pvp = self.get_creature(target_id)
                        .and_then(|c| self.map.get_tile(c.position()))
                        .map(|t| t.has_flag(crate::map::tile::TILESTATE_PVPZONE))
                        .unwrap_or(false);

                    let not_gain_in_fight = self.get_player(aid)
                        .map(|p| p.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_IN_FIGHT))
                        .unwrap_or(false);

                    if target_is_player && !target_in_pvp && !not_gain_in_fight {
                        // Check party/guild exemption
                        let is_partner = {
                            let ap = self.get_player(aid);
                            let tp = self.get_player(target_id);
                            match (ap, tp) {
                                (Some(a), Some(t)) => {
                                    (a.party_id.is_some() && a.party_id == t.party_id)
                                    || (a.guild_id.is_some() && a.guild_id == t.guild_id)
                                }
                                _ => false,
                            }
                        };

                        if !is_partner {
                            // PVP_ENFORCED: immediate pzLock
                            if self.get_world_type() == crate::game::WorldType::PvpEnforced {
                                let was_locked = self.get_player(aid).map(|p| p.pz_locked).unwrap_or(true);
                                if !was_locked {
                                    if let Some(ap) = self.get_player_mut(aid) {
                                        ap.pz_locked = true;
                                    }
                                    crate::net::game_protocol::send_icons_to_player(aid);
                                }
                            }

                            // Target gets InFight
                            if let Some(tp) = self.get_player_mut(target_id) {
                                tp.add_in_fight_ticks(false);
                            }

                            let (attacker_skull, attacker_skull_of_target, target_skull, target_has_attacked) = {
                                let ap = self.get_player(aid);
                                let tp = self.get_player(target_id);
                                match (ap, tp) {
                                    (Some(a), Some(t)) => {
                                        let wt = self.get_world_type();
                                        (
                                            a.base.skull, a.get_skull_client_of_player(t, wt),
                                            t.base.skull, t.has_attacked(a.guid),
                                        )
                                    }
                                    _ => (Skull::None, Skull::None, Skull::None, false),
                                }
                            };

                            let both_in_pvp_zone = {
                                let ap_pvp = self.get_creature(aid)
                                    .and_then(|c| self.map.get_tile(c.position()))
                                    .map(|t| t.has_flag(crate::map::tile::TILESTATE_PVPZONE))
                                    .unwrap_or(false);
                                let tp_pvp = self.get_creature(target_id)
                                    .and_then(|c| self.map.get_tile(c.position()))
                                    .map(|t| t.has_flag(crate::map::tile::TILESTATE_PVPZONE))
                                    .unwrap_or(false);
                                ap_pvp && tp_pvp
                            };

                            let target_guid = self.get_player(target_id).map(|p| p.guid).unwrap_or(0);

                            if attacker_skull == Skull::None && attacker_skull_of_target == Skull::Yellow {
                                // Yellow skull path: reciprocal attack recording
                                if let Some(ap) = self.get_player_mut(aid) {
                                    ap.attacked_set.insert(target_guid);
                                }
                                crate::net::game_protocol::send_creature_skull_to_player(target_id, aid, Skull::None);
                            } else if !target_has_attacked {
                                // Target hasn't attacked us → pzLock + possible white skull
                                let was_locked = self.get_player(aid).map(|p| p.pz_locked).unwrap_or(true);
                                if !was_locked {
                                    if let Some(ap) = self.get_player_mut(aid) {
                                        ap.pz_locked = true;
                                    }
                                    crate::net::game_protocol::send_icons_to_player(aid);
                                }

                                if !both_in_pvp_zone {
                                    if let Some(ap) = self.get_player_mut(aid) {
                                        ap.attacked_set.insert(target_guid);
                                    }
                                    if target_skull == Skull::None && attacker_skull == Skull::None {
                                        if let Some(ap) = self.get_player_mut(aid) {
                                            ap.base.skull = Skull::White;
                                        }
                                    }
                                    // Send skull to target (only if attacker has no skull)
                                    let current_skull = self.get_player(aid).map(|p| p.base.skull).unwrap_or(Skull::None);
                                    if current_skull == Skull::None {
                                        crate::net::game_protocol::send_creature_skull_to_player(target_id, aid, Skull::None);
                                    } else {
                                        crate::net::game_protocol::send_creature_skull_to_player(target_id, aid, current_skull);
                                    }
                                }
                            }
                        }
                    }

                    // Attacker gets InFight (pzlock for PvP targets)
                    if let Some(p) = self.get_player_mut(aid) {
                        p.add_in_fight_ticks(target_is_player && !target_in_pvp);
                    }
                } else if target_is_player {
                    // Non-player attacker (monster) → target gets InFight (onAttacked)
                    if let Some(p) = self.get_player_mut(target_id) {
                        p.add_in_fight_ticks(false);
                    }
                }

                // Party in-fight ticks (port of Party::updatePlayerTicks via
                // Player::onAttackedCreature) — feeds shared-experience gating.
                if let Some(leader_id) = self.get_player(aid).and_then(|p| p.party_id) {
                    let not_in_fight = self
                        .get_player(aid)
                        .map(|p| p.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_IN_FIGHT))
                        .unwrap_or(false);
                    if !not_in_fight {
                        if let Some(party) = self.get_party_mut(leader_id) {
                            party.update_player_ticks(aid, real_damage as u32);
                        }
                        self.party_update_shared_experience(leader_id);
                    }
                }
            }
        }

        // Broadcast updated health bar
        self.add_creature_health(target_id);

        // Schedule death if health hit zero
        if new_health <= 0 {
            let tid = target_id;
            g_dispatcher().add_task(Task::new(move || {
                if let Ok(mut game) = crate::game::g_game().lock() {
                    game.execute_death(tid);
                }
            }));
        }

        true
    }

    /// Mirrors C++ `Game::combatChangeMana`.
    pub fn combat_change_mana(
        &mut self,
        attacker_id: Option<CreatureId>,
        target_id: CreatureId,
        damage: &mut CombatDamage,
    ) -> bool {
        let target_is_player = self.creatures.get(&target_id).map(|c| c.is_player()).unwrap_or(false);
        if !target_is_player { return true; }

        let mana_change = damage.primary_value + damage.secondary_value;
        if mana_change > 0 {
            // healing mana
            let attacker_skull = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_skull()).unwrap_or(Skull::None);
            let attacker_skull_of_target = Skull::None;
            if attacker_skull == Skull::Black && attacker_skull_of_target == Skull::None {
                return false;
            }
            if damage.origin != CombatOrigin::None
                && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::ManaChange)
                && crate::events::dispatch::fire_mana_change_events(target_id, attacker_id, damage)
            {
                damage.origin = CombatOrigin::None;
                return self.combat_change_mana(attacker_id, target_id, damage);
            }
            if let Some(p) = self.creatures.get_mut(&target_id).and_then(|c| c.as_player_mut()) {
                p.mana = (p.mana as i32 + mana_change).min(p.mana_max as i32).max(0) as u32;
            }
        } else {
            let target_pos = self.creatures.get(&target_id).map(|c| c.position()).unwrap_or_default();
            let target_attackable = self.creatures.get(&target_id).map(|c| c.is_attackable()).unwrap_or(false);
            let target_in_ghost = self.creatures.get(&target_id).map(|c| c.is_in_ghost_mode()).unwrap_or(false);
            if !target_attackable {
                if !target_in_ghost { self.add_magic_effect(target_pos, CONST_ME_POFF); }
                return false;
            }
            let attacker_skull = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_skull()).unwrap_or(Skull::None);
            if attacker_skull == Skull::Black { return false; }

            let target_mana = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_mana() as i32).unwrap_or(0);
            let mana_loss = target_mana.min(-mana_change);
            // block check
            let bt = match self.creatures.get_mut(&target_id) {
                Some(c) => c.block_hit(attacker_id, CombatType::ManaDrain, &mut { mana_loss }, false, false, false, false),
                None => BlockType::None,
            };
            if bt != BlockType::None {
                self.add_magic_effect(target_pos, CONST_ME_POFF);
                return false;
            }
            if mana_loss <= 0 { return true; }

            if damage.origin != CombatOrigin::None
                && self.has_creature_event(target_id, crate::events::creature::CreatureEventType::ManaChange)
                && crate::events::dispatch::fire_mana_change_events(target_id, attacker_id, damage)
            {
                damage.origin = CombatOrigin::None;
                return self.combat_change_mana(attacker_id, target_id, damage);
            }
            if let Some(p) = self.creatures.get_mut(&target_id).and_then(|c| c.as_player_mut()) {
                p.mana = (p.mana as i32 - mana_loss).max(0) as u32;
            }
            self.add_animated_text(target_pos, TEXTCOLOR_BLUE, &mana_loss.to_string());

            let target_name = self.creatures.get(&target_id).map(|c| c.get_name_description()).unwrap_or_default();
            let attacker_name = attacker_id.and_then(|id| self.creatures.get(&id)).map(|c| c.get_name_description());
            let target_sex_is_female = self.creatures.get(&target_id).and_then(|c| c.as_player()).map(|p| p.get_sex() == PlayerSex::Female).unwrap_or(false);

            let spectators = self.map.get_spectators(target_pos, false, true, 0, 0, 0, 0);
            for spec_id in spectators {
                let is_attacker = Some(spec_id) == attacker_id;
                let is_target = spec_id == target_id;
                let msg = if is_attacker && !is_target {
                    let mut s = format!("{} loses {} mana due to your attack.", target_name, mana_loss);
                    if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                    s
                } else if is_target {
                    match &attacker_name {
                        None => format!("You lose {} mana.", mana_loss),
                        Some(_) if attacker_id == Some(target_id) => format!("You lose {} mana due to your own attack.", mana_loss),
                        Some(an) => format!("You lose {} mana due to an attack by {}.", mana_loss, an),
                    }
                } else {
                    match &attacker_name {
                        None => {
                            let mut s = format!("{} loses {} mana.", target_name, mana_loss);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) if attacker_id == Some(target_id) => {
                            let pron = if target_sex_is_female { "her" } else { "his" };
                            let mut s = format!("{} loses {} mana due to {} own attack.", target_name, mana_loss, pron);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                        Some(an) => {
                            let mut s = format!("{} loses {} mana due to an attack by {}.", target_name, mana_loss, an);
                            if let Some(c) = s.get_mut(0..1) { c.make_ascii_uppercase(); }
                            s
                        }
                    }
                };
                self.send_text_message(spec_id, MESSAGE_STATUS_DEFAULT, msg);
            }
        }
        true
    }

    // ── Death handling ────────────────────────────────────────────────────────

    /// Mirrors C++ `Game::executeDeath`. Called by dispatcher after health hits 0.
    pub fn execute_death(&mut self, creature_id: CreatureId) {
        if !self.creatures.contains_key(&creature_id) {
            return;
        }

        // Collect damage map + identity info before mutating.
        let pz_locked_ms = crate::config::g_config()
            .get_number(crate::config::IntegerConfig::PzLocked) as i64;
        let now_ms = crate::util::otsys_time();
        let (is_player, last_hit_id, damage_entries, pos, lost_exp, most_damage_id, player_look_corpse) = {
            let Some(creature) = self.creatures.get(&creature_id) else { return };
            let base = creature.base();
            let last_hit_id = if base.last_hit_creature_id != 0 { Some(base.last_hit_creature_id) } else { None };
            let damage_entries: Vec<(CreatureId, i32)> = base.damage_map.iter()
                .map(|(&id, cb)| (id, cb.total))
                .collect();
            let pos = creature.position();
            let lost_exp: u64 = match creature {
                Creature::Monster(m) => {
                    if m.base.skill_loss { m.mtype_info.experience } else { 0 }
                }
                _ => 0,
            };
            let is_player = creature.is_player();
            let player_look_corpse: u16 = match creature {
                Creature::Player(p) => {
                    if p.sex == crate::creatures::player::PlayerSex::Female { 3065 } else { 3058 }
                }
                _ => 0,
            };
            // Determine most-damage attacker (C++ Creature::onDeath logic).
            let most_damage_id: CreatureId = {
                let mut best_total = 0i32;
                let mut best_id = 0u32;
                for (&attacker_id, cb) in &base.damage_map {
                    if cb.total > best_total && (now_ms - cb.ticks) <= pz_locked_ms {
                        best_total = cb.total;
                        best_id = attacker_id;
                    }
                }
                best_id
            };
            (is_player, last_hit_id, damage_entries, pos, lost_exp, most_damage_id, player_look_corpse)
        };

        // Collect creature Lua event script IDs while creature still in game.
        // These are fired in a deferred task (after game lock is released) to avoid deadlock.
        // Pairs are (script_id, from_lua) — from_lua=true means id is in lua_callbacks,
        // false means it's in the CreatureEvents script interface's event table.
        use crate::events::creature::CreatureEventType;
        let collect_script_info = |base: &crate::creatures::CreatureBase, etype: CreatureEventType| -> Vec<(i32, bool)> {
            use crate::events::registry::g_script_registry;
            let names = base.get_creature_event_names(etype);
            if names.is_empty() { return Vec::new(); }
            let registry = g_script_registry().lock().unwrap();
            names.iter()
                .filter_map(|n| registry.creature_events.get_event_by_name(n, true).map(|e| (e.script_id, e.from_lua)))
                .collect()
        };

        let kill_script_ids_last: Vec<(i32, bool)> = last_hit_id
            .and_then(|id| self.get_creature(id))
            .map(|c| collect_script_info(c.base(), CreatureEventType::Kill))
            .unwrap_or_default();

        let kill_script_ids_most: Vec<(i32, bool)> = if most_damage_id != 0
            && most_damage_id != last_hit_id.unwrap_or(0)
        {
            self.get_creature(most_damage_id)
                .map(|c| collect_script_info(c.base(), CreatureEventType::Kill))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let death_script_ids: Vec<(i32, bool)> = self.get_creature(creature_id)
            .map(|c| collect_script_info(c.base(), CreatureEventType::Death))
            .unwrap_or_default();

        let last_hit_copy = last_hit_id.unwrap_or(0);
        let most_dmg_copy = most_damage_id;
        let creature_id_copy = creature_id;

        // ── Experience distribution ──────────────────────────────────────────
        // Raw exp per attacker is floor(damageRatio * lostExperience); the
        // experience-stage and stamina multipliers are applied by the Lua
        // `Player:onGainExperience` event, which must run with the game lock
        // released, so the whole step is deferred to a dispatcher task.
        if lost_exp > 0 && !damage_entries.is_empty() {
            let total_damage: i32 = damage_entries.iter().map(|(_, d)| d).sum();
            if total_damage > 0 {
                let pos_copy = pos;
                let source_id = creature_id;
                let mut distribution: Vec<(CreatureId, u64)> = Vec::new();
                for (attacker_id, attacker_damage) in &damage_entries {
                    let gained = ((*attacker_damage as f64 / total_damage as f64) * lost_exp as f64).floor() as u64;
                    if gained == 0 { continue; }
                    distribution.push((*attacker_id, gained));
                }
                if !distribution.is_empty() {
                    crate::runtime::g_dispatcher().add_task(
                        crate::runtime::dispatcher::Task::new(move || {
                            for (attacker_id, raw_gained) in distribution {
                                let modified = crate::events::g_events().lock().unwrap()
                                    .event_player_on_gain_experience(
                                        attacker_id, Some(source_id), "Monster", raw_gained, raw_gained,
                                    );
                                if modified == 0 { continue; }

                                // Party shared experience: if the attacker is in
                                // a party with shared exp active+enabled, the gain
                                // is shared among all members instead of given
                                // directly (port of Player::onGainExperience).
                                let shared_party = {
                                    let game = crate::game::g_game().lock().unwrap();
                                    game.get_player(attacker_id).and_then(|p| p.party_id).filter(|&lid| {
                                        game.get_party(lid)
                                            .map(|pt| pt.is_shared_experience_active() && pt.is_shared_experience_enabled())
                                            .unwrap_or(false)
                                    })
                                };

                                if let Some(leader_id) = shared_party {
                                    // Lua Party:onShareExperience runs with the lock released.
                                    let shared = crate::events::g_events().lock().unwrap()
                                        .event_party_on_share_experience(leader_id, modified);
                                    let recipients = {
                                        let game = crate::game::g_game().lock().unwrap();
                                        let mut r = game.get_party(leader_id)
                                            .map(|pt| pt.get_members().to_vec())
                                            .unwrap_or_default();
                                        r.push(leader_id);
                                        r
                                    };
                                    if let Ok(mut game) = crate::game::g_game().lock() {
                                        for member in recipients {
                                            game.give_experience(member, shared, pos_copy);
                                        }
                                    }
                                } else if let Ok(mut game) = crate::game::g_game().lock() {
                                    game.give_experience(attacker_id, modified, pos_copy);
                                }
                            }
                        }),
                    );
                }
            }
        }

        // ── Player death ─────────────────────────────────────────────────────
        if is_player {
            // Place player corpse on the death tile before teleporting.
            let (corpse_server_id, corpse_pos, corpse_tile_idx) = if player_look_corpse > 0 {
                let corpse_item = crate::map::tile::MapItem {
                    server_id: player_look_corpse,
                    owner_id: creature_id,
                    ..crate::map::tile::MapItem::default()
                };
                let items_arc = self.items.clone();
                if let Some(tile) = self.map.get_tile_mut(pos) {
                    tile.internal_add_item(corpse_item, &items_arc);
                    (player_look_corpse, pos, 0i32)
                } else {
                    (0u16, pos, -1i32)
                }
            } else {
                (0u16, pos, -1i32)
            };
            if corpse_server_id > 0 {
                self.start_decay(corpse_server_id, corpse_pos);
            }
            let corpse_server_id_copy = corpse_server_id;
            let corpse_pos_copy = corpse_pos;
            let corpse_tile_idx_copy = corpse_tile_idx;

            self.execute_player_death(creature_id, last_hit_id);

            // Dispatch player onDeath events (with corpse) and killer onKill events.
            let need_dispatch = !death_script_ids.is_empty()
                || !kill_script_ids_last.is_empty()
                || !kill_script_ids_most.is_empty();
            if need_dispatch {
                crate::runtime::g_dispatcher().add_task(
                    crate::runtime::dispatcher::Task::new(move || {
                        for (sid, from_lua) in &death_script_ids {
                            crate::events::dispatch::fire_death_script(*sid, *from_lua, creature_id_copy, last_hit_copy, most_dmg_copy, corpse_server_id_copy, corpse_pos_copy, corpse_tile_idx_copy);
                        }
                        for (sid, from_lua) in &kill_script_ids_last {
                            crate::events::dispatch::fire_kill_script(*sid, *from_lua, last_hit_copy, creature_id_copy);
                        }
                        for (sid, from_lua) in &kill_script_ids_most {
                            crate::events::dispatch::fire_kill_script(*sid, *from_lua, most_dmg_copy, creature_id_copy);
                        }
                    }),
                );
            }
        } else {
            // Collect corpse item id and loot before removing the creature.
            let (look_corpse, loot_items, loot_drop, skill_loss) = {
                let Some(m) = self.creatures.get(&creature_id).and_then(|c| c.as_monster()) else {
                    self.remove_creature_from_world(creature_id);
                    return;
                };
                (m.mtype_info.look_corpse, m.mtype_info.loot_items.clone(), m.base.loot_drop, m.base.skill_loss)
            };

            // Notify spawn system so monster slot becomes available for respawn.
            let now_ms = crate::util::otsys_time();
            for spawn in &mut self.spawns.spawn_list {
                if spawn.spawned_map.values().any(|&cid| cid == creature_id) {
                    spawn.spawned_map.retain(|_, &mut cid| cid != creature_id);
                    // Mark last_spawn time for respawn interval tracking.
                    for sb in spawn.spawn_map.values_mut() {
                        if sb.last_spawn == 0 {
                            sb.last_spawn = now_ms;
                        }
                    }
                    break;
                }
            }

            self.remove_creature_from_world(creature_id);

            // Place corpse with loot (0 = no corpse defined → POFF effect).
            if look_corpse > 0 {
                let mut corpse = crate::map::tile::MapItem { server_id: look_corpse, ..crate::map::tile::MapItem::default() };
                let killer_suppresses_loot = last_hit_id
                    .and_then(|id| self.get_player(id))
                    .map(|p| p.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GENERATE_LOOT))
                    .unwrap_or(false);
                if loot_drop && skill_loss && !killer_suppresses_loot {
                    let rate_loot = crate::config::g_config().get_number(crate::config::IntegerConfig::RateLoot);
                    if rate_loot > 0 {
                        for lb in &loot_items {
                            generate_loot_recursive(lb, &mut corpse.children);
                        }
                    }
                }
                // Send "Loot of X: ..." message to the killing player.
                if !corpse.children.is_empty() {
                    if let Some(attacker_id) = last_hit_id {
                        if self.get_player(attacker_id).is_some() {
                            let monster_name = self.creatures.get(&creature_id)
                                .and_then(|c| c.as_monster())
                                .map(|m| m.get_name().to_string())
                                .unwrap_or_default();
                            let loot_desc = describe_loot_items(&corpse.children, self);
                            let msg = format!("Loot of a {}: {}.", monster_name, loot_desc);
                            self.send_text_message(attacker_id, MESSAGE_INFO_DESCR, msg);
                        }
                    }
                }
                self.place_map_item_on_tile(pos, corpse);
            } else {
                self.add_magic_effect(pos, CONST_ME_POFF);
            }

            // Dispatch monster onDeath + killer onKill Lua events.
            let need_dispatch = !kill_script_ids_last.is_empty()
                || !kill_script_ids_most.is_empty()
                || !death_script_ids.is_empty();
            if need_dispatch {
                crate::runtime::g_dispatcher().add_task(
                    crate::runtime::dispatcher::Task::new(move || {
                        for (sid, from_lua) in &kill_script_ids_last {
                            crate::events::dispatch::fire_kill_script(*sid, *from_lua, last_hit_copy, creature_id_copy);
                        }
                        for (sid, from_lua) in &kill_script_ids_most {
                            crate::events::dispatch::fire_kill_script(*sid, *from_lua, most_dmg_copy, creature_id_copy);
                        }
                        for (sid, from_lua) in &death_script_ids {
                            crate::events::dispatch::fire_death_script(*sid, *from_lua, creature_id_copy, last_hit_copy, most_dmg_copy, 0, crate::map::Position::default(), -1);
                        }
                    }),
                );
            }
        }
    }

    /// Give experience to a player attacker and broadcast the text/stats.
    fn give_experience(&mut self, attacker_id: CreatureId, gained: u64, target_pos: Position) {
        use crate::world::vocation::g_vocations;

        let (level, exp_before, health_max, mana_max, cap, voc_id) = {
            let Some(player) = self.get_player(attacker_id) else { return };
            if player.has_flag(crate::creatures::player::PLAYER_FLAG_NOT_GAIN_EXPERIENCE) || gained == 0 {
                return;
            }
            (player.level, player.experience, player.base.health_max, player.mana_max, player.capacity, player.vocation_id)
        };

        let new_exp = exp_before + gained;

        // Check for level-up(s).
        let mut new_level = level;
        let mut new_health_max = health_max;
        let mut new_mana_max = mana_max;
        let mut new_cap = cap;

        let voc = g_vocations().get_vocation(voc_id).cloned();
        loop {
            let needed = Player::get_exp_for_level((new_level + 1) as u64);
            if new_exp < needed { break; }
            new_level += 1;
            if let Some(ref v) = voc {
                new_health_max = (new_health_max + v.gain_hp as i32).max(0);
                new_mana_max = new_mana_max.saturating_add(v.gain_mana);
                new_cap = new_cap.saturating_add(v.gain_cap);
            }
        }

        // Apply changes.
        if let Some(player) = self.get_player_mut(attacker_id) {
            player.experience = new_exp;
            player.level = new_level;
            player.base.health_max = new_health_max;
            player.mana_max = new_mana_max;
            player.capacity = new_cap;
        }

        // Animated text at target location.
        self.add_animated_text(target_pos, TEXTCOLOR_WHITE, &gained.to_string());

        // Text message to the player.
        let leveled_up = new_level > level;
        let msg = if leveled_up {
            format!("You gained {} experience points. You advanced from level {} to level {}.",
                    gained, level, new_level)
        } else {
            format!("You gained {} experience point{}.", gained, if gained != 1 { "s" } else { "" })
        };
        self.send_combat_text_message(attacker_id, 26, target_pos, gained as u32, TEXTCOLOR_WHITE, 0, TEXTCOLOR_NONE, msg);

        if leveled_up {
            // Send level-up magic effect + 0x82 magic effect at player position.
            let ppos = self.get_player(attacker_id).map(|p| p.base.position).unwrap_or_default();
            self.add_magic_effect(ppos, CONST_ME_MAGIC_BLUE);
            // Send updated stats.
            self.send_player_stats(attacker_id);
            // Fire onAdvance (SKILL_LEVEL = 8) for each level gained.
            let aid = attacker_id;
            let prev = level;
            let next = new_level;
            crate::runtime::g_dispatcher().add_task(crate::runtime::dispatcher::Task::new(move || {
                for l in prev..next {
                    crate::events::dispatch::execute_creature_event_advance(aid, 8, l, l + 1);
                }
            }));
        }
    }

    /// Full player death: skill/exp loss, teleport to temple, send packets.
    /// Mirrors C++ `Player::death`.
    fn execute_player_death(&mut self, player_id: CreatureId, _last_hit_id: Option<CreatureId>) {
        use crate::config::{g_config, IntegerConfig};
        use crate::world::vocation::g_vocations;

        let (skill_loss, level, experience, level_percent, voc_id, skull, health_max, mana_max,
             blessed_count, is_promoted, temple_pos, current_pos)
        = {
            let Some(player) = self.get_player(player_id) else { return };
            let temple_pos = {
                let town_id = player.town_id;
                self.map.get_town_temple_pos(town_id)
                    .unwrap_or(player.base.position)
            };
            (
                player.skill_loss,
                player.level,
                player.experience,
                player.level_percent,
                player.vocation_id,
                player.base.skull,
                player.base.health_max,
                player.mana_max,
                player.blessings.count_ones(),
                player.vocation_id > 3,
                temple_pos,
                player.base.position,
            )
        };

        let death_lose_percent_cfg = g_config().get_number(IntegerConfig::DeathLosePercent);

        let loss_pct: f64 = if skill_loss {
            if death_lose_percent_cfg != -1 {
                let pct = death_lose_percent_cfg as f64
                    - if is_promoted { 3.0 } else { 0.0 }
                    - blessed_count as f64;
                pct.max(0.0) / 100.0
            } else if level >= 25 {
                let tmp_level = level as f64 + (level_percent as f64 / 100.0);
                let pct = ((tmp_level + 50.0) * 50.0 * ((tmp_level * tmp_level) - (5.0 * tmp_level) + 8.0))
                    / experience as f64;
                let reduction = if is_promoted { 30.0 } else { 0.0 } + blessed_count as f64 * 8.0;
                pct * (1.0 - reduction / 100.0) / 100.0
            } else {
                10.0 / 100.0
            }
        } else {
            0.0
        };

        if skill_loss && loss_pct > 0.0 {
            let exp_loss = (experience as f64 * loss_pct) as u64;

            // Level loss
            let new_exp = experience.saturating_sub(exp_loss);
            let mut new_level = level;
            let mut new_health_max = health_max;
            let mut new_mana_max = mana_max;
            let voc = g_vocations().get_vocation(voc_id).cloned();

            if voc_id == 0 || level > 7 {
                while new_level > 1 && new_exp < Player::get_exp_for_level(new_level as u64) {
                    new_level -= 1;
                    if let Some(ref v) = voc {
                        new_health_max = (new_health_max - v.gain_hp as i32).max(0);
                        new_mana_max = new_mana_max.saturating_sub(v.gain_mana);
                    }
                }
            }

            let new_level_percent = {
                let curr = Player::get_exp_for_level(new_level as u64);
                let next = Player::get_exp_for_level(new_level as u64 + 1);
                if next > curr {
                    Player::get_percent_level(new_exp.saturating_sub(curr), next - curr)
                } else {
                    0
                }
            };

            let old_level = level;
            if let Some(player) = self.get_player_mut(player_id) {
                player.experience = new_exp;
                player.level = new_level;
                player.level_percent = new_level_percent;
                player.base.health_max = new_health_max;
                player.mana_max = new_mana_max;

                // Reset health/mana (black skull gets 40 hp / 0 mana).
                if skull == Skull::Black {
                    player.base.health = 40;
                    player.mana = 0;
                } else {
                    player.base.health = new_health_max;
                    player.mana = new_mana_max;
                }

                // Remove non-persistent conditions.
                player.base.conditions.retain(|c| !c.is_persistent());

                player.base.position = temple_pos;
            }

            if old_level != new_level {
                let msg = format!("You were downgraded from Level {} to Level {}.", old_level, new_level);
                self.send_text_message(player_id, MESSAGE_STATUS_DEFAULT, msg);
            }
        } else {
            // No skill loss: just heal and teleport.
            if let Some(player) = self.get_player_mut(player_id) {
                player.base.health = player.base.health_max;
                player.base.conditions.retain(|c| !c.is_persistent());
                player.base.position = temple_pos;
            }
        }

        // Move on map.
        self.move_creature_position(player_id, current_pos, temple_pos);

        // Send updated health bar, stats, and re-login window (0x28).
        self.add_creature_health(player_id);
        self.send_player_stats(player_id);
        send_packet_to_player(player_id, |output: &mut OutputMessage| {
            output.add_byte(0x28);
        });
    }

    /// Send player stats (0xA0) packet to the player.
    pub fn send_player_stats(&mut self, player_id: CreatureId) {
        let Some(player) = self.get_player(player_id) else { return };
        // Snapshot mutable data into owned values.
        let health = player.base.health;
        let health_max = player.base.health_max;
        let exp = player.experience;
        let level = player.level;
        let level_percent = player.level_percent;
        let mana = player.mana;
        let mana_max = player.mana_max;
        let magic_level = player.get_magic_level();
        let mag_level_percent = player.mag_level_percent;
        let soul = player.soul;
        let stamina = player.stamina_minutes;
        let free_cap = player.get_free_capacity();
        send_packet_to_player(player_id, move |output: &mut OutputMessage| {
            output.add_byte(0xA0);
            output.add_u16(health.min(0xFFFF) as u16);
            output.add_u16(health_max.min(0xFFFF) as u16);
            output.add_u32(free_cap);
            output.add_u32(exp.min(0x7FFF_FFFF) as u32);
            output.add_u16(level as u16);
            output.add_byte(level_percent);
            output.add_u16(mana.min(0xFFFF) as u16);
            output.add_u16(mana_max.min(0xFFFF) as u16);
            output.add_byte(magic_level.min(0xFF) as u8);
            output.add_byte(mag_level_percent);
            output.add_byte(soul);
            output.add_u16(stamina);
        });
    }

    /// Remove a creature from the game world and notify spectators (0x6C).
    /// Mirrors C++ `Game::removeCreature` (the death/disappear path).
    pub fn remove_creature_from_world(&mut self, creature_id: CreatureId) {
        let Some(creature) = self.creatures.get(&creature_id) else { return };
        let pos = creature.position();
        let is_player = creature.is_player();

        // Summon bookkeeping: detach this creature from its master's summon list
        // (so the master can re-summon), and orphan any summons it owns.
        let (master_id, own_summons) = self
            .get_creature(creature_id)
            .map(|c| (c.base().master_id, c.base().summon_ids.clone()))
            .unwrap_or((None, Vec::new()));
        if let Some(mid) = master_id {
            if let Some(master) = self.get_creature_mut(mid) {
                master.base_mut().summon_ids.retain(|&id| id != creature_id);
            }
        }
        for sid in own_summons {
            if let Some(summon) = self.get_creature_mut(sid) {
                summon.base_mut().master_id = None;
            }
        }

        // Compute stackpos BEFORE removing from tile
        let stackpos = self.map.get_tile(pos)
            .map(|t| t.get_client_index_of_creature(creature_id))
            .unwrap_or(-1);

        // Get spectators BEFORE removing
        let spectators: Vec<CreatureId> = self.map.get_spectators(pos, true, true, 0, 0, 0, 0)
            .into_iter()
            .filter(|&id| id != creature_id)
            .collect();

        // Remove from tile and data structures
        self.map.remove_creature_from_tile(pos, creature_id, is_player);
        if let Some(creature) = self.creatures.remove(&creature_id) {
            if let Some(player) = creature.as_player() {
                self.player_name_to_id.remove(&player.name);
                self.player_guid_to_id.remove(&player.guid);
            }
        }
        self.remove_creature_check(creature_id);

        // Send 0x6C (remove creature) to all spectator players
        if stackpos >= 0 {
            let sp = stackpos as u8;
            for spec_id in spectators {
                send_packet_to_player(spec_id, move |output: &mut OutputMessage| {
                    output.add_byte(0x6C);
                    if sp < 10 {
                        output.add_position(pos.x, pos.y, pos.z);
                        output.add_byte(sp);
                    } else {
                        output.add_u16(0xFFFF);
                        output.add_u32(creature_id);
                    }
                });
            }
        }
    }

    // ── Line of Sight delegates ─────────────────────────────────────────────

    pub fn is_sight_clear(&self, from_pos: Position, to_pos: Position, same_floor: bool) -> bool {
        self.map.is_sight_clear(from_pos, to_pos, same_floor, &self.items)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn can_throw_object_to(
        &self,
        from_pos: Position,
        to_pos: Position,
        check_line_of_sight: bool,
        same_floor: bool,
        range_x: i32,
        range_y: i32,
    ) -> bool {
        self.map.can_throw_object_to(from_pos, to_pos, check_line_of_sight, same_floor, range_x, range_y, &self.items)
    }

    // ── Money operations (port of game.cpp removeMoney / addMoney) ──────────

    pub fn remove_money(&mut self, player_id: CreatureId, amount: u64) -> bool {
        if amount == 0 {
            return true;
        }

        let Some(player) = self.get_player(player_id) else { return false };

        let mut money_map: std::collections::BTreeMap<u64, Vec<(usize, usize)>> = std::collections::BTreeMap::new();
        let mut money_count: u64 = 0;

        for slot in crate::creatures::player::CONST_SLOT_FIRST..=crate::creatures::player::CONST_SLOT_LAST {
            if let Some(ref item) = player.inventory_items[slot] {
                let worth_per = self.items.get_item_type(usize::from(item.server_id)).worth;
                if worth_per > 0 {
                    let stack_worth = worth_per * item.count.max(1) as u64;
                    money_count += stack_worth;
                    money_map.entry(stack_worth).or_default().push((slot, usize::MAX));
                }
                Self::collect_money_from_children(item, &self.items, slot, &mut money_map, &mut money_count);
            }
        }

        if money_count < amount {
            return false;
        }

        let mut remaining = amount;

        let entries: Vec<(u64, Vec<(usize, usize)>)> = money_map.into_iter().collect();

        for (worth, locations) in &entries {
            if remaining == 0 {
                break;
            }
            for &(slot, child_idx) in locations {
                if remaining == 0 {
                    break;
                }
                if *worth <= remaining {
                    remaining -= worth;
                    self.remove_money_item(player_id, slot, child_idx);
                } else {
                    let item_info = self.get_money_item_info(player_id, slot, child_idx);
                    if let Some((server_id, count)) = item_info {
                        let worth_per = self.items.get_item_type(usize::from(server_id)).worth;
                        if worth_per > 0 {
                            let remove_count = ((remaining as f64) / (worth_per as f64)).ceil() as u16;
                            let change = (worth_per * remove_count as u64).saturating_sub(remaining);
                            if remove_count >= count {
                                self.remove_money_item(player_id, slot, child_idx);
                            } else {
                                self.reduce_money_item(player_id, slot, child_idx, remove_count);
                            }
                            remaining = 0;
                            if change > 0 {
                                self.add_money(player_id, change);
                            }
                        }
                    }
                    break;
                }
            }
        }

        true
    }

    fn collect_money_from_children(
        item: &crate::map::tile::MapItem,
        items: &Items,
        slot: usize,
        money_map: &mut std::collections::BTreeMap<u64, Vec<(usize, usize)>>,
        money_count: &mut u64,
    ) {
        for (i, child) in item.children.iter().enumerate() {
            let worth_per = items.get_item_type(usize::from(child.server_id)).worth;
            if worth_per > 0 {
                let stack_worth = worth_per * child.count.max(1) as u64;
                *money_count += stack_worth;
                money_map.entry(stack_worth).or_default().push((slot, i));
            }
            if !child.children.is_empty() {
                Self::collect_money_from_children(child, items, slot, money_map, money_count);
            }
        }
    }

    fn get_money_item_info(&self, player_id: CreatureId, slot: usize, child_idx: usize) -> Option<(u16, u16)> {
        let player = self.get_player(player_id)?;
        let item = player.inventory_items[slot].as_ref()?;
        if child_idx == usize::MAX {
            Some((item.server_id, item.count.max(1)))
        } else {
            let child = item.children.get(child_idx)?;
            Some((child.server_id, child.count.max(1)))
        }
    }

    fn remove_money_item(&mut self, player_id: CreatureId, slot: usize, child_idx: usize) {
        let Some(player) = self.get_player_mut(player_id) else { return };
        if child_idx == usize::MAX {
            player.inventory[slot] = None;
            player.inventory_count[slot] = 1;
            player.inventory_items[slot] = None;
        } else if let Some(ref mut item) = player.inventory_items[slot] {
            if child_idx < item.children.len() {
                item.children.remove(child_idx);
            }
        }
    }

    fn reduce_money_item(&mut self, player_id: CreatureId, slot: usize, child_idx: usize, remove_count: u16) {
        let Some(player) = self.get_player_mut(player_id) else { return };
        if child_idx == usize::MAX {
            if let Some(ref mut item) = player.inventory_items[slot] {
                item.count = item.count.saturating_sub(remove_count);
                player.inventory_count[slot] = item.count.max(1);
            }
        } else if let Some(ref mut item) = player.inventory_items[slot] {
            if let Some(child) = item.children.get_mut(child_idx) {
                child.count = child.count.saturating_sub(remove_count);
            }
        }
    }

    pub fn add_money(&mut self, player_id: CreatureId, mut amount: u64) {
        if amount == 0 {
            return;
        }

        let currency_items: Vec<(u64, u16)> = self
            .items
            .get_currency_items()
            .iter()
            .map(|(rev_worth, &item_id)| (rev_worth.0, item_id))
            .collect();

        for (worth, item_id) in &currency_items {
            let mut currency_coins = amount / worth;
            if currency_coins == 0 {
                continue;
            }

            amount -= currency_coins * worth;
            while currency_coins > 0 {
                let count = std::cmp::min(100, currency_coins) as u16;

                let item = crate::map::tile::MapItem {
                    server_id: *item_id,
                    count,
                    ..crate::map::tile::MapItem::default()
                };

                if !self.add_money_item_to_player(player_id, item.clone()) {
                    if let Some(player) = self.get_player(player_id) {
                        let pos = player.base.position;
                        self.place_map_item_on_tile(pos, item);
                    }
                }

                currency_coins -= count as u64;
            }
        }
    }

    fn add_money_item_to_player(&mut self, player_id: CreatureId, item: crate::map::tile::MapItem) -> bool {
        use crate::creatures::player::{CONST_SLOT_FIRST, CONST_SLOT_LAST, CONST_SLOT_BACKPACK};
        let Some(player) = self.get_player_mut(player_id) else { return false };
        if let Some(slot) = (CONST_SLOT_FIRST..=CONST_SLOT_LAST).find(|&s| player.inventory[s].is_none()) {
            player.inventory[slot] = Some(item.server_id);
            player.inventory_count[slot] = item.count.max(1);
            player.inventory_items[slot] = Some(item);
            return true;
        }
        if let Some(Some(bp)) = player.inventory_items.get_mut(CONST_SLOT_BACKPACK) {
            bp.children.insert(0, item);
            return true;
        }
        false
    }

}
