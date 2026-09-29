use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use aes_gcm_siv::{Aes256GcmSiv, Key as SivKey, Nonce as SivNonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

const VERSION: u8 = 0x01;

const NONCE_SIZE: usize = 12;

#[derive(Debug, PartialEq, Eq)]
pub enum SecretError {
    Base64,
    TooShort,
    UnsupportedVersion,
    Crypto,
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::Base64 => write!(f, "secret: invalid base64"),
            SecretError::TooShort => write!(f, "secret: blob too short"),
            SecretError::UnsupportedVersion => write!(f, "secret: unsupported version"),
            SecretError::Crypto => write!(f, "secret: crypto failure"),
        }
    }
}

impl std::error::Error for SecretError {}

fn cipher(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
}

pub fn seal_with(key: &[u8; 32], plaintext: &str) -> Result<String, SecretError> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher(key)
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| SecretError::Crypto)?;
    let mut buf = Vec::with_capacity(1 + NONCE_SIZE + ct.len());
    buf.push(VERSION);
    buf.extend_from_slice(nonce.as_slice());
    buf.extend_from_slice(&ct);
    Ok(B64.encode(buf))
}

pub fn open_with(key: &[u8; 32], blob: &str) -> Result<String, SecretError> {
    if blob.is_empty() {
        return Ok(String::new());
    }
    let raw = B64.decode(blob).map_err(|_| SecretError::Base64)?;
    if raw.is_empty() {
        return Err(SecretError::TooShort);
    }
    if raw[0] != VERSION {
        return Err(SecretError::UnsupportedVersion);
    }
    if raw.len() < 1 + NONCE_SIZE {
        return Err(SecretError::TooShort);
    }
    let nonce = Nonce::from_slice(&raw[1..1 + NONCE_SIZE]);
    let ct = &raw[1 + NONCE_SIZE..];
    let pt = cipher(key)
        .decrypt(nonce, ct)
        .map_err(|_| SecretError::Crypto)?;
    String::from_utf8(pt).map_err(|_| SecretError::Crypto)
}

pub fn seal_segment(key: &[u8; 32], note_id: &str, plaintext: &str) -> Result<String, SecretError> {
    use sha2::{Digest, Sha256};

    let digest =
        Sha256::digest([b"draftnote-segment-v1\0".as_slice(), note_id.as_bytes()].concat());
    let nonce = SivNonce::from_slice(&digest[..12]);
    let aad = segment_aad(note_id);
    let cipher = Aes256GcmSiv::new(SivKey::<Aes256GcmSiv>::from_slice(key));
    let ciphertext = cipher
        .encrypt(
            nonce,
            aes_gcm_siv::aead::Payload {
                msg: plaintext.as_bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| SecretError::Crypto)?;
    Ok(B64.encode(ciphertext))
}

pub fn open_segment(key: &[u8; 32], note_id: &str, blob: &str) -> Result<String, SecretError> {
    use sha2::{Digest, Sha256};

    let ciphertext = B64.decode(blob).map_err(|_| SecretError::Base64)?;
    if ciphertext.len() < 16 {
        return Err(SecretError::TooShort);
    }
    let digest =
        Sha256::digest([b"draftnote-segment-v1\0".as_slice(), note_id.as_bytes()].concat());
    let nonce = SivNonce::from_slice(&digest[..12]);
    let aad = segment_aad(note_id);
    let cipher = Aes256GcmSiv::new(SivKey::<Aes256GcmSiv>::from_slice(key));
    let plaintext = cipher
        .decrypt(
            nonce,
            aes_gcm_siv::aead::Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| SecretError::Crypto)?;
    String::from_utf8(plaintext).map_err(|_| SecretError::Crypto)
}

fn segment_aad(note_id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(note_id.len() + 24);
    aad.extend_from_slice(b"draftnote-note-segment-v1\0");
    aad.extend_from_slice(&(note_id.len() as u64).to_be_bytes());
    aad.extend_from_slice(note_id.as_bytes());
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_segment_encryption_is_stable_and_authenticated() {
        let key: [u8; 32] = Aes256Gcm::generate_key(&mut OsRng).into();
        let first = seal_segment(&key, "note-a", "same paragraph").unwrap();
        let repeated = seal_segment(&key, "note-a", "same paragraph").unwrap();
        let other_note = seal_segment(&key, "note-b", "same paragraph").unwrap();

        assert_eq!(first, repeated);
        assert_ne!(first, other_note);
        assert_eq!(
            open_segment(&key, "note-a", &first).unwrap(),
            "same paragraph"
        );
        assert!(open_segment(&key, "note-a", &other_note).is_err());
        let mut tampered = B64.decode(&first).unwrap();
        tampered[0] ^= 1;
        assert!(open_segment(&key, "note-a", &B64.encode(tampered)).is_err());
    }

    fn from_hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn nist_aes256gcm_known_answer() {
        let key = [0u8; 32];
        let iv = [0u8; 12];
        let pt = [0u8; 16];
        let expected_ct = "cea7403d4d606b6e074ec5d3baf39d18";
        let expected_tag = "d0d1c8a799996bf0265b98b5d48ab919";
        let c = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let out = c.encrypt(Nonce::from_slice(&iv), pt.as_ref()).unwrap();
        let expected: Vec<u8> = from_hex(&format!("{expected_ct}{expected_tag}"));
        assert_eq!(out, expected, "aes-256-gcm primitive must match NIST KAT");
    }

    fn test_key() -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(b"draftnote-unit-test-key").into()
    }

    #[test]
    fn seal_open_round_trip() {
        let key = test_key();
        let secret = "ghp_exampleToken_1234567890";
        let blob = seal_with(&key, secret).unwrap();
        assert_ne!(blob, secret);
        assert_eq!(open_with(&key, &blob).unwrap(), secret);
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let blob = seal_with(&test_key(), "secret").unwrap();
        let mut other = test_key();
        other[0] ^= 0xff;
        assert_eq!(open_with(&other, &blob), Err(SecretError::Crypto));
    }

    #[test]
    fn empty_maps_to_empty() {
        let key = test_key();
        assert_eq!(seal_with(&key, "").unwrap(), "");
        assert_eq!(open_with(&key, "").unwrap(), "");
    }

    #[test]
    fn wire_format_is_version_nonce_ct() {
        let blob = seal_with(&test_key(), "x").unwrap();
        let raw = B64.decode(&blob).unwrap();
        assert_eq!(raw[0], VERSION);
        assert_eq!(raw.len(), 1 + NONCE_SIZE + 1 + 16);
    }

    #[test]
    fn open_rejects_bad_version() {
        let mut raw = vec![0x02u8];
        raw.extend_from_slice(&[0u8; NONCE_SIZE + 16 + 1]);
        let blob = B64.encode(raw);
        assert_eq!(
            open_with(&test_key(), &blob),
            Err(SecretError::UnsupportedVersion)
        );
    }

    #[test]
    fn open_rejects_short_blob() {
        let blob = B64.encode([VERSION, 0x00, 0x01]);
        assert_eq!(open_with(&test_key(), &blob), Err(SecretError::TooShort));
    }

    #[test]
    fn open_rejects_bad_base64() {
        assert_eq!(
            open_with(&test_key(), "!!!not-base64!!!"),
            Err(SecretError::Base64)
        );
    }
}
