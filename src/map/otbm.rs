use std::ffi::OsString;
use std::path::Path;

use thiserror::Error;
use tracing::warn;

use crate::io::otb::{Identifier, Loader, Node, OtbError, PropStream};
use crate::items::{ItemKind, Items};

use super::tile::{
    MapItem, Tile, TileKind, TILESTATE_NOLOGOUT, TILESTATE_NOPVPZONE, TILESTATE_PROTECTIONZONE,
    TILESTATE_PVPZONE,
};
use super::{Map, Position, Town};

const OTBM_IDENTIFIER: Identifier = *b"OTBM";
const OTBM_MAP_DATA: u8 = 2;
const OTBM_TILE_AREA: u8 = 4;
const OTBM_TILE: u8 = 5;
const OTBM_ITEM: u8 = 6;
const OTBM_TOWNS: u8 = 12;
const OTBM_TOWN: u8 = 13;
const OTBM_HOUSETILE: u8 = 14;
const OTBM_WAYPOINTS: u8 = 15;
const OTBM_WAYPOINT: u8 = 16;

const OTBM_ATTR_DESCRIPTION: u8 = 1;
const OTBM_ATTR_TILE_FLAGS: u8 = 3;
const OTBM_ATTR_ITEM: u8 = 9;
const OTBM_ATTR_EXT_SPAWN_FILE: u8 = 11;
const OTBM_ATTR_EXT_HOUSE_FILE: u8 = 13;

const ATTR_ACTION_ID: u8 = 4;
const ATTR_UNIQUE_ID: u8 = 5;
const ATTR_TEXT: u8 = 6;
const ATTR_DESC: u8 = 7;
const ATTR_TELE_DEST: u8 = 8;
const ATTR_DEPOT_ID: u8 = 10;
const ATTR_RUNE_CHARGES: u8 = 12;
const ATTR_HOUSEDOORID: u8 = 14;
const ATTR_COUNT: u8 = 15;
const ATTR_DURATION: u8 = 16;
const ATTR_DECAYING_STATE: u8 = 17;
const ATTR_WRITTENDATE: u8 = 18;
const ATTR_WRITTENBY: u8 = 19;
const ATTR_SLEEPERGUID: u8 = 20;
const ATTR_SLEEPSTART: u8 = 21;
const ATTR_CHARGES: u8 = 22;
const ATTR_NAME: u8 = 24;
const ATTR_ARTICLE: u8 = 25;
const ATTR_PLURALNAME: u8 = 26;
const ATTR_WEIGHT: u8 = 27;
const ATTR_ATTACK: u8 = 28;
const ATTR_DEFENSE: u8 = 29;
const ATTR_EXTRADEFENSE: u8 = 30;
const ATTR_ARMOR: u8 = 31;
const ATTR_HITCHANCE: u8 = 32;
const ATTR_SHOOTRANGE: u8 = 33;
const ATTR_DECAYTO: u8 = 35;

const OTBM_TILEFLAG_PROTECTIONZONE: u32 = 1 << 0;
const OTBM_TILEFLAG_NOPVPZONE: u32 = 1 << 2;
const OTBM_TILEFLAG_NOLOGOUT: u32 = 1 << 3;
const OTBM_TILEFLAG_PVPZONE: u32 = 1 << 4;

const CLIENT_VERSION_810: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RootHeader {
    version: u32,
    width: u16,
    height: u16,
    major_version_items: u32,
    minor_version_items: u32,
}

