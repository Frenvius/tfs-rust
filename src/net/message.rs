use std::mem::size_of;

use byteorder::{ByteOrder, LittleEndian};

pub const NETWORK_MESSAGE_MAXSIZE: usize = 65_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub x: u16,
    pub y: u16,
    pub z: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkMessageInfo {
    length: u16,
    position: u16,
    overrun: bool,
}

impl Default for NetworkMessageInfo {
    fn default() -> Self {
        Self {
            length: 0,
            position: NetworkMessage::INITIAL_BUFFER_POSITION,
            overrun: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkMessage {
    info: NetworkMessageInfo,
    buffer: [u8; NETWORK_MESSAGE_MAXSIZE],
}

impl Default for NetworkMessage {
    fn default() -> Self {
        Self {
            info: NetworkMessageInfo::default(),
            buffer: [0; NETWORK_MESSAGE_MAXSIZE],
        }
    }
}

impl NetworkMessage {
    pub const INITIAL_BUFFER_POSITION: u16 = 8;
    pub const HEADER_LENGTH: usize = 2;
    pub const CHECKSUM_LENGTH: usize = 4;
    pub const XTEA_MULTIPLE: usize = 8;
    pub const MAX_BODY_LENGTH: usize =
        NETWORK_MESSAGE_MAXSIZE - Self::HEADER_LENGTH - Self::CHECKSUM_LENGTH - Self::XTEA_MULTIPLE;
    pub const MAX_PROTOCOL_BODY_LENGTH: usize = Self::MAX_BODY_LENGTH - 10;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.info = NetworkMessageInfo::default();
    }

    pub fn get_byte(&mut self) -> u8 {
        if !self.can_read(1) {
            return 0;
        }

        let value = self.buffer[self.info.position as usize];
        self.info.position += 1;
        value
    }

    pub fn get_previous_byte(&mut self) -> u8 {
        self.info.position -= 1;
        self.buffer[self.info.position as usize]
    }

    pub fn get_u16(&mut self) -> u16 {
        self.read_numeric()
    }

    pub fn get_u32(&mut self) -> u32 {
        self.read_numeric()
    }

    pub fn get_u64(&mut self) -> u64 {
        self.read_numeric()
    }

    pub fn get_string(&mut self, string_len: Option<u16>) -> Vec<u8> {
        let string_len = string_len.unwrap_or_else(|| self.get_u16()) as usize;
        if !self.can_read(string_len) {
            return Vec::new();
        }

        let start = self.info.position as usize;
        let end = start + string_len;
        self.info.position += string_len as u16;
        self.buffer[start..end].to_vec()
    }

    pub fn get_position(&mut self) -> Position {
        Position {
            x: self.get_u16(),
            y: self.get_u16(),
            z: self.get_byte(),
        }
    }

    pub fn skip_bytes(&mut self, count: i16) {
        self.info.position = self.info.position.wrapping_add_signed(count);
    }

    pub fn add_byte(&mut self, value: u8) {
        if !self.can_add(1) {
            return;
        }

        self.buffer[self.info.position as usize] = value;
        self.info.position += 1;
        self.info.length += 1;
    }

    pub fn add_u16(&mut self, value: u16) {
        self.write_numeric(value);
    }

    pub fn add_u32(&mut self, value: u32) {
        self.write_numeric(value);
    }

    pub fn add_u64(&mut self, value: u64) {
        self.write_numeric(value);
    }

    pub fn add_bytes(&mut self, bytes: &[u8]) {
        if !self.can_add(bytes.len()) || bytes.len() > 8_192 {
            return;
        }

        let start = self.info.position as usize;
        let end = start + bytes.len();
        self.buffer[start..end].copy_from_slice(bytes);
        self.info.position += bytes.len() as u16;
        self.info.length += bytes.len() as u16;
    }

    pub fn add_padding_bytes(&mut self, count: usize) {
        if !self.can_add(count) {
            return;
        }

        let start = self.info.position as usize;
        let end = start + count;
        self.buffer[start..end].fill(0x33);
        self.info.length += count as u16;
    }

    pub fn add_string(&mut self, value: &[u8]) {
        if !self.can_add(value.len() + size_of::<u16>()) || value.len() > 8_192 {
            return;
        }

        self.add_u16(value.len() as u16);
        self.add_bytes(value);
    }

    pub fn add_double(&mut self, value: f64, precision: u8) {
        self.add_byte(precision);
        let scaled = (value * 10f64.powi(i32::from(precision))) + f64::from(i32::MAX);
        self.add_u32(scaled as u32);
    }

    pub fn add_position(&mut self, position: Position) {
        self.add_u16(position.x);
        self.add_u16(position.y);
        self.add_byte(position.z);
    }

    pub fn get_length(&self) -> u16 {
        self.info.length
    }

    pub fn set_length(&mut self, new_length: u16) {
        self.info.length = new_length;
    }

    pub fn get_buffer_position(&self) -> u16 {
        self.info.position
    }

    pub fn set_buffer_position(&mut self, position: u16) -> bool {
        if usize::from(position)
            < NETWORK_MESSAGE_MAXSIZE - usize::from(Self::INITIAL_BUFFER_POSITION)
        {
            self.info.position = position + Self::INITIAL_BUFFER_POSITION;
            return true;
        }

        false
    }

    pub fn get_length_header(&self) -> u16 {
        LittleEndian::read_u16(&self.buffer[..2])
    }

    pub fn is_overrun(&self) -> bool {
        self.info.overrun
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut [u8] {
        &mut self.buffer
    }

    pub fn body_buffer_mut(&mut self) -> &mut [u8] {
        self.info.position = 2;
        &mut self.buffer[Self::HEADER_LENGTH..]
    }

    fn can_add(&self, size: usize) -> bool {
        size + usize::from(self.info.position) < Self::MAX_BODY_LENGTH
    }

    fn can_read(&mut self, size: usize) -> bool {
        let position = usize::from(self.info.position);
        let length = usize::from(self.info.length);

        if position + size > length + usize::from(Self::INITIAL_BUFFER_POSITION)
            || size >= (NETWORK_MESSAGE_MAXSIZE - position)
        {
            self.info.overrun = true;
            return false;
        }

        true
    }

    fn read_numeric<T>(&mut self) -> T
    where
        T: NumericLe,
    {
        if !self.can_read(size_of::<T>()) {
            return T::zero();
        }

        let start = self.info.position as usize;
        let end = start + size_of::<T>();
        let value = T::read(&self.buffer[start..end]);
        self.info.position += size_of::<T>() as u16;
        value
    }

    fn write_numeric<T>(&mut self, value: T)
    where
        T: NumericLe,
    {
        if !self.can_add(size_of::<T>()) {
            return;
        }

        let start = self.info.position as usize;
        let end = start + size_of::<T>();
        value.write(&mut self.buffer[start..end]);
        self.info.position += size_of::<T>() as u16;
        self.info.length += size_of::<T>() as u16;
    }
}

trait NumericLe: Sized {
    fn read(bytes: &[u8]) -> Self;
    fn write(self, bytes: &mut [u8]);
    fn zero() -> Self;
}

impl NumericLe for u16 {
    fn read(bytes: &[u8]) -> Self {
        LittleEndian::read_u16(bytes)
    }

    fn write(self, bytes: &mut [u8]) {
        LittleEndian::write_u16(bytes, self);
    }

    fn zero() -> Self {
        0
    }
}

impl NumericLe for u32 {
    fn read(bytes: &[u8]) -> Self {
        LittleEndian::read_u32(bytes)
    }

    fn write(self, bytes: &mut [u8]) {
        LittleEndian::write_u32(bytes, self);
    }

    fn zero() -> Self {
        0
    }
}

impl NumericLe for u64 {
    fn read(bytes: &[u8]) -> Self {
        LittleEndian::read_u64(bytes)
    }

    fn write(self, bytes: &mut [u8]) {
        LittleEndian::write_u64(bytes, self);
    }

    fn zero() -> Self {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{NetworkMessage, Position};

    #[test]
    fn add_and_get_primitives_should_use_little_endian_layout() {
        let mut message = NetworkMessage::new();

        message.add_byte(0x12);
        message.add_u16(0x3456);
        message.add_u32(0x789A_BCDE);
        message.set_buffer_position(0);
        message.set_length(7);

        assert_eq!(message.get_byte(), 0x12);
        assert_eq!(message.get_u16(), 0x3456);
        assert_eq!(message.get_u32(), 0x789A_BCDE);
    }

    #[test]
    fn add_and_get_string_should_round_trip_bytes() {
        let mut message = NetworkMessage::new();

        message.add_string(b"Tibia");
        message.set_buffer_position(0);
        message.set_length(7);

        assert_eq!(message.get_string(None), b"Tibia");
    }

    #[test]
    fn get_position_should_read_expected_wire_layout() {
        let mut message = NetworkMessage::new();
        let position = Position {
            x: 100,
            y: 200,
            z: 7,
        };

        message.add_position(position);
        message.set_buffer_position(0);
        message.set_length(5);

        assert_eq!(message.get_position(), position);
    }

    #[test]
    fn reading_past_the_available_body_should_set_overrun() {
        let mut message = NetworkMessage::new();
        message.set_length(0);

        assert_eq!(message.get_byte(), 0);
        assert!(message.is_overrun());
    }
}
