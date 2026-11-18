use serde::{Deserialize, Serialize};

/// Producer-owned logical identity for history, independent of execution identity.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ActionHistoryKey {
    pub namespace: String,
    pub key: String,
}

impl ActionHistoryKey {
    pub fn normalize(self) -> Option<Self> {
        let namespace_valid = !self.namespace.is_empty()
            && self.namespace.len() <= 64
            && self
                .namespace
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        let key_valid = !self.key.is_empty()
            && self.key.len() <= 128
            && !self.key.chars().any(char::is_control);
        (namespace_valid && key_valid).then_some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::ActionHistoryKey;

    #[test]
    fn preserves_future_namespaces_and_opaque_keys() {
        let key = ActionHistoryKey {
            namespace: "vendor.compiler.v7".into(),
            key: "subject:compile".into(),
        };
        assert_eq!(key.clone().normalize(), Some(key));
    }

    #[test]
    fn rejects_invalid_and_oversized_identity_without_truncation() {
        for (namespace, key) in [
            ("", "key".to_string()),
            ("invalid namespace", "key".to_string()),
            ("valid", String::new()),
            ("valid", "x".repeat(129)),
            ("valid", "key\n".to_string()),
        ] {
            assert!(ActionHistoryKey {
                namespace: namespace.into(),
                key,
            }
            .normalize()
            .is_none());
        }
    }
}
