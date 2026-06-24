use byteorder::{ByteOrder, LittleEndian};

use crate::crypto::xtea::{self, expand_key, Key, RoundKeys};
use crate::net::message::NetworkMessage;
use crate::net::output_message::OutputMessage;

pub struct ProtocolCrypto {
    pub encryption_enabled: bool,
    pub checksum_enabled: bool,
    pub raw_messages: bool,
    pub round_keys: Option<Box<RoundKeys>>,
}

impl ProtocolCrypto {
    pub fn new(checksum_enabled: bool) -> Self {
        Self {
            encryption_enabled: false,
            checksum_enabled,
            raw_messages: false,
            round_keys: None,
        }
    }

    pub fn enable_xtea(&mut self, key: &Key) {
        self.round_keys = Some(Box::new(expand_key(key)));
        self.encryption_enabled = true;
    }

    pub fn finalize_output(&self, output: &mut OutputMessage) {
        if self.raw_messages {
            return;
        }
        output.write_message_length();
        if self.encryption_enabled {
            if let Some(ref rk) = self.round_keys {
                output.xtea_encrypt(rk);
            }
            output.add_crypto_header(self.checksum_enabled);
        }
    }
}

pub fn xtea_decrypt_incoming(msg: &mut NetworkMessage, round_keys: &RoundKeys) -> bool {
    let body_len = msg.get_length() as usize;
    if body_len < 4 {
        return false;
    }
    let xtea_len = body_len - 4;
    if xtea_len == 0 || xtea_len % 8 != 0 {
        return false;
    }

    // position is at 6 (after 4-byte checksum at body start)
    let pos = msg.get_buffer_position() as usize;

    let buf = msg.buffer_mut();
    if xtea::decrypt(&mut buf[pos..pos + xtea_len], round_keys).is_err() {
        return false;
    }

    let inner_len = LittleEndian::read_u16(&buf[pos..pos + 2]);

    msg.skip_bytes(2);
    msg.set_length(inner_len);
    true
}

pub fn rsa_decrypt(msg: &mut NetworkMessage) -> bool {
    let pos = msg.get_buffer_position() as usize;
    let body_len = msg.get_length() as usize;

    let available = body_len + 2 - pos;
    if available < 128 {
        return false;
    }

    if crate::crypto::rsa::g_rsa()
        .decrypt_block(&mut msg.buffer_mut()[pos..pos + 128])
        .is_err()
    {
        tracing::info!("rsa_decrypt: decrypt_block failed");
        return false;
    }

    let first_byte = msg.get_byte();
    tracing::info!(first_byte, "rsa_decrypt: first decrypted byte");
    first_byte == 0
}
