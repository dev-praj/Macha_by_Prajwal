//! End-to-end envelope protection for the Supabase relay. Supabase receives
//! only the envelope metadata and authenticated AES-GCM ciphertext.

use base64::Engine as _;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::hkdf::{HKDF_SHA256, Salt};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

const HKDF_SALT: &[u8] = b"macha-supabase-relay-v1";
const MAX_AGE_MS: u64 = 600_000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub seq: u64,
    pub ts: u64,
    pub nonce: String,
    pub ciphertext: String,
}

pub struct Identity {
    private: StaticSecret,
    pub public: [u8; 32],
}

impl Identity {
    pub fn generate() -> Self {
        let private = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let public = PublicKey::from(&private).to_bytes();
        Self { private, public }
    }

    pub fn from_private_b64(value: &str) -> Option<Self> {
        let private: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(value).ok()?.try_into().ok()?;
        let private = StaticSecret::from(private);
        let public = PublicKey::from(&private).to_bytes();
        Some(Self { private, public })
    }

    pub fn private_b64(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.private.to_bytes())
    }

    pub fn public_b64(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.public)
    }

    pub fn derive(&self, peer_public_b64: &str, link_id: &str) -> Result<[u8; 32], String> {
        let peer: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(peer_public_b64).map_err(|_| "Invalid peer public key".to_string())?.try_into().map_err(|_| "Invalid peer public key length".to_string())?;
        let secret = self.private.diffie_hellman(&PublicKey::from(peer));
        if secret.as_bytes().iter().all(|b| *b == 0) { return Err("Invalid X25519 public key".into()); }
        let prk = Salt::new(HKDF_SHA256, HKDF_SALT).extract(secret.as_bytes());
        let info = [b"link:".as_slice(), link_id.as_bytes()].concat();
        let mut out = [0u8; 32];
        prk.expand(&[&info], KeyLen(32)).map_err(|_| "Could not derive the relay key".to_string())?.fill(&mut out).map_err(|_| "Could not derive the relay key".to_string())?;
        Ok(out)
    }
}

struct KeyLen(usize);
impl ring::hkdf::KeyType for KeyLen { fn len(&self) -> usize { self.0 } }

pub struct Codec {
    key: [u8; 32],
    sent: u64,
    received: u64,
}

impl Codec {
    pub fn new(key: [u8; 32]) -> Self { Self { key, sent: 0, received: 0 } }
    pub fn with_counters(key: [u8; 32], sent: u64, received: u64) -> Self { Self { key, sent, received } }
    pub fn counters(&self) -> (u64, u64) { (self.sent, self.received) }

    pub fn seal(&mut self, text: &str, kind: &str) -> Result<Envelope, String> {
        self.sent = self.sent.checked_add(1).ok_or("Cloud message counter exhausted")?;
        let ts = now_ms();
        let id = random_id()?;
        let mut nonce_bytes = [0u8; 12];
        SystemRandom::new().fill(&mut nonce_bytes).map_err(|_| "No secure random source")?;
        let aad = aad(&id, kind, self.sent, ts);
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &self.key).map_err(|_| "Invalid cloud key")?);
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut ciphertext = text.as_bytes().to_vec();
        key.seal_in_place_append_tag(nonce, Aad::from(aad.as_bytes()), &mut ciphertext).map_err(|_| "Could not encrypt cloud message")?;
        Ok(Envelope {
            id,
            kind: kind.to_string(),
            seq: self.sent,
            ts,
            nonce: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce_bytes),
            ciphertext: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(ciphertext),
        })
    }

    pub fn open(&mut self, envelope: &Envelope) -> Result<String, String> {
        if envelope.seq <= self.received { return Err("Duplicate or out-of-order cloud message".into()); }
        if now_ms().abs_diff(envelope.ts) > MAX_AGE_MS { return Err("Cloud message expired".into()); }
        let nonce: [u8; 12] = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&envelope.nonce).map_err(|_| "Invalid message nonce")?.try_into().map_err(|_| "Invalid message nonce length")?;
        let mut ciphertext = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&envelope.ciphertext).map_err(|_| "Invalid ciphertext")?;
        let aad = aad(&envelope.id, &envelope.kind, envelope.seq, envelope.ts);
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &self.key).map_err(|_| "Invalid cloud key")?);
        let plain = key.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad.as_bytes()), &mut ciphertext).map_err(|_| "Cloud message authentication failed")?;
        let text = std::str::from_utf8(plain).map_err(|_| "Cloud message is not UTF-8")?.to_string();
        self.received = envelope.seq;
        Ok(text)
    }
}

fn aad(id: &str, kind: &str, seq: u64, ts: u64) -> String { format!("{id}\n{kind}\n{seq}\n{ts}") }
fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis().min(u64::MAX as u128) as u64 }

fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| "No secure random source")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok(format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]))
}

#[cfg(test)]
mod tests {
    use super::{Codec, Identity};

    #[test]
    fn x25519_keys_derive_equal_keys_and_aes_gcm_rejects_replay_and_tampering() {
        let laptop = Identity::generate();
        let phone = Identity::generate();
        let link = "a-link-uuid";
        let laptop_key = laptop.derive(&phone.public_b64(), link).unwrap();
        let phone_key = phone.derive(&laptop.public_b64(), link).unwrap();
        assert_eq!(laptop_key, phone_key);
        let mut sender = Codec::new(laptop_key);
        let mut receiver = Codec::new(phone_key);
        let envelope = sender.seal("hello", "data").unwrap();
        assert_eq!(receiver.open(&envelope).unwrap(), "hello");
        assert!(receiver.open(&envelope).is_err());
        let mut altered = sender.seal("again", "data").unwrap();
        altered.ciphertext.push('x');
        assert!(receiver.open(&altered).is_err());
    }
}
