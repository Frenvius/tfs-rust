use byteorder::{ByteOrder, LittleEndian};

use crate::crypto::xtea::RoundKeys;
use crate::net::message::NETWORK_MESSAGE_MAXSIZE;
use crate::util::adler_checksum;

const INITIAL_BUFFER_POSITION: usize = 8;

pub struct OutputMessage {
    buffer: Box<[u8; NETWORK_MESSAGE_MAXSIZE]>,
    output_buffer_start: usize,
    length: usize,
    position: usize,
}

impl OutputMessage {
    pub fn new() -> Self {
        Self {
            buffer: Box::new([0u8; NETWORK_MESSAGE_MAXSIZE]),
            output_buffer_start: INITIAL_BUFFER_POSITION,
            length: 0,
            position: INITIAL_BUFFER_POSITION,
        }
    }

    pub fn add_byte(&mut self, value: u8) {
        if self.position < NETWORK_MESSAGE_MAXSIZE {
            self.buffer[self.position] = value;
            self.position += 1;
            self.sync_length();
        }
    }

    pub fn add_u16(&mut self, value: u16) {
        if self.position + 2 <= NETWORK_MESSAGE_MAXSIZE {
            LittleEndian::write_u16(&mut self.buffer[self.position..], value);
            self.position += 2;
            self.sync_length();
        }
    }

    pub fn add_u32(&mut self, value: u32) {
        if self.position + 4 <= NETWORK_MESSAGE_MAXSIZE {
            LittleEndian::write_u32(&mut self.buffer[self.position..], value);
            self.position += 4;
            self.sync_length();
        }
    }

    pub fn add_bytes(&mut self, data: &[u8]) {
        if self.position + data.len() <= NETWORK_MESSAGE_MAXSIZE {
            self.buffer[self.position..self.position + data.len()].copy_from_slice(data);
            self.position += data.len();
            self.sync_length();
        }
    }

    pub fn add_string(&mut self, data: &[u8]) {
        self.add_u16(data.len() as u16);
        self.add_bytes(data);
    }

    pub fn add_padding_bytes(&mut self, count: usize) {
        if self.position + count <= NETWORK_MESSAGE_MAXSIZE {
            self.buffer[self.position..self.position + count].fill(0x33);
            self.position += count;
            self.sync_length();
        }
    }

    pub fn skip_bytes(&mut self, count: isize) {
        self.position = (self.position as isize + count) as usize;
    }

    fn sync_length(&mut self) {
        let end = self.position - self.output_buffer_start;
        if end > self.length {
            self.length = end;
        }
    }

    fn add_header_u16(&mut self, value: u16) {
        self.output_buffer_start -= 2;
        LittleEndian::write_u16(&mut self.buffer[self.output_buffer_start..], value);
        self.length += 2;
    }

    fn add_header_u32(&mut self, value: u32) {
        self.output_buffer_start -= 4;
        LittleEndian::write_u32(&mut self.buffer[self.output_buffer_start..], value);
        self.length += 4;
    }

    pub fn write_message_length(&mut self) {
        let len = self.length as u16;
        self.add_header_u16(len);
    }

    pub fn add_crypto_header(&mut self, checksummed: bool) {
        if checksummed {
            let start = self.output_buffer_start;
            let len = self.length;
            let checksum = adler_checksum(&self.buffer[start..start + len]);
            self.add_header_u32(checksum);
        }
        self.write_message_length();
    }

    pub fn xtea_encrypt(&mut self, round_keys: &RoundKeys) {
        let padding = (8 - (self.length % 8)) % 8;
        if padding > 0 {
            self.add_padding_bytes(padding);
        }
        let start = self.output_buffer_start;
        let len = self.length;
        let _ = crate::crypto::xtea::encrypt(&mut self.buffer[start..start + len], round_keys);
    }

    pub fn get_output_buffer(&self) -> &[u8] {
        &self.buffer[self.output_buffer_start..self.output_buffer_start + self.length]
    }

    pub fn add_u64(&mut self, value: u64) {
        if self.position + 8 <= NETWORK_MESSAGE_MAXSIZE {
            LittleEndian::write_u64(&mut self.buffer[self.position..], value);
            self.position += 8;
            self.sync_length();
        }
    }

    pub fn get_raw_buffer(&self) -> &[u8] {
        &(*self.buffer)[..]
    }

    pub fn get_length(&self) -> usize {
        self.length
    }

    pub fn add_position(&mut self, x: u16, y: u16, z: u8) {
        self.add_u16(x);
        self.add_u16(y);
        self.add_byte(z);
    }
}

impl Default for OutputMessage {
    fn default() -> Self {
        Self::new()
    }
}
