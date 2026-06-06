use std::fs;
use std::path::Path;
use std::sync::OnceLock;

static G_RSA: OnceLock<Rsa> = OnceLock::new();

pub fn g_rsa() -> &'static Rsa {
    G_RSA.get().expect("RSA not initialized")
}

pub(crate) fn init_rsa(rsa: Rsa) {
    G_RSA
        .set(rsa)
        .unwrap_or_else(|_| panic!("RSA already initialized"));
}

use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BigUint, RsaPrivateKey};
use thiserror::Error;

const RSA_BLOCK_SIZE: usize = 128;

#[derive(Debug)]
pub struct Rsa {
    private_key: RsaPrivateKey,
}

#[derive(Debug, Error)]
pub enum RsaError {
    #[error("failed to read `{path}`: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse RSA private key from `{path}`: {source}")]
    Parse {
        path: String,
        #[source]
        source: rsa::pkcs1::Error,
    },
    #[error("RSA private key is not valid: {0}")]
    InvalidKey(String),
    #[error("RSA input block must be exactly 128 bytes")]
    InvalidBlockLength,
}

impl Rsa {
    pub fn load_pem(path: impl AsRef<Path>) -> Result<Self, RsaError> {
        let path = path.as_ref();
        let pem = fs::read_to_string(path).map_err(|source| RsaError::Read {
            path: path.display().to_string(),
            source,
        })?;

        let private_key =
            RsaPrivateKey::from_pkcs1_pem(&pem).map_err(|source| RsaError::Parse {
                path: path.display().to_string(),
                source,
            })?;

        private_key
            .validate()
            .map_err(|source| RsaError::InvalidKey(source.to_string()))?;

        Ok(Self { private_key })
    }

    pub fn decrypt_block(&self, block: &mut [u8]) -> Result<(), RsaError> {
        if block.len() != RSA_BLOCK_SIZE {
            return Err(RsaError::InvalidBlockLength);
        }

        let value = BigUint::from_bytes_be(block);
        let decrypted = value.modpow(self.private_key.d(), self.private_key.n());
        let decrypted_bytes = decrypted.to_bytes_be();
        let padding = RSA_BLOCK_SIZE - decrypted_bytes.len();

        block.fill(0);
        block[padding..].copy_from_slice(&decrypted_bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rand::rngs::OsRng;
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    use rsa::{BigUint, RsaPrivateKey};

    use super::{Rsa, RsaError, RSA_BLOCK_SIZE};

    #[test]
    fn decrypt_block_should_reverse_raw_public_modular_exponentiation() {
        let mut rng = OsRng;
        let private_key =
            RsaPrivateKey::new(&mut rng, 1024).expect("test key generation should succeed");
        let public_key = private_key.to_public_key();

        let path = std::env::temp_dir().join("tfs-rust-rsa-test.pem");
        fs::write(
            &path,
            private_key
                .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                .expect("PEM encoding should succeed")
                .as_bytes(),
        )
        .expect("temp PEM should be writable");

        let rsa = Rsa::load_pem(&path).expect("PEM should load");
        let message = BigUint::from_bytes_be(&[0x42; 32]);
        let encrypted = message.modpow(public_key.e(), public_key.n());
        let mut block = [0u8; RSA_BLOCK_SIZE];
        let encrypted_bytes = encrypted.to_bytes_be();
        block[RSA_BLOCK_SIZE - encrypted_bytes.len()..].copy_from_slice(&encrypted_bytes);

        rsa.decrypt_block(&mut block)
            .expect("decrypt should succeed");

        let decrypted = BigUint::from_bytes_be(&block);
        assert_eq!(decrypted, message);

        fs::remove_file(path).expect("temp PEM should be removable");
    }

    #[test]
    fn decrypt_block_should_reject_non_128_byte_input() {
        let mut rng = OsRng;
        let private_key =
            RsaPrivateKey::new(&mut rng, 1024).expect("test key generation should succeed");
        let path = std::env::temp_dir().join("tfs-rust-rsa-invalid-block.pem");
        fs::write(
            &path,
            private_key
                .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                .expect("PEM encoding should succeed")
                .as_bytes(),
        )
        .expect("temp PEM should be writable");

        let rsa = Rsa::load_pem(&path).expect("PEM should load");
        let mut block = [0u8; 127];

        let error = rsa
            .decrypt_block(&mut block)
            .expect_err("short block should be rejected");

        assert!(matches!(error, RsaError::InvalidBlockLength));

        fs::remove_file(path).expect("temp PEM should be removable");
    }
}
