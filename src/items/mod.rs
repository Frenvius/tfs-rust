pub mod description;
pub mod special;

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tracing::warn;

use crate::io::otb::{Identifier, Loader, OtbError, PropStream};
use crate::map::tile::{
    TILESTATE_FLOORCHANGE_DOWN, TILESTATE_FLOORCHANGE_EAST, TILESTATE_FLOORCHANGE_EAST_ALT,
    TILESTATE_FLOORCHANGE_NORTH, TILESTATE_FLOORCHANGE_SOUTH, TILESTATE_FLOORCHANGE_SOUTH_ALT,
    TILESTATE_FLOORCHANGE_WEST,
};
use crate::util::json5::{self, Json5LoadError};

use std::sync::{Arc, OnceLock};

static G_ITEMS: OnceLock<Arc<Items>> = OnceLock::new();

pub fn init_items(items: Arc<Items>) {
    G_ITEMS.set(items).unwrap_or_else(|_| panic!("items already initialized"));
}

pub fn g_items() -> &'static Items {
    G_ITEMS.get().expect("items not initialized")
}

const OTBI_IDENTIFIER: Identifier = *b"OTBI";
const ROOT_ATTR_VERSION: u8 = 0x01;
const ITEM_ATTR_SERVER_ID: u8 = 0x10;
const ITEM_ATTR_CLIENT_ID: u8 = 0x11;
const ITEM_ATTR_SPEED: u8 = 0x14;
const ITEM_ATTR_LIGHT2: u8 = 0x2A;
const ITEM_ATTR_TOP_ORDER: u8 = 0x2B;
const ITEM_ATTR_WARE_ID: u8 = 0x2D;
const CLIENT_VERSION_860_OLD: u32 = 19;
const FLAG_BLOCK_SOLID: u32 = 1 << 0;
const FLAG_BLOCK_PROJECTILE: u32 = 1 << 1;
const FLAG_BLOCK_PATH_FIND: u32 = 1 << 2;
const FLAG_HAS_HEIGHT: u32 = 1 << 3;
const FLAG_USEABLE: u32 = 1 << 4;
const FLAG_PICKUPABLE: u32 = 1 << 5;
const FLAG_MOVEABLE: u32 = 1 << 6;
const FLAG_STACKABLE: u32 = 1 << 7;
const FLAG_ALWAYS_ON_TOP: u32 = 1 << 13;
const FLAG_READABLE: u32 = 1 << 14;
const FLAG_ROTATABLE: u32 = 1 << 15;
const FLAG_HANGABLE: u32 = 1 << 16;
const FLAG_VERTICAL: u32 = 1 << 17;
const FLAG_HORIZONTAL: u32 = 1 << 18;
const FLAG_ALLOW_DIST_READ: u32 = 1 << 20;
const FLAG_LOOK_THROUGH: u32 = 1 << 23;
const FLAG_FORCE_USE: u32 = 1 << 26;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ItemGroup {
    #[default]
    None = 0,
    Ground = 1,
    Container = 2,
    Weapon = 3,
    Ammunition = 4,
    Armor = 5,
    Charges = 6,
    Teleport = 7,
    MagicField = 8,
    Writeable = 9,
    Key = 10,
    Splash = 11,
    Fluid = 12,
    Door = 13,
    Deprecated = 14,
}

