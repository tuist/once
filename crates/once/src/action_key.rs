use once_cas::Digest;

const DOMAIN: &[u8] = b"once.sdk.action-key.v1\0";

/// Build a stable digest for one cached automation action.
///
/// The namespace partitions unrelated integrations. Inputs are ordered, so
/// callers should add them in a deterministic order. Labels and values use a
/// length-prefixed encoding, and the versioned domain lets Once evolve the
/// format without making old cache entries unsafe.
#[derive(Debug, Clone)]
pub struct ActionKeyBuilder {
    encoded: Vec<u8>,
}

impl ActionKeyBuilder {
    pub fn new(namespace: impl AsRef<[u8]>) -> Self {
        let namespace = namespace.as_ref();
        let mut encoded = Vec::with_capacity(DOMAIN.len() + namespace.len() + 64);
        encoded.extend_from_slice(DOMAIN);
        push_field(&mut encoded, namespace);
        Self { encoded }
    }

    pub fn push_digest(&mut self, label: impl AsRef<[u8]>, digest: Digest) -> &mut Self {
        self.encoded.push(1);
        push_field(&mut self.encoded, label.as_ref());
        self.encoded.extend_from_slice(digest.as_bytes());
        self
    }

    pub fn push_bytes(&mut self, label: impl AsRef<[u8]>, bytes: impl AsRef<[u8]>) -> &mut Self {
        self.encoded.push(2);
        push_field(&mut self.encoded, label.as_ref());
        push_field(&mut self.encoded, bytes.as_ref());
        self
    }

    pub fn finish(self) -> Digest {
        Digest::of_bytes(&self.encoded)
    }
}

fn push_field(encoded: &mut Vec<u8>, value: &[u8]) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    encoded.extend_from_slice(&length.to_le_bytes());
    encoded.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_actions_have_identical_keys() {
        let build = || {
            let mut key = ActionKeyBuilder::new("compile");
            key.push_bytes("tool", "swiftc")
                .push_digest("source", Digest::of_bytes(b"source"));
            key.finish()
        };

        assert_eq!(build(), build());
    }

    #[test]
    fn namespaces_labels_order_and_values_partition_keys() {
        let key = |namespace: &str, inputs: &[(&str, &str)]| {
            let mut key = ActionKeyBuilder::new(namespace);
            for (label, value) in inputs {
                key.push_bytes(label, value);
            }
            key.finish()
        };

        let base = key("compile", &[("tool", "swiftc"), ("mode", "debug")]);
        assert_ne!(base, key("link", &[("tool", "swiftc"), ("mode", "debug")]));
        assert_ne!(
            base,
            key("compile", &[("mode", "debug"), ("tool", "swiftc")])
        );
        assert_ne!(
            base,
            key("compile", &[("tool", "clang"), ("mode", "debug")])
        );
        assert_ne!(
            base,
            key("compile", &[("compiler", "swiftc"), ("mode", "debug")])
        );
    }
}