impl RootHeader {
    fn read_from(stream: &mut PropStream) -> Result<Self, OtbmError> {
        Ok(Self {
            version: stream.read_u32()?,
            width: stream.read_u16()?,
            height: stream.read_u16()?,
            major_version_items: stream.read_u32()?,
            minor_version_items: stream.read_u32()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DestinationCoords {
    x: u16,
    y: u16,
    z: u8,
}

impl DestinationCoords {
    fn read_from(stream: &mut PropStream) -> Result<Self, OtbmError> {
        Ok(Self {
            x: stream.read_u16()?,
            y: stream.read_u16()?,
            z: stream.read_u8()?,
        })
    }

    fn into_position(self) -> Position {
        Position {
            x: self.x,
            y: self.y,
            z: self.z,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TileCoords {
    x: u8,
    y: u8,
}

impl TileCoords {
    fn read_from(stream: &mut PropStream) -> Result<Self, OtbmError> {
        Ok(Self {
            x: stream.read_u8()?,
            y: stream.read_u8()?,
        })
    }
}

pub fn load_from_path(path: impl AsRef<Path>, items: &Items) -> Result<Map, OtbmError> {
    let path = path.as_ref();
    let mut loader = Loader::from_path(path, OTBM_IDENTIFIER)?;
    load_with_loader(path, &mut loader, items)
}

fn load_with_loader(path: &Path, loader: &mut Loader, items: &Items) -> Result<Map, OtbmError> {
    let root = loader.parse_tree()?.clone();

    let mut root_props = loader
        .get_props(&root)?
        .ok_or(OtbmError::MissingRootProperties)?;
    let root_header = RootHeader::read_from(&mut root_props)?;

    validate_root_header(root_header, items)?;

    if root.children.len() != 1 || root.children[0].node_type != OTBM_MAP_DATA {
        return Err(OtbmError::MissingMapDataNode);
    }

    let mut map = Map {
        width: root_header.width,
        height: root_header.height,
        ..Map::default()
    };

    let map_node = root.children[0].clone();
    parse_map_data_attributes(loader, &map_node, &mut map, path)?;

    for map_data_node in &map_node.children {
        match map_data_node.node_type {
            OTBM_TILE_AREA => parse_tile_area(loader, map_data_node, &mut map, items)?,
            OTBM_TOWNS => parse_towns(loader, map_data_node, &mut map)?,
            OTBM_WAYPOINTS if root_header.version > 1 => {
                parse_waypoints(loader, map_data_node, &mut map)?
            }
            other => return Err(OtbmError::UnknownMapNode(other)),
        }
    }

    Ok(map)
}

fn validate_root_header(header: RootHeader, items: &Items) -> Result<(), OtbmError> {
    if header.version == 0 {
        return Err(OtbmError::UnsupportedMapVersion(header.version));
    }
    if header.version > 2 {
        return Err(OtbmError::UnsupportedMapVersion(header.version));
    }
    if header.major_version_items < 3 {
        return Err(OtbmError::UnsupportedItemMajorVersion(
            header.major_version_items,
        ));
    }
    if header.major_version_items > items.major_version() {
        return Err(OtbmError::UnsupportedItemMajorVersion(
            header.major_version_items,
        ));
    }
    if header.minor_version_items < CLIENT_VERSION_810 {
        return Err(OtbmError::UnsupportedItemMinorVersion(
            header.minor_version_items,
        ));
    }
    if header.minor_version_items > items.minor_version() {
        warn!("map needs an updated items.otb");
    }
    Ok(())
}

fn parse_map_data_attributes(
    loader: &Loader,
    map_node: &Node,
    map: &mut Map,
    file_name: &Path,
) -> Result<(), OtbmError> {
    let mut stream = loader
        .get_props(map_node)?
        .ok_or(OtbmError::InvalidMapDataAttributes)?;

    while let Some(attribute) = stream.read_optional_u8()? {
        match attribute {
            OTBM_ATTR_DESCRIPTION => {
                map.description = decode_string(stream.read_string()?);
            }
            OTBM_ATTR_EXT_SPAWN_FILE => {
                map.spawn_file = Some(resolve_sidecar_path(
                    file_name,
                    &decode_string(stream.read_string()?),
                ));
            }
            OTBM_ATTR_EXT_HOUSE_FILE => {
                map.house_file = Some(resolve_sidecar_path(
                    file_name,
                    &decode_string(stream.read_string()?),
                ));
            }
            other => return Err(OtbmError::UnknownHeaderNode(other)),
        }
    }

    Ok(())
}

fn parse_tile_area(
    loader: &Loader,
    node: &Node,
    map: &mut Map,
    items: &Items,
) -> Result<(), OtbmError> {
    let mut stream = loader.get_props(node)?.ok_or(OtbmError::InvalidMapNode)?;
    let area = DestinationCoords::read_from(&mut stream)?;

    for tile_node in &node.children {
        if tile_node.node_type != OTBM_TILE && tile_node.node_type != OTBM_HOUSETILE {
            return Err(OtbmError::UnknownTileNode(tile_node.node_type));
        }

        let mut tile_stream = loader
            .get_props(tile_node)?
            .ok_or(OtbmError::CouldNotReadNodeData)?;
        let tile_coords = TileCoords::read_from(&mut tile_stream)?;
        let position = Position {
            x: area.x + u16::from(tile_coords.x),
            y: area.y + u16::from(tile_coords.y),
            z: area.z,
        };

        let house_id = if tile_node.node_type == OTBM_HOUSETILE {
            let house_id = tile_stream.read_u32()?;
            map.houses.add_house(house_id).add_tile(position);
            Some(house_id)
        } else {
            None
        };

        let mut tile: Option<Tile> = house_id.map(|id| {
            let mut tile = Tile::new(position, TileKind::House { house_id: id });
            tile.set_flag(TILESTATE_PROTECTIONZONE);
            tile
        });
        let mut ground_item: Option<MapItem> = None;
        let mut tileflags = 0u32;

        while let Some(attribute) = tile_stream.read_optional_u8()? {
            match attribute {
                OTBM_ATTR_TILE_FLAGS => {
                    tileflags |= map_tile_flags(tile_stream.read_u32()?);
                }
                OTBM_ATTR_ITEM => {
                    let item = parse_direct_item(&mut tile_stream)?;
                    insert_map_item(
                        map,
                        &mut tile,
                        &mut ground_item,
                        item,
                        house_id,
                        position,
                        items,
                    );
                }
                other => {
                    return Err(OtbmError::UnknownTileAttribute {
                        x: position.x,
                        y: position.y,
                        z: position.z,
                        attribute: other,
                    });
                }
            }
        }

        for child in &tile_node.children {
            if child.node_type != OTBM_ITEM {
                return Err(OtbmError::UnknownNodeType {
                    x: position.x,
                    y: position.y,
                    z: position.z,
                    node_type: child.node_type,
                });
            }

            let item = parse_item_node(loader, child)?;
            insert_map_item(
                map,
                &mut tile,
                &mut ground_item,
                item,
                house_id,
                position,
                items,
            );
        }

        let mut tile = tile
            .unwrap_or_else(|| create_tile(position, house_id, ground_item.take(), None, items));
        tile.set_flag(tileflags);
        map.set_tile(position, tile);
    }

    Ok(())
}

fn insert_map_item(
    map: &mut Map,
    tile: &mut Option<Tile>,
    ground_item: &mut Option<MapItem>,
    item: MapItem,
    house_id: Option<u32>,
    position: Position,
    items: &Items,
) {
    let item_type = items.get_item_type(usize::from(item.server_id));
    let item_kind = item_type.kind;
    let door_id = item.house_door_id;
    if house_id.is_some() && item_type.moveable {
        warn!(
            item_id = item.server_id,
            house_id,
            x = position.x,
            y = position.y,
            z = position.z,
            "skipping moveable house item loaded from map"
        );
        return;
    }

    if tile.is_none() && item_type.is_ground_tile() {
        *ground_item = Some(item);
        return;
    }

    let tile_ref = tile.get_or_insert_with(|| {
        create_tile(position, house_id, ground_item.take(), Some(&item), items)
    });
    tile_ref.internal_add_item(item, items);

    if let Some(house_id) = house_id {
        let house = map.houses.add_house(house_id);
        if item_kind == ItemKind::Door {
            house.add_door(door_id, position);
        } else if item_kind == ItemKind::Bed {
            house.add_bed(position);
        }
    }
}

fn parse_towns(loader: &Loader, node: &Node, map: &mut Map) -> Result<(), OtbmError> {
    for town_node in &node.children {
        if town_node.node_type != OTBM_TOWN {
            return Err(OtbmError::UnknownTownNode(town_node.node_type));
        }

        let mut stream = loader
            .get_props(town_node)?
            .ok_or(OtbmError::CouldNotReadTownData)?;
        let town_id = stream.read_u32()?;
        let town_name = decode_string(stream.read_string()?);
        let temple_pos = DestinationCoords::read_from(&mut stream)?.into_position();
        map.towns.insert(
            town_id,
            Town {
                id: town_id,
                name: town_name,
                temple_pos,
            },
        );
    }

    Ok(())
}

fn parse_waypoints(loader: &Loader, node: &Node, map: &mut Map) -> Result<(), OtbmError> {
    for waypoint_node in &node.children {
        if waypoint_node.node_type != OTBM_WAYPOINT {
            return Err(OtbmError::UnknownWaypointNode(waypoint_node.node_type));
        }

        let mut stream = loader
            .get_props(waypoint_node)?
            .ok_or(OtbmError::CouldNotReadWaypointData)?;
        let name = decode_string(stream.read_string()?);
        let position = DestinationCoords::read_from(&mut stream)?.into_position();
        map.waypoints.insert(name, position);
    }

    Ok(())
}

fn parse_direct_item(stream: &mut PropStream) -> Result<MapItem, OtbmError> {
    let server_id = stream.read_u16()?;
    Ok(MapItem {
        server_id,
        count: 1,
        loaded_from_map: true,
        ..MapItem::default()
    })
}

fn parse_item_node(loader: &Loader, node: &Node) -> Result<MapItem, OtbmError> {
    let mut stream = loader.get_props(node)?.ok_or(OtbmError::InvalidItemNode)?;
    let server_id = stream.read_u16()?;
    let mut item = MapItem {
        server_id,
        count: 1,
        loaded_from_map: true,
        ..MapItem::default()
    };

    while let Some(attribute) = stream.read_optional_u8()? {
        if attribute == 0 {
            break;
        }

        match attribute {
            ATTR_COUNT => item.count = u16::from(stream.read_u8()?),
            ATTR_RUNE_CHARGES => item.rune_charges = stream.read_u8()?,
            ATTR_ACTION_ID => item.action_id = stream.read_u16()?,
            ATTR_UNIQUE_ID => item.unique_id = stream.read_u16()?,
            ATTR_TEXT => item.text = decode_string(stream.read_string()?),
            ATTR_DESC => item.description = decode_string(stream.read_string()?),
            ATTR_TELE_DEST => {
                item.teleport_destination =
                    Some(DestinationCoords::read_from(&mut stream)?.into_position())
            }
            ATTR_DEPOT_ID => item.depot_id = stream.read_u16()?,
            ATTR_HOUSEDOORID => item.house_door_id = stream.read_u8()?,
            ATTR_DURATION => item.duration = stream.read_u32()? as i32,
            ATTR_DECAYING_STATE => item.decaying_state = stream.read_u8()?,
            ATTR_WRITTENDATE => item.written_date = stream.read_u32()?,
            ATTR_WRITTENBY => item.written_by = decode_string(stream.read_string()?),
            ATTR_SLEEPERGUID => item.sleeper_guid = stream.read_u32()?,
            ATTR_SLEEPSTART => item.sleep_start = stream.read_u32()?,
            ATTR_CHARGES => item.charges = stream.read_u16()?,
            ATTR_NAME => item.name = decode_string(stream.read_string()?),
            ATTR_ARTICLE => item.article = decode_string(stream.read_string()?),
            ATTR_PLURALNAME => item.plural_name = decode_string(stream.read_string()?),
            ATTR_WEIGHT => item.weight = stream.read_u32()?,
            ATTR_ATTACK => item.attack = stream.read_u32()? as i32,
            ATTR_DEFENSE => item.defense = stream.read_u32()? as i32,
            ATTR_EXTRADEFENSE => item.extra_defense = stream.read_u32()? as i32,
            ATTR_ARMOR => item.armor = stream.read_u32()? as i32,
            ATTR_HITCHANCE => item.hit_chance = stream.read_u8()? as i8,
            ATTR_SHOOTRANGE => item.shoot_range = stream.read_u8()?,
            ATTR_DECAYTO => item.decay_to = stream.read_u32()? as i32,
            other => return Err(OtbmError::UnsupportedItemAttribute(other)),
        }
    }

    for child in &node.children {
        if child.node_type != OTBM_ITEM {
            return Err(OtbmError::UnsupportedChildItemNode(child.node_type));
        }
        item.children.push(parse_item_node(loader, child)?);
    }

    Ok(item)
}

fn create_tile(
    position: Position,
    house_id: Option<u32>,
    ground: Option<MapItem>,
    first_item: Option<&MapItem>,
    items: &Items,
) -> Tile {
    let kind = if let Some(house_id) = house_id {
        TileKind::House { house_id }
    } else if ground.is_none()
        || first_item
            .map(|item| items.get_item_type(usize::from(item.server_id)).block_solid)
            .unwrap_or(false)
        || ground
            .as_ref()
            .map(|item| items.get_item_type(usize::from(item.server_id)).block_solid)
            .unwrap_or(false)
    {
        TileKind::Static
    } else {
        TileKind::Dynamic
    };

    let mut tile = Tile::new(position, kind);
    if house_id.is_some() {
        tile.set_flag(TILESTATE_PROTECTIONZONE);
    }
    if let Some(ground) = ground {
        tile.internal_add_item(ground, items);
    }
    tile
}

fn map_tile_flags(flags: u32) -> u32 {
    let mut tile_flags = 0;
    if flags & OTBM_TILEFLAG_PROTECTIONZONE != 0 {
        tile_flags |= TILESTATE_PROTECTIONZONE;
    } else if flags & OTBM_TILEFLAG_NOPVPZONE != 0 {
        tile_flags |= TILESTATE_NOPVPZONE;
    } else if flags & OTBM_TILEFLAG_PVPZONE != 0 {
        tile_flags |= TILESTATE_PVPZONE;
    }
    if flags & OTBM_TILEFLAG_NOLOGOUT != 0 {
        tile_flags |= TILESTATE_NOLOGOUT;
    }
    tile_flags
}

fn resolve_sidecar_path(file_name: &Path, child: &str) -> String {
    let parent = file_name.parent().unwrap_or_else(|| Path::new(""));
    let child_path = Path::new(child);
    let normalized_child = child_path.to_path_buf();

    parent
        .join(OsString::from(normalized_child))
        .display()
        .to_string()
}

fn decode_string(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

#[derive(Debug, Error)]
pub enum OtbmError {
    #[error(transparent)]
    Otb(#[from] OtbError),
    #[error("missing root properties")]
    MissingRootProperties,
    #[error("missing map data node")]
    MissingMapDataNode,
    #[error("unsupported map version: {0}")]
    UnsupportedMapVersion(u32),
    #[error("unsupported map item major version: {0}")]
    UnsupportedItemMajorVersion(u32),
    #[error("unsupported map item minor version: {0}")]
    UnsupportedItemMinorVersion(u32),
    #[error("invalid map data attributes")]
    InvalidMapDataAttributes,
    #[error("unknown header node: {0}")]
    UnknownHeaderNode(u8),
    #[error("unknown map node: {0}")]
    UnknownMapNode(u8),
    #[error("invalid map node")]
    InvalidMapNode,
    #[error("unknown tile node: {0}")]
    UnknownTileNode(u8),
    #[error("could not read node data")]
    CouldNotReadNodeData,
    #[error("[x:{x}, y:{y}, z:{z}] unknown tile attribute {attribute}")]
    UnknownTileAttribute {
        x: u16,
        y: u16,
        z: u8,
        attribute: u8,
    },
    #[error("[x:{x}, y:{y}, z:{z}] unknown node type {node_type}")]
    UnknownNodeType {
        x: u16,
        y: u16,
        z: u8,
        node_type: u8,
    },
    #[error("unknown town node: {0}")]
    UnknownTownNode(u8),
    #[error("could not read town data")]
    CouldNotReadTownData,
    #[error("unknown waypoint node: {0}")]
    UnknownWaypointNode(u8),
    #[error("could not read waypoint data")]
    CouldNotReadWaypointData,
    #[error("invalid item node")]
    InvalidItemNode,
    #[error("unsupported item attribute {0}")]
    UnsupportedItemAttribute(u8),
    #[error("unsupported child item node {0}")]
    UnsupportedChildItemNode(u8),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::items::Items;

    use super::{
        load_from_path, Position, OTBM_ATTR_DESCRIPTION, OTBM_ATTR_EXT_HOUSE_FILE,
        OTBM_ATTR_EXT_SPAWN_FILE, OTBM_ATTR_ITEM, OTBM_ATTR_TILE_FLAGS, OTBM_HOUSETILE, OTBM_ITEM,
        OTBM_MAP_DATA, OTBM_TILE, OTBM_TILEFLAG_PROTECTIONZONE, OTBM_TILE_AREA,
        OTBM_TOWN, OTBM_TOWNS, OTBM_WAYPOINT, OTBM_WAYPOINTS,
    };
    use crate::io::otb::Node;

    #[test]
    fn load_from_path_should_parse_otbm_header_tiles_towns_and_waypoints() {
        let mut items = Items::new();
        items
            .load_from_otb_bytes(build_items_otb_bytes())
            .expect("items.otb should load");

        let path = std::env::temp_dir().join("tfs-rust-test.otbm");
        fs::write(&path, build_otbm_bytes()).expect("otbm should be writable");

        let map = load_from_path(&path, &items).expect("otbm should load");

        assert_eq!(map.width, 256);
        assert_eq!(map.height, 512);
        assert!(map.description.contains("Forgotten"));
        assert!(map
            .spawn_file
            .as_deref()
            .unwrap_or_default()
            .ends_with("forgotten-spawn.xml"));
        assert!(map
            .house_file
            .as_deref()
            .unwrap_or_default()
            .ends_with("forgotten-house.xml"));

        let tile = map
            .get_tile(Position {
                x: 100,
                y: 200,
                z: 7,
            })
            .expect("tile should exist");
        assert!(tile.ground.is_some());
        assert_eq!(tile.items.len(), 1);
        assert_eq!(tile.items[0].action_id, 4500);
        assert_eq!(tile.items[0].text, "hello");
        assert!(tile.has_flag(crate::map::tile::TILESTATE_PROTECTIONZONE));

        let house = map.houses.get_house(77).expect("house should exist");
        assert_eq!(house.tiles.len(), 1);
        assert_eq!(
            map.towns.get(&1).map(|town| town.name.as_str()),
            Some("Temple")
        );
        assert_eq!(
            map.waypoints.get("depot").map(|position| position.x),
            Some(102)
        );

        fs::remove_file(path).expect("otbm should be removable");
    }

    fn build_items_otb_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OTBI");
        bytes.push(Node::START);
        bytes.push(0x00);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0x01);
        bytes.extend_from_slice(&(140u16).to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&19u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 128]);

        bytes.push(Node::START);
        bytes.push(crate::items::ItemGroup::Ground as u8);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0x10);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.push(0x11);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&200u16.to_le_bytes());
        bytes.push(Node::END);

        bytes.push(Node::START);
        bytes.push(crate::items::ItemGroup::Container as u8);
        bytes.extend_from_slice(&(1u32).to_le_bytes());
        bytes.push(0x10);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&101u16.to_le_bytes());
        bytes.push(0x11);
        bytes.extend_from_slice(&(2u16).to_le_bytes());
        bytes.extend_from_slice(&201u16.to_le_bytes());
        bytes.push(Node::END);
        bytes.push(Node::END);
        bytes
    }

    fn build_otbm_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OTBM");
        bytes.push(Node::START);
        bytes.push(1); // OTBM_ROOTV1
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&256u16.to_le_bytes());
        bytes.extend_from_slice(&512u16.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&19u32.to_le_bytes());

        bytes.push(Node::START);
        bytes.push(OTBM_MAP_DATA);
        push_string_attr(&mut bytes, OTBM_ATTR_DESCRIPTION, "Forgotten Test");
        push_string_attr(&mut bytes, OTBM_ATTR_EXT_SPAWN_FILE, "forgotten-spawn.xml");
        push_string_attr(&mut bytes, OTBM_ATTR_EXT_HOUSE_FILE, "forgotten-house.xml");

        bytes.push(Node::START);
        bytes.push(OTBM_TILE_AREA);
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.extend_from_slice(&200u16.to_le_bytes());
        bytes.push(7);

        bytes.push(Node::START);
        bytes.push(OTBM_TILE);
        bytes.push(0);
        bytes.push(0);
        bytes.push(OTBM_ATTR_TILE_FLAGS);
        bytes.extend_from_slice(&OTBM_TILEFLAG_PROTECTIONZONE.to_le_bytes());
        bytes.push(OTBM_ATTR_ITEM);
        bytes.extend_from_slice(&100u16.to_le_bytes());

        bytes.push(Node::START);
        bytes.push(OTBM_ITEM);
        bytes.extend_from_slice(&101u16.to_le_bytes());
        bytes.push(4);
        bytes.extend_from_slice(&4500u16.to_le_bytes());
        bytes.push(6);
        bytes.extend_from_slice(&5u16.to_le_bytes());
        bytes.extend_from_slice(b"hello");
        bytes.push(0);
        bytes.push(Node::END);
        bytes.push(Node::END);

        bytes.push(Node::START);
        bytes.push(OTBM_HOUSETILE);
        bytes.push(1);
        bytes.push(0);
        bytes.extend_from_slice(&77u32.to_le_bytes());
        bytes.push(Node::END);
        bytes.push(Node::END);

        bytes.push(Node::START);
        bytes.push(OTBM_TOWNS);
        bytes.push(Node::START);
        bytes.push(OTBM_TOWN);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&6u16.to_le_bytes());
        bytes.extend_from_slice(b"Temple");
        bytes.extend_from_slice(&101u16.to_le_bytes());
        bytes.extend_from_slice(&201u16.to_le_bytes());
        bytes.push(7);
        bytes.push(Node::END);
        bytes.push(Node::END);

        bytes.push(Node::START);
        bytes.push(OTBM_WAYPOINTS);
        bytes.push(Node::START);
        bytes.push(OTBM_WAYPOINT);
        bytes.extend_from_slice(&5u16.to_le_bytes());
        bytes.extend_from_slice(b"depot");
        bytes.extend_from_slice(&102u16.to_le_bytes());
        bytes.extend_from_slice(&202u16.to_le_bytes());
        bytes.push(7);
        bytes.push(Node::END);
        bytes.push(Node::END);

        bytes.push(Node::END);
        bytes.push(Node::END);
        bytes
    }

    fn push_string_attr(bytes: &mut Vec<u8>, attribute: u8, value: &str) {
        bytes.push(attribute);
        bytes.extend_from_slice(&(value.len() as u16).to_le_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
}
