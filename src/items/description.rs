use crate::items::ItemType;

pub const WEAPON_NONE: u8 = 0;
pub const WEAPON_SWORD: u8 = 1;
pub const WEAPON_CLUB: u8 = 2;
pub const WEAPON_AXE: u8 = 3;
pub const WEAPON_SHIELD: u8 = 4;
pub const WEAPON_DISTANCE: u8 = 5;
pub const WEAPON_WAND: u8 = 6;
pub const WEAPON_AMMO: u8 = 7;

pub const ITEM_KIND_RUNE: u8 = 11; // ItemKind::Rune

fn get_name_description(it: &ItemType, count: u32, add_article: bool) -> String {
    if !it.name.is_empty() {
        if it.stackable && count > 1 && it.show_count {
            format!("{} {}", count, if it.plural_name.is_empty() { &it.name } else { &it.plural_name })
        } else if add_article && !it.article.is_empty() {
            format!("{} {}", it.article, it.name)
        } else {
            it.name.clone()
        }
    } else {
        format!("an item of type {}", it.id)
    }
}

fn format_weight(weight: u32) -> String {
    if weight < 10 {
        format!("0.0{}", weight)
    } else if weight < 100 {
        format!("0.{}", weight)
    } else {
        let whole = weight / 100;
        let frac = weight % 100;
        if frac == 0 {
            format!("{}.00", whole)
        } else if frac < 10 {
            format!("{}.0{}", whole, frac)
        } else {
            format!("{}.{}", whole, frac)
        }
    }
}

fn get_combat_name(combat_type: u8) -> &'static str {
    match combat_type {
        1 => "physical",
        2 => "energy",
        4 => "earth",
        8 => "fire",
        16 => "undefined",
        32 => "life drain",
        64 => "mana drain",
        128 => "healing",
        0 => "death",
        _ => "unknown",
    }
}

pub fn get_item_description(it: &ItemType, look_distance: i32, count: u32) -> String {
    let mut s = get_name_description(it, count, true);

    let is_rune = it.kind == crate::items::ItemKind::Rune;

    if is_rune {
        if it.rune_level > 0 || it.rune_mag_level > 0 || !it.vocation_string.is_empty() {
            s.push_str(".\n");
            if count > 1 && it.show_count {
                s.push_str("They");
            } else {
                s.push_str("It");
            }
            s.push_str(" can only be used by ");
            if !it.vocation_string.is_empty() {
                s.push_str(&it.vocation_string);
            } else {
                s.push_str("players");
            }
            if it.rune_level > 0 {
                s.push_str(&format!(" with level {} or higher", it.rune_level));
            }
            if it.rune_mag_level > 0 {
                if it.rune_level > 0 {
                    s.push_str(" and");
                } else {
                    s.push_str(" with");
                }
                s.push_str(&format!(" magic level {} or higher", it.rune_mag_level));
            }
        }
    } else if it.weapon_type != WEAPON_NONE {
        let is_distance = it.weapon_type == WEAPON_DISTANCE;
        let is_ammo = it.weapon_type == WEAPON_AMMO;

        if is_distance && it.ammo_type != 0 {
            s.push_str(&format!(
                " (Range:{}, Atk{:+}, Hit%{:+})",
                it.shoot_range,
                it.attack,
                it.hit_chance
            ));
        } else if is_distance && it.ammo_type == 0 {
            s.push_str(&format!(" (Atk:{}, Def:{})", it.attack, it.defense));
        } else if is_ammo {
            s.push_str(&format!(" (Atk{:+}, Hit%{:+})", it.attack, it.hit_chance));
        } else if it.weapon_type != WEAPON_SHIELD && it.weapon_type != WEAPON_WAND {
            let mut stats = String::new();
            if it.element_damage > 0 && it.element_type > 0 {
                let phys = it.attack.saturating_sub(it.element_damage as i32);
                stats.push_str(&format!(
                    "Atk:{} physical + {} {}",
                    phys,
                    it.element_damage,
                    get_combat_name(it.element_type)
                ));
            } else {
                stats.push_str(&format!("Atk:{}", it.attack));
            }
            if it.attack_speed > 0 {
                let spd = it.attack_speed as f64 / 1000.0;
                stats.push_str(&format!(", Atk Spd:{:.2}s", spd));
            }
            if it.extra_defense != 0 {
                stats.push_str(&format!(", Def:{} {:+}", it.defense, it.extra_defense));
            } else {
                stats.push_str(&format!(", Def:{}", it.defense));
            }
            s.push_str(&format!(" ({})", stats));
        } else if it.weapon_type == WEAPON_SHIELD {
            if it.extra_defense != 0 {
                s.push_str(&format!(" (Def:{} {:+})", it.defense, it.extra_defense));
            } else {
                s.push_str(&format!(" (Def:{})", it.defense));
            }
        } else if it.weapon_type == WEAPON_WAND {
            s.push_str(&format!(" (Range:{})", it.shoot_range));
        }
    } else if it.armor != 0 || it.show_attributes {
        s.push_str(&format!(" (Arm:{})", it.armor));
    } else if it.kind == crate::items::ItemKind::Container {
        s.push_str(&format!(" (Vol:{})", it.max_items));
    } else if it.speed != 0 {
        s.push_str(&format!(" (speed {:+})", (it.speed as i32) >> 1));
    } else if it.kind == crate::items::ItemKind::Key {
        s.push_str(" (Key:0000)");
    }

    if it.show_charges && it.charges > 0 {
        if count > 1 {
            s.push_str(&format!(
                " that have {} charge{} left",
                it.charges,
                if it.charges != 1 { "s" } else { "" }
            ));
        } else {
            s.push_str(&format!(
                " that has {} charge{} left",
                it.charges,
                if it.charges != 1 { "s" } else { "" }
            ));
        }
    }

    if it.show_duration && it.decay_time > 0 {
        s.push_str(" that is brand-new");
    }

    s.push('.');

    if it.wield_info != 0 {
        s.push_str("\nIt can only be wielded properly by ");
        if it.wield_info & 0x04 != 0 {
            s.push_str("premium ");
        }
        if !it.vocation_string.is_empty() {
            s.push_str(&it.vocation_string);
        } else {
            s.push_str("players");
        }
        if it.min_req_level > 0 || it.min_req_magic_level > 0 {
            s.push_str(" of");
            if it.min_req_level > 0 {
                s.push_str(&format!(" level {} or higher", it.min_req_level));
            }
            if it.min_req_magic_level > 0 {
                if it.min_req_level > 0 {
                    s.push_str(" and");
                }
                s.push_str(&format!(" magic level {} or higher", it.min_req_magic_level));
            }
        }
        s.push('.');
    }

    if look_distance <= 1 && it.pickupable && it.weight > 0 {
        let total_weight = it.weight * count;
        if count > 1 && it.show_count {
            s.push_str(&format!("\nThey weigh {} oz.", format_weight(total_weight)));
        } else {
            s.push_str(&format!("\nIt weighs {} oz.", format_weight(total_weight)));
        }
    }

    if !it.description.is_empty() && look_distance <= 1 {
        s.push('\n');
        s.push_str(&it.description);
    }

    s
}