impl TryFrom<u8> for ItemGroup {
    type Error = ItemsError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Ground),
            2 => Ok(Self::Container),
            3 => Ok(Self::Weapon),
            4 => Ok(Self::Ammunition),
            5 => Ok(Self::Armor),
            6 => Ok(Self::Charges),
            7 => Ok(Self::Teleport),
            8 => Ok(Self::MagicField),
            9 => Ok(Self::Writeable),
            10 => Ok(Self::Key),
            11 => Ok(Self::Splash),
            12 => Ok(Self::Fluid),
            13 => Ok(Self::Door),
            14 => Ok(Self::Deprecated),
            group => Err(ItemsError::UnknownGroup(group)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemKind {
    #[default]
    None,
    Depot,
    Mailbox,
    TrashHolder,
    Container,
    Door,
    MagicField,
    Teleport,
    Bed,
    Key,
    Rune,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemAttribute {
    pub key: String,
    pub value: Value,
    pub attributes: Vec<ItemAttribute>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemType {
    pub id: u16,
    pub client_id: u16,
    pub ware_id: u16,
    pub speed: u16,
    pub weight: u32,
    pub level_door: u32,
    pub decay_time: u32,
    pub charges: u32,
    pub worth: u64,
    pub max_hit_chance: i32,
    pub decay_to: i32,
    pub attack: i32,
    pub defense: i32,
    pub extra_defense: i32,
    pub armor: i32,
    pub group: ItemGroup,
    pub kind: ItemKind,
    pub name: String,
    pub article: String,
    pub plural_name: String,
    pub description: String,
    pub light_level: u8,
    pub light_color: u8,
    pub always_on_top_order: u8,
    pub floor_change: u32,
    pub hit_chance: i8,
    pub shoot_range: u8,
    pub rotate_to: u16,
    pub max_text_len: u16,
    pub write_once_item_id: u16,
    pub transform_to_free: u16,
    pub transform_to_on_use: [u16; 2],
    pub bed_partner_dir: u8,
    pub transform_equip_to: u16,
    pub transform_de_equip_to: u16,
    pub destroy_to: u16,
    pub max_items: u16,
    pub slot_position: u16,
    pub weapon_type: u8,
    pub ammo_type: u8,
    pub corpse_type: u8,
    pub fluid_source: u8,
    pub attack_speed: u32,
    pub wield_info: u32,
    pub min_req_level: u32,
    pub min_req_magic_level: u32,
    pub rune_mag_level: i32,
    pub rune_level: i32,
    pub rune_spell_name: String,
    pub vocation_string: String,
    pub element_damage: u16,
    pub element_type: u8,
    pub stackable: bool,
    pub block_solid: bool,
    pub block_projectile: bool,
    pub block_path_find: bool,
    pub has_height: bool,
    pub force_serialize: bool,
    pub ignore_blocking: bool,
    pub no_field_block_path: bool,
    pub show_duration: bool,
    pub show_charges: bool,
    pub show_attributes: bool,
    pub show_count: bool,
    pub replaceable: bool,
    pub useable: bool,
    pub pickupable: bool,
    pub moveable: bool,
    pub can_write_text: bool,
    pub store_item: bool,
    pub stop_time: bool,
    pub walk_stack: bool,
    pub always_on_top: bool,
    pub is_vertical: bool,
    pub is_horizontal: bool,
    pub is_hangable: bool,
    pub allow_dist_read: bool,
    pub rotatable: bool,
    pub can_read_text: bool,
    pub look_through: bool,
    pub force_use: bool,
    pub supports_hangable: bool,
    pub attributes: Vec<ItemAttribute>,
}

impl Default for ItemType {
    fn default() -> Self {
        Self {
            id: 0,
            client_id: 0,
            ware_id: 0,
            speed: 0,
            weight: 0,
            level_door: 0,
            decay_time: 0,
            charges: 0,
            worth: 0,
            max_hit_chance: -1,
            decay_to: -1,
            attack: 0,
            defense: 0,
            extra_defense: 0,
            armor: 0,
            group: ItemGroup::None,
            kind: ItemKind::None,
            name: String::new(),
            article: String::new(),
            plural_name: String::new(),
            description: String::new(),
            light_level: 0,
            light_color: 0,
            always_on_top_order: 0,
            floor_change: 0,
            hit_chance: 0,
            shoot_range: 1,
            rotate_to: 0,
            max_text_len: 0,
            write_once_item_id: 0,
            transform_to_free: 0,
            transform_to_on_use: [0, 0],
            bed_partner_dir: 0,
            transform_equip_to: 0,
            transform_de_equip_to: 0,
            destroy_to: 0,
            max_items: 8,
            slot_position: SLOTP_HAND,
            weapon_type: 0,
            ammo_type: 0,
            corpse_type: 0,
            fluid_source: 0,
            attack_speed: 0,
            wield_info: 0,
            min_req_level: 0,
            min_req_magic_level: 0,
            rune_mag_level: 0,
            rune_level: 0,
            rune_spell_name: String::new(),
            vocation_string: String::new(),
            element_damage: 0,
            element_type: 0,
            stackable: false,
            block_solid: false,
            block_projectile: false,
            block_path_find: false,
            has_height: false,
            force_serialize: false,
            ignore_blocking: false,
            no_field_block_path: false,
            show_duration: false,
            show_charges: false,
            show_attributes: false,
            show_count: false,
            replaceable: true,
            useable: false,
            pickupable: false,
            moveable: false,
            can_write_text: false,
            store_item: false,
            stop_time: false,
            walk_stack: true,
            always_on_top: false,
            is_vertical: false,
            is_horizontal: false,
            is_hangable: false,
            allow_dist_read: false,
            rotatable: false,
            can_read_text: false,
            look_through: false,
            force_use: false,
            supports_hangable: false,
            attributes: Vec::new(),
        }
    }
}

impl ItemType {
    pub fn is_ground_tile(&self) -> bool {
        self.group == ItemGroup::Ground
    }

    pub fn is_fluid_container(&self) -> bool {
        self.group == ItemGroup::Fluid
    }

    pub fn is_splash(&self) -> bool {
        self.group == ItemGroup::Splash
    }

    pub fn has_sub_type(&self) -> bool {
        self.is_fluid_container() || self.is_splash() || self.stackable
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Items {
    items: Vec<ItemType>,
    major_version: u32,
    minor_version: u32,
    build_number: u32,
    client_id_to_server_id: ClientIdToServerIdMap,
    name_to_item_id: BTreeMap<String, u16>,
    currency_items: BTreeMap<std::cmp::Reverse<u64>, u16>,
}

impl Items {
    pub fn new() -> Self {
        Self {
            items: vec![ItemType::default()],
            major_version: 0,
            minor_version: 0,
            build_number: 0,
            client_id_to_server_id: ClientIdToServerIdMap::default(),
            name_to_item_id: BTreeMap::new(),
            currency_items: BTreeMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.items.push(ItemType::default());
        self.major_version = 0;
        self.minor_version = 0;
        self.build_number = 0;
        self.client_id_to_server_id.clear();
        self.name_to_item_id.clear();
        self.currency_items.clear();
    }

    pub fn load_from_otb(&mut self, path: impl AsRef<Path>) -> Result<(), ItemsError> {
        let mut loader = Loader::from_path(path, OTBI_IDENTIFIER)?;
        self.load_with_loader(&mut loader)
    }

    pub fn load_from_otb_bytes(&mut self, bytes: Vec<u8>) -> Result<(), ItemsError> {
        let mut loader = Loader::from_bytes(bytes, OTBI_IDENTIFIER)?;
        self.load_with_loader(&mut loader)
    }

    pub fn load_from_json5(&mut self, path: impl AsRef<Path>) -> Result<(), ItemsError> {
        let data: ItemsJson5 = json5::load_from_path(path)?;
        self.apply_json5(data)
    }

    pub fn major_version(&self) -> u32 {
        self.major_version
    }

    pub fn minor_version(&self) -> u32 {
        self.minor_version
    }

    pub fn build_number(&self) -> u32 {
        self.build_number
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.len() <= 1
    }

    pub fn get_item_type(&self, id: usize) -> &ItemType {
        self.items.get(id).unwrap_or(&self.items[0])
    }

    pub fn get_item_id_by_client_id(&self, client_id: u16) -> &ItemType {
        if client_id >= 100 {
            if let Some(server_id) = self.client_id_to_server_id.get_server_id(client_id) {
                return self.get_item_type(usize::from(server_id));
            }
        }

        &self.items[0]
    }

    pub fn get_item_id_by_name(&self, name: &str) -> Option<u16> {
        self.name_to_item_id.get(&lowercase_string(name)).copied()
    }

    pub fn get_currency_items(&self) -> &BTreeMap<std::cmp::Reverse<u64>, u16> {
        &self.currency_items
    }

    fn load_with_loader(&mut self, loader: &mut Loader) -> Result<(), ItemsError> {
        self.clear();

        let root = loader.parse_tree()?.clone();
        if let Some(mut props) = loader.get_props(&root)? {
            let _flags = props.read_u32()?;
            if let Some(attribute) = props.read_optional_u8()? {
                if attribute == ROOT_ATTR_VERSION {
                    let data_len = usize::from(props.read_u16()?);
                    if data_len != VersionInfo::BYTE_SIZE {
                        return Err(ItemsError::InvalidRootVersionLength { actual: data_len });
                    }

                    let version = VersionInfo::read_from(&mut props)?;
                    self.major_version = version.major_version;
                    self.minor_version = version.minor_version;
                    self.build_number = version.build_number;
                }
            }
        }

        if self.major_version == u32::MAX {
            warn!("items.otb using generic client version");
        } else if self.major_version != 3 {
            return Err(ItemsError::UnsupportedMajorVersion(self.major_version));
        } else if self.minor_version < CLIENT_VERSION_860_OLD {
            return Err(ItemsError::UnsupportedMinorVersion(self.minor_version));
        }

        for item_node in &root.children {
            let mut stream = loader
                .get_props(item_node)?
                .ok_or(ItemsError::MissingItemProperties)?;
            let flags = stream.read_u32()?;

            let mut server_id = 0u16;
            let mut client_id = 0u16;
            let mut speed = 0u16;
            let mut ware_id = 0u16;
            let mut light_level = 0u8;
            let mut light_color = 0u8;
            let mut always_on_top_order = 0u8;

            while let Some(attribute) = stream.read_optional_u8()? {
                let data_len = usize::from(stream.read_u16()?);

                match attribute {
                    ITEM_ATTR_SERVER_ID => {
                        ensure_length(attribute, data_len, 2)?;
                        server_id = stream.read_u16()?;
                    }
                    ITEM_ATTR_CLIENT_ID => {
                        ensure_length(attribute, data_len, 2)?;
                        client_id = stream.read_u16()?;
                    }
                    ITEM_ATTR_SPEED => {
                        ensure_length(attribute, data_len, 2)?;
                        speed = stream.read_u16()?;
                    }
                    ITEM_ATTR_LIGHT2 => {
                        ensure_length(attribute, data_len, LightBlock2::BYTE_SIZE)?;
                        let light = LightBlock2::read_from(&mut stream)?;
                        light_level = light.light_level as u8;
                        light_color = light.light_color as u8;
                    }
                    ITEM_ATTR_TOP_ORDER => {
                        ensure_length(attribute, data_len, 1)?;
                        always_on_top_order = stream.read_u8()?;
                    }
                    ITEM_ATTR_WARE_ID => {
                        ensure_length(attribute, data_len, 2)?;
                        ware_id = stream.read_u16()?;
                    }
                    _ => stream.skip(data_len)?,
                }
            }

            self.client_id_to_server_id.emplace(client_id, server_id);

            let required_len = usize::from(server_id) + 1;
            if self.items.len() < required_len {
                self.items.resize_with(required_len, ItemType::default);
            }

            let group = ItemGroup::try_from(item_node.node_type)?;
            let item = &mut self.items[usize::from(server_id)];
            item.group = group;
            item.kind = group.default_kind();
            item.id = server_id;
            item.client_id = client_id;
            item.speed = speed;
            item.light_level = light_level;
            item.light_color = light_color;
            item.ware_id = ware_id;
            item.always_on_top_order = always_on_top_order;
            item.block_solid = has_bit_set(FLAG_BLOCK_SOLID, flags);
            item.block_projectile = has_bit_set(FLAG_BLOCK_PROJECTILE, flags);
            item.block_path_find = has_bit_set(FLAG_BLOCK_PATH_FIND, flags);
            item.has_height = has_bit_set(FLAG_HAS_HEIGHT, flags);
            item.useable = has_bit_set(FLAG_USEABLE, flags);
            item.pickupable = has_bit_set(FLAG_PICKUPABLE, flags);
            item.moveable = has_bit_set(FLAG_MOVEABLE, flags);
            item.stackable = has_bit_set(FLAG_STACKABLE, flags);
            item.always_on_top = has_bit_set(FLAG_ALWAYS_ON_TOP, flags);
            item.is_vertical = has_bit_set(FLAG_VERTICAL, flags);
            item.is_horizontal = has_bit_set(FLAG_HORIZONTAL, flags);
            item.is_hangable = has_bit_set(FLAG_HANGABLE, flags);
            item.allow_dist_read = has_bit_set(FLAG_ALLOW_DIST_READ, flags);
            item.rotatable = has_bit_set(FLAG_ROTATABLE, flags);
            item.can_read_text = has_bit_set(FLAG_READABLE, flags);
            item.look_through = has_bit_set(FLAG_LOOK_THROUGH, flags);
            item.force_use = has_bit_set(FLAG_FORCE_USE, flags);
        }

        self.items.shrink_to_fit();
        Ok(())
    }

    fn apply_json5(&mut self, data: ItemsJson5) -> Result<(), ItemsError> {
        for item in data.items {
            if let Some(id) = item.id {
                self.apply_json5_item(&item, id)?;
                continue;
            }

            let Some(from_id) = item.fromid else {
                return Err(ItemsError::MissingItemIdentifier);
            };
            let Some(to_id) = item.toid else {
                return Err(ItemsError::MissingToId { from_id });
            };

            for id in from_id..=to_id {
                self.apply_json5_item(&item, id)?;
            }
        }

        Ok(())
    }

    fn apply_json5_item(&mut self, item: &ItemJson5, id: u16) -> Result<(), ItemsError> {
        if id > 0 && id < 100 {
            let item_type = self.get_item_type_mut_or_default(id);
            item_type.id = id;
        }

        if self.get_item_type_mut_or_default(id).id == 0 {
            return Ok(());
        }

        if !self.get_item_type_mut_or_default(id).name.is_empty() {
            warn!("duplicate item definition for id {id}");
            return Ok(());
        }

        let lowercase_name = item.name.as_deref().map(lowercase_string);
        let mut attributes = item.attributes.clone().unwrap_or_default();
        if let Some(attribute) = item.attribute.clone() {
            attributes.insert(0, attribute);
        }
        {
            let item_type = self.get_item_type_mut_or_default(id);
            item_type.name = item.name.clone().unwrap_or_default();
            item_type.article = item.article.clone().unwrap_or_default();
            item_type.plural_name = item.plural.clone().unwrap_or_default();

            for attribute in &attributes {
                apply_item_attribute(item_type, attribute)?;
            }
            item_type.attributes =
                attributes.iter().cloned().map(ItemAttribute::from).collect();
        }

        for attribute in &attributes {
            match lowercase_string(&attribute.key).as_str() {
                "maletransformto" | "malesleeper" => {
                    let value = parse_u16(&attribute.value)?;
                    self.get_item_type_mut_or_default(id).transform_to_on_use[1] = value;
                    let other = self.get_item_type_mut_or_default(value);
                    if other.transform_to_free == 0 {
                        other.transform_to_free = id;
                    }
                    let it = self.get_item_type_mut_or_default(id);
                    if it.transform_to_on_use[0] == 0 {
                        it.transform_to_on_use[0] = value;
                    }
                }
                "femaletransformto" | "femalesleeper" => {
                    let value = parse_u16(&attribute.value)?;
                    self.get_item_type_mut_or_default(id).transform_to_on_use[0] = value;
                    let other = self.get_item_type_mut_or_default(value);
                    if other.transform_to_free == 0 {
                        other.transform_to_free = id;
                    }
                    let it = self.get_item_type_mut_or_default(id);
                    if it.transform_to_on_use[1] == 0 {
                        it.transform_to_on_use[1] = value;
                    }
                }
                _ => {}
            }
        }

        if let Some(lowercase_name) = lowercase_name {
            self.name_to_item_id.entry(lowercase_name).or_insert(id);
        }

        let worth = self.get_item_type(usize::from(id)).worth;
        if worth > 0 {
            self.currency_items.insert(std::cmp::Reverse(worth), id);
        }

        Ok(())
    }

    fn get_item_type_mut_or_default(&mut self, id: u16) -> &mut ItemType {
        if usize::from(id) < self.items.len() {
            return &mut self.items[usize::from(id)];
        }

        &mut self.items[0]
    }
}

impl ItemGroup {
    fn default_kind(self) -> ItemKind {
        match self {
            Self::Container => ItemKind::Container,
            Self::Door => ItemKind::Door,
            Self::MagicField => ItemKind::MagicField,
            Self::Teleport => ItemKind::Teleport,
            _ => ItemKind::None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ItemsError {
    #[error(transparent)]
    Otb(#[from] OtbError),
    #[error(transparent)]
    Json5(#[from] Json5LoadError),
    #[error("invalid root version block length: expected 140 bytes, got {actual}")]
    InvalidRootVersionLength { actual: usize },
    #[error("unsupported items.otb major version: {0}")]
    UnsupportedMajorVersion(u32),
    #[error("unsupported items.otb minor version: {0}")]
    UnsupportedMinorVersion(u32),
    #[error("item attribute 0x{attribute:02X} expected {expected} bytes, got {actual}")]
    InvalidAttributeLength {
        attribute: u8,
        expected: usize,
        actual: usize,
    },
    #[error("unknown OTB item group: {0}")]
    UnknownGroup(u8),
    #[error("item node is missing properties")]
    MissingItemProperties,
    #[error("item entry is missing both `id` and `fromid`")]
    MissingItemIdentifier,
    #[error("item range starting at {from_id} is missing `toid`")]
    MissingToId { from_id: u16 },
    #[error("invalid item attribute value: {value}")]
    InvalidAttributeValue { value: Value },
    #[error("unknown floorchange value: {0}")]
    UnknownFloorChange(String),
    #[error("unsigned item attribute out of range: {value}")]
    AttributeOutOfRange { value: u64 },
    #[error("signed item attribute out of range: {value}")]
    SignedAttributeOutOfRange { value: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ClientIdToServerIdMap {
    values: Vec<u16>,
}

impl ClientIdToServerIdMap {
    fn emplace(&mut self, client_id: u16, server_id: u16) {
        let client_id = usize::from(client_id);
        if client_id >= self.values.len() {
            self.values.resize(client_id + 1, 0);
        }
        if self.values[client_id] == 0 {
            self.values[client_id] = server_id;
        }
    }

    fn get_server_id(&self, client_id: u16) -> Option<u16> {
        self.values
            .get(usize::from(client_id))
            .copied()
            .filter(|id| *id != 0)
    }

    fn clear(&mut self) {
        self.values.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VersionInfo {
    major_version: u32,
    minor_version: u32,
    build_number: u32,
    csd_version: [u8; 128],
}

impl VersionInfo {
    const BYTE_SIZE: usize = 140;

    fn read_from(stream: &mut PropStream) -> Result<Self, ItemsError> {
        Ok(Self {
            major_version: stream.read_u32()?,
            minor_version: stream.read_u32()?,
            build_number: stream.read_u32()?,
            csd_version: stream.read_fixed_bytes::<128>()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LightBlock2 {
    light_level: u16,
    light_color: u16,
}

impl LightBlock2 {
    const BYTE_SIZE: usize = 4;

    fn read_from(stream: &mut PropStream) -> Result<Self, ItemsError> {
        Ok(Self {
            light_level: stream.read_u16()?,
            light_color: stream.read_u16()?,
        })
    }
}

fn ensure_length(attribute: u8, actual: usize, expected: usize) -> Result<(), ItemsError> {
    if actual != expected {
        return Err(ItemsError::InvalidAttributeLength {
            attribute,
            expected,
            actual,
        });
    }

    Ok(())
}

fn has_bit_set(flag: u32, flags: u32) -> bool {
    flags & flag != 0
}

fn apply_item_attribute(
    item_type: &mut ItemType,
    attribute: &ItemAttributeJson5,
) -> Result<(), ItemsError> {
    let key = lowercase_string(&attribute.key);

    match key.as_str() {
        "type" => {
            let value = parse_string(&attribute.value)?;
            apply_item_kind(item_type, &value);
        }
        "description" => item_type.description = parse_string(&attribute.value)?,
        "weight" => item_type.weight = parse_u32(&attribute.value)?,
        "showcount" => item_type.show_count = parse_bool(&attribute.value)?,
        "armor" => item_type.armor = parse_i32(&attribute.value)?,
        "defense" => item_type.defense = parse_i32(&attribute.value)?,
        "extradef" => item_type.extra_defense = parse_i32(&attribute.value)?,
        "attack" => item_type.attack = parse_i32(&attribute.value)?,
        "attackspeed" => {
            item_type.attack_speed = parse_u32(&attribute.value)?;
            if item_type.attack_speed > 0 && item_type.attack_speed < 100 {
                item_type.attack_speed = 100;
            }
        }
        "rotateto" => item_type.rotate_to = parse_u16(&attribute.value)?,
        "moveable" | "movable" => item_type.moveable = parse_bool(&attribute.value)?,
        "blockprojectile" => item_type.block_projectile = parse_bool(&attribute.value)?,
        "ignoreblocking" => item_type.ignore_blocking = parse_bool(&attribute.value)?,
        "allowpickupable" | "pickupable" => item_type.pickupable = parse_bool(&attribute.value)?,
        "forceserialize" | "forcesave" => item_type.force_serialize = parse_bool(&attribute.value)?,
        "floorchange" => apply_floor_change(item_type, &attribute.value)?,
        "containersize" => item_type.max_items = parse_u16(&attribute.value)?,
        "readable" => item_type.can_read_text = parse_bool(&attribute.value)?,
        "writeable" => {
            item_type.can_write_text = parse_bool(&attribute.value)?;
            item_type.can_read_text = item_type.can_write_text;
        }
        "maxtextlen" => item_type.max_text_len = parse_u16(&attribute.value)?,
        "writeonceitemid" => item_type.write_once_item_id = parse_u16(&attribute.value)?,
        "transformequipto" => item_type.transform_equip_to = parse_u16(&attribute.value)?,
        "transformdeequipto" => item_type.transform_de_equip_to = parse_u16(&attribute.value)?,
        "duration" => item_type.decay_time = parse_u32(&attribute.value)?,
        "showduration" => item_type.show_duration = parse_bool(&attribute.value)?,
        "charges" => item_type.charges = parse_u32(&attribute.value)?,
        "showcharges" => item_type.show_charges = parse_bool(&attribute.value)?,
        "showattributes" => item_type.show_attributes = parse_bool(&attribute.value)?,
        "hitchance" => item_type.hit_chance = clamp_i8(parse_i32(&attribute.value)?),
        "maxhitchance" => {
            item_type.max_hit_chance = clamp_i32(parse_i32(&attribute.value)?, 0, 100)
        }
        "replaceable" => item_type.replaceable = parse_bool(&attribute.value)?,
        "leveldoor" => item_type.level_door = parse_u32(&attribute.value)?,
        "transformto" => item_type.transform_to_free = parse_u16(&attribute.value)?,
        "partnerdirection" => {
            item_type.bed_partner_dir = parse_direction(&parse_string(&attribute.value)?)
        }
        "destroyto" => item_type.destroy_to = parse_u16(&attribute.value)?,
        "walkstack" => item_type.walk_stack = parse_bool(&attribute.value)?,
        "blocking" => item_type.block_solid = parse_bool(&attribute.value)?,
        "allowdistread" => item_type.allow_dist_read = parse_bool(&attribute.value)?,
        "storeitem" => item_type.store_item = parse_bool(&attribute.value)?,
        "worth" => item_type.worth = parse_u64(&attribute.value)?,
        "stopduration" => item_type.stop_time = parse_bool(&attribute.value)?,
        "nofieldblockpath" => item_type.no_field_block_path = parse_bool(&attribute.value)?,
        "supporthangable" => item_type.supports_hangable = parse_bool(&attribute.value)?,
        "runespellname" => item_type.rune_spell_name = parse_string(&attribute.value)?,
        "runemagicallevel" | "runemagLevel" => {
            item_type.rune_mag_level = parse_i32(&attribute.value)?;
        }
        "runelevel" => item_type.rune_level = parse_i32(&attribute.value)?,
        "weapontype" => apply_weapon_type(item_type, &attribute.value)?,
        "slottype" => apply_slot_type(item_type, &attribute.value)?,
        "ammotype" => apply_ammo_type(item_type, &attribute.value)?,
        "corpsetype" => apply_corpse_type(item_type, &attribute.value)?,
        "fluidsource" => apply_fluid_source(item_type, &attribute.value)?,
        "shootrange" => item_type.shoot_range = parse_u32(&attribute.value)? as u8,
        "elementice" => {
            item_type.element_damage = parse_u16(&attribute.value)?;
            item_type.element_type = COMBAT_ICEDAMAGE;
        }
        "elementearth" => {
            item_type.element_damage = parse_u16(&attribute.value)?;
            item_type.element_type = COMBAT_EARTHDAMAGE;
        }
        "elementfire" => {
            item_type.element_damage = parse_u16(&attribute.value)?;
            item_type.element_type = COMBAT_FIREDAMAGE;
        }
        "elementenergy" => {
            item_type.element_damage = parse_u16(&attribute.value)?;
            item_type.element_type = COMBAT_ENERGYDAMAGE;
        }
        "elementdeath" => {
            item_type.element_damage = parse_u16(&attribute.value)?;
            item_type.element_type = COMBAT_DEATHDAMAGE;
        }
        "elementholy" => {
            item_type.element_damage = parse_u16(&attribute.value)?;
            item_type.element_type = COMBAT_HOLYDAMAGE;
        }
        "wieldinfo" => item_type.wield_info = parse_u32(&attribute.value)?,
        "vocationstring" => item_type.vocation_string = parse_string(&attribute.value)?,
        "minlevel" | "minimumlevel" => item_type.min_req_level = parse_u32(&attribute.value)?,
        "minmagiclevel" | "minimummagiclevel" => item_type.min_req_magic_level = parse_u32(&attribute.value)?,
        _ => {}
    }

    Ok(())
}

fn parse_direction(value: &str) -> u8 {
    match value.to_ascii_lowercase().as_str() {
        "north" | "n" | "0" => 0,
        "east" | "e" | "1" => 1,
        "south" | "s" | "2" => 2,
        "west" | "w" | "3" => 3,
        "southwest" | "south west" | "south-west" | "sw" | "4" => 4,
        "southeast" | "south east" | "south-east" | "se" | "5" => 5,
        "northwest" | "north west" | "north-west" | "nw" | "6" => 6,
        "northeast" | "north east" | "north-east" | "ne" | "7" => 7,
        _ => 0,
    }
}

fn apply_item_kind(item_type: &mut ItemType, value: &str) {
    match lowercase_string(value).as_str() {
        "key" => item_type.kind = ItemKind::Key,
        "magicfield" => item_type.kind = ItemKind::MagicField,
        "container" => {
            item_type.kind = ItemKind::Container;
            item_type.group = ItemGroup::Container;
        }
        "depot" => item_type.kind = ItemKind::Depot,
        "mailbox" => item_type.kind = ItemKind::Mailbox,
        "trashholder" => item_type.kind = ItemKind::TrashHolder,
        "teleport" => item_type.kind = ItemKind::Teleport,
        "door" => item_type.kind = ItemKind::Door,
        "bed" => item_type.kind = ItemKind::Bed,
        "rune" => item_type.kind = ItemKind::Rune,
        _ => {}
    }
}

fn apply_floor_change(item_type: &mut ItemType, value: &Value) -> Result<(), ItemsError> {
    let floor_change = match lowercase_string(&parse_string(value)?).as_str() {
        "down" => TILESTATE_FLOORCHANGE_DOWN,
        "north" => TILESTATE_FLOORCHANGE_NORTH,
        "south" => TILESTATE_FLOORCHANGE_SOUTH,
        "east" => TILESTATE_FLOORCHANGE_EAST,
        "west" => TILESTATE_FLOORCHANGE_WEST,
        "southalt" => TILESTATE_FLOORCHANGE_SOUTH_ALT,
        "eastalt" => TILESTATE_FLOORCHANGE_EAST_ALT,
        other => {
            return Err(ItemsError::UnknownFloorChange(other.to_string()));
        }
    };

    item_type.floor_change |= floor_change;
    Ok(())
}

fn parse_bool(value: &Value) -> Result<bool, ItemsError> {
    Ok(match value {
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_i64().unwrap_or_default() != 0,
        Value::String(value) => boolean_string(value),
        other => {
            return Err(ItemsError::InvalidAttributeValue {
                value: other.clone(),
            })
        }
    })
}

fn parse_string(value: &Value) -> Result<String, ItemsError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        other => Err(ItemsError::InvalidAttributeValue {
            value: other.clone(),
        }),
    }
}

fn parse_u16(value: &Value) -> Result<u16, ItemsError> {
    let value = parse_u64(value)?;
    u16::try_from(value).map_err(|_| ItemsError::AttributeOutOfRange { value })
}

fn parse_u32(value: &Value) -> Result<u32, ItemsError> {
    let value = parse_u64(value)?;
    u32::try_from(value).map_err(|_| ItemsError::AttributeOutOfRange { value })
}

fn parse_u64(value: &Value) -> Result<u64, ItemsError> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| ItemsError::InvalidAttributeValue {
                value: value.clone(),
            }),
        Value::String(value) => {
            value
                .parse::<u64>()
                .map_err(|_| ItemsError::InvalidAttributeValue {
                    value: Value::String(value.clone()),
                })
        }
        other => Err(ItemsError::InvalidAttributeValue {
            value: other.clone(),
        }),
    }
}

fn parse_i32(value: &Value) -> Result<i32, ItemsError> {
    match value {
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                i32::try_from(value).map_err(|_| ItemsError::SignedAttributeOutOfRange { value })
            } else if let Some(value) = number.as_u64() {
                i32::try_from(value).map_err(|_| ItemsError::AttributeOutOfRange { value })
            } else {
                Err(ItemsError::InvalidAttributeValue {
                    value: value.clone(),
                })
            }
        }
        Value::String(value) => {
            let parsed = value
                .parse::<i64>()
                .map_err(|_| ItemsError::InvalidAttributeValue {
                    value: Value::String(value.clone()),
                })?;
            i32::try_from(parsed)
                .map_err(|_| ItemsError::SignedAttributeOutOfRange { value: parsed })
        }
        other => Err(ItemsError::InvalidAttributeValue {
            value: other.clone(),
        }),
    }
}

fn clamp_i8(value: i32) -> i8 {
    value.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8
}

fn clamp_i32(value: i32, min: i32, max: i32) -> i32 {
    value.clamp(min, max)
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

fn lowercase_string(value: &str) -> String {
    value.to_ascii_lowercase()
}


impl From<ItemAttributeJson5> for ItemAttribute {
    fn from(value: ItemAttributeJson5) -> Self {
        Self {
            key: value.key,
            value: value.value,
            attributes: value
                .attributes
                .unwrap_or_default()
                .into_iter()
                .map(ItemAttribute::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ItemsJson5 {
    #[serde(default)]
    items: Vec<ItemJson5>,
}

#[derive(Debug, Clone, Deserialize)]
struct ItemJson5 {
    id: Option<u16>,
    fromid: Option<u16>,
    toid: Option<u16>,
    name: Option<String>,
    article: Option<String>,
    plural: Option<String>,
    #[serde(default)]
    attributes: Option<Vec<ItemAttributeJson5>>,
    #[serde(default)]
    attribute: Option<ItemAttributeJson5>,
}

#[derive(Debug, Clone, Deserialize)]
struct ItemAttributeJson5 {
    key: String,
    value: Value,
    #[serde(default)]
    attributes: Option<Vec<ItemAttributeJson5>>,
}

pub const COMBAT_NONE: u8 = 0;
pub const COMBAT_PHYSICALDAMAGE: u8 = 1;
pub const COMBAT_ENERGYDAMAGE: u8 = 2;
pub const COMBAT_EARTHDAMAGE: u8 = 3;
pub const COMBAT_FIREDAMAGE: u8 = 4;
pub const COMBAT_UNDEFINEDDAMAGE: u8 = 5;
pub const COMBAT_LIFEDRAIN: u8 = 6;
pub const COMBAT_MANADRAIN: u8 = 7;
pub const COMBAT_HEALING: u8 = 8;
pub const COMBAT_DROWNDAMAGE: u8 = 9;
pub const COMBAT_ICEDAMAGE: u8 = 10;
pub const COMBAT_HOLYDAMAGE: u8 = 11;
pub const COMBAT_DEATHDAMAGE: u8 = 12;

pub const SLOTP_HEAD: u16 = 1 << 0;
pub const SLOTP_NECKLACE: u16 = 1 << 1;
pub const SLOTP_BACKPACK: u16 = 1 << 2;
pub const SLOTP_ARMOR: u16 = 1 << 3;
pub const SLOTP_RIGHT: u16 = 1 << 4;
pub const SLOTP_LEFT: u16 = 1 << 5;
pub const SLOTP_LEGS: u16 = 1 << 6;
pub const SLOTP_FEET: u16 = 1 << 7;
pub const SLOTP_RING: u16 = 1 << 8;
pub const SLOTP_AMMO: u16 = 1 << 9;
pub const SLOTP_TWO_HAND: u16 = 1 << 11;
pub const SLOTP_HAND: u16 = SLOTP_LEFT | SLOTP_RIGHT;

fn apply_weapon_type(item_type: &mut ItemType, value: &Value) -> Result<(), ItemsError> {
    let v = lowercase_string(&parse_string(value)?);
    item_type.weapon_type = match v.as_str() {
        "sword" => 1,
        "club" => 2,
        "axe" => 3,
        "shield" => 4,
        "distance" => 5,
        "wand" => 6,
        "ammunition" => 7,
        _ => {
            warn!("unknown weaponType: {v}");
            0
        }
    };
    Ok(())
}

fn apply_slot_type(item_type: &mut ItemType, value: &Value) -> Result<(), ItemsError> {
    let v = lowercase_string(&parse_string(value)?);
    match v.as_str() {
        "head" => item_type.slot_position |= SLOTP_HEAD,
        "body" => item_type.slot_position |= SLOTP_ARMOR,
        "legs" => item_type.slot_position |= SLOTP_LEGS,
        "feet" => item_type.slot_position |= SLOTP_FEET,
        "backpack" => item_type.slot_position |= SLOTP_BACKPACK,
        "two-handed" => item_type.slot_position |= SLOTP_TWO_HAND,
        "right-hand" => item_type.slot_position &= !SLOTP_LEFT,
        "left-hand" => item_type.slot_position &= !SLOTP_RIGHT,
        "necklace" => item_type.slot_position |= SLOTP_NECKLACE,
        "ring" => item_type.slot_position |= SLOTP_RING,
        "ammo" => item_type.slot_position |= SLOTP_AMMO,
        "hand" => item_type.slot_position |= SLOTP_HAND,
        _ => warn!("unknown slotType: {v}"),
    }
    Ok(())
}

fn apply_ammo_type(item_type: &mut ItemType, value: &Value) -> Result<(), ItemsError> {
    let v = lowercase_string(&parse_string(value)?);
    item_type.ammo_type = match v.as_str() {
        "spear" => 3,
        "bolt" => 1,
        "arrow" | "poisonarrow" | "burstarrow" => 2,
        "throwingstar" => 4,
        "throwingknife" => 5,
        "smallstone" | "largerock" => 6,
        "snowball" => 7,
        _ => {
            warn!("unknown ammoType: {v}");
            0
        }
    };
    Ok(())
}

fn apply_corpse_type(item_type: &mut ItemType, value: &Value) -> Result<(), ItemsError> {
    let v = lowercase_string(&parse_string(value)?);
    item_type.corpse_type = match v.as_str() {
        "venom" => 1,
        "blood" => 2,
        "undead" => 3,
        "fire" => 4,
        "energy" => 5,
        _ => {
            warn!("unknown corpseType: {v}");
            0
        }
    };
    Ok(())
}

fn apply_fluid_source(item_type: &mut ItemType, value: &Value) -> Result<(), ItemsError> {
    let v = lowercase_string(&parse_string(value)?);
    item_type.fluid_source = match v.as_str() {
        "water" => 1,
        "blood" => 2,
        "beer" => 3,
        "slime" => 4,
        "lemonade" => 5,
        "milk" => 6,
        "mana" => 7,
        "life" => 10,
        "oil" => 11,
        "urine" => 13,
        "coconut" => 14,
        "wine" => 15,
        "mud" => 19,
        "fruitjuice" => 21,
        "lava" => 26,
        "rum" => 27,
        "swamp" => 28,
        "tea" => 35,
        "mead" => 43,
        _ => {
            warn!("unknown fluidSource: {v}");
            0
        }
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{ItemGroup, ItemKind, Items};
    use crate::io::otb::Node;

    #[test]
    fn load_from_otb_bytes_should_populate_versions_and_item_flags() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OTBI");
        bytes.push(Node::START);
        bytes.push(0x00);

        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0x01);
        bytes.extend_from_slice(&(140u16).to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&19u32.to_le_bytes());
        bytes.extend_from_slice(&42u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 128]);

        bytes.push(Node::START);
        bytes.push(ItemGroup::Container as u8);
        bytes.extend_from_slice(&(1u32 | (1u32 << 7) | (1u32 << 13)).to_le_bytes());
        bytes.push(0x10);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.push(0x11);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&200u16.to_le_bytes());
        bytes.push(0x14);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&250u16.to_le_bytes());
        bytes.push(0x23);
        bytes.extend_from_slice(&(4u16).to_le_bytes());
        bytes.extend_from_slice(&7u16.to_le_bytes());
        bytes.extend_from_slice(&215u16.to_le_bytes());
        bytes.push(0x24);
        bytes.extend_from_slice(&(1u16).to_le_bytes());
        bytes.push(3);
        bytes.push(0x26);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&999u16.to_le_bytes());
        bytes.push(Node::END);
        bytes.push(Node::END);

        let mut items = Items::new();
        items
            .load_from_otb_bytes(bytes)
            .expect("synthetic items.otb should load");

        let item = items.get_item_type(100);
        assert_eq!(items.major_version(), 3);
        assert_eq!(items.minor_version(), 19);
        assert_eq!(items.build_number(), 42);
        assert_eq!(item.group, ItemGroup::Container);
        assert_eq!(item.kind, ItemKind::Container);
        assert_eq!(item.client_id, 200);
        assert_eq!(item.speed, 250);
        assert_eq!(item.light_level, 7);
        assert_eq!(item.light_color, 215);
        assert_eq!(item.always_on_top_order, 3);
        assert_eq!(item.ware_id, 999);
        assert!(item.block_solid);
        assert!(item.stackable);
        assert!(item.always_on_top);
        assert_eq!(items.get_item_id_by_client_id(200).id, 100);
    }

    #[test]
    fn load_from_otb_bytes_should_reject_older_minor_versions() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OTBI");
        bytes.push(Node::START);
        bytes.push(0x00);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0x01);
        bytes.extend_from_slice(&(140u16).to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&18u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 128]);
        bytes.push(Node::END);

        let mut items = Items::new();
        let error = items
            .load_from_otb_bytes(bytes)
            .expect_err("older client versions should fail");

        assert_eq!(error.to_string(), "unsupported items.otb minor version: 18");
    }

    #[test]
    fn load_from_json5_should_overlay_item_metadata_and_attributes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OTBI");
        bytes.push(Node::START);
        bytes.push(0x00);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0x01);
        bytes.extend_from_slice(&(140u16).to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&19u32.to_le_bytes());
        bytes.extend_from_slice(&42u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 128]);
        bytes.push(Node::START);
        bytes.push(ItemGroup::Container as u8);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0x10);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.push(0x11);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&200u16.to_le_bytes());
        bytes.push(Node::END);
        bytes.push(Node::END);

        let mut items = Items::new();
        items
            .load_from_otb_bytes(bytes)
            .expect("synthetic items.otb should load");

        let path = std::env::temp_dir().join("tfs-rust-items.json5");
        fs::write(
            &path,
            r#"
{
  items: [
    {
      id: 100,
      name: "Torch",
      article: "a",
      plural: "torches",
      attributes: [
        { key: "description", value: "Bright." },
        { key: "weight", value: 2300 },
        { key: "moveable", value: false },
        { key: "pickupable", value: true },
        { key: "armor", value: 1 },
        { key: "charges", value: 7 },
        { key: "worth", value: 50 },
        { key: "allowdistread", value: "yes" },
      ],
    },
  ],
}
"#,
        )
        .expect("temp items json5 should be writable");

        items
            .load_from_json5(&path)
            .expect("json5 overlay should load");

        let item = items.get_item_type(100);
        assert_eq!(item.name, "Torch");
        assert_eq!(item.article, "a");
        assert_eq!(item.plural_name, "torches");
        assert_eq!(item.description, "Bright.");
        assert_eq!(item.weight, 2300);
        assert!(!item.moveable);
        assert!(item.pickupable);
        assert_eq!(item.armor, 1);
        assert_eq!(item.charges, 7);
        assert_eq!(item.worth, 50);
        assert!(item.allow_dist_read);
        assert_eq!(items.get_item_id_by_name("torch"), Some(100));
        assert_eq!(item.attributes.len(), 8);

        fs::remove_file(path).expect("temp items json5 should be removable");
    }
}
