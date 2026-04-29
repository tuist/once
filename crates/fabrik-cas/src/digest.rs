use std::fmt;

use serde::{Deserialize, Serialize};

/// BLAKE3-256 digest. The wire format is lowercase hex.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest(String);

impl Digest {
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let hash = blake3::hash(bytes);
        Self(hash.to_hex().to_string())
    }

    pub fn to_hex(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({})", self.0)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
