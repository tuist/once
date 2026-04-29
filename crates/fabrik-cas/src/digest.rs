use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// BLAKE3-256 digest. Stored as a fixed 32-byte array; serialized as a
/// 64-character lowercase hex string for human-readable wire formats.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest([u8; Self::LEN]);

impl Digest {
    pub const LEN: usize = 32;

    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse a 64-character lowercase hex string. Returns `None` for any
    /// other length, character set, or case.
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != Self::LEN * 2 || s.chars().any(|c| !matches!(c, '0'..='9' | 'a'..='f')) {
            return None;
        }
        let mut out = [0u8; Self::LEN];
        hex::decode_to_slice(s, &mut out).ok()?;
        Some(Self(out))
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({})", self.to_hex())
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Digest;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a 64-character lowercase hex BLAKE3 digest")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Digest, E> {
                Digest::from_hex(v).ok_or_else(|| E::custom("invalid digest"))
            }
        }
        deserializer.deserialize_str(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_hex() {
        let d = Digest::of_bytes(b"hello");
        let hex = d.to_hex();
        assert_eq!(Digest::from_hex(&hex), Some(d));
    }

    #[test]
    fn rejects_uppercase_and_short() {
        assert!(Digest::from_hex("ABCD").is_none());
        assert!(Digest::from_hex(&"a".repeat(63)).is_none());
        assert!(Digest::from_hex(&"A".repeat(64)).is_none());
    }

    #[test]
    fn serde_roundtrip_is_hex() {
        let d = Digest::of_bytes(b"x");
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json.len(), 66); // two quotes + 64 hex chars
        let back: Digest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }
}
