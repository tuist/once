use struo_cas::Digest;

// Released SDK keys hash this domain, so it keeps its original `once` name to
// keep existing keys stable across the rename.
const DOMAIN: &[u8] = b"once.sdk.action-key.v1\0";

/// Build a stable digest for one cached automation action.
///
/// The namespace partitions unrelated integrations. Inputs are ordered, so
/// callers should add them in a deterministic order. Labels and values use a
/// length-prefixed encoding, and the versioned domain lets Struo evolve the
/// format without making old cache entries unsafe.
#[derive(Debug, Clone)]
pub struct ActionKeyBuilder {
    hasher: blake3::Hasher,
}

impl ActionKeyBuilder {
    /// Start a key in `namespace`.
    ///
    /// The namespace is mixed in first, so two integrations that push
    /// identical inputs still get distinct keys.
    pub fn new(namespace: impl AsRef<[u8]>) -> Self {
        let namespace = namespace.as_ref();
        let mut hasher = blake3::Hasher::new();
        hasher.update(DOMAIN);
        push_field(&mut hasher, namespace);
        Self { hasher }
    }

    /// Add an input that is already content-addressed.
    ///
    /// Prefer this over [`push_bytes`](Self::push_bytes) for file
    /// contents: the digest is fixed width, so the key stays small no
    /// matter how large the input is.
    pub fn push_digest(&mut self, label: impl AsRef<[u8]>, digest: Digest) -> &mut Self {
        self.hasher.update(&[1]);
        push_field(&mut self.hasher, label.as_ref());
        self.hasher.update(digest.as_bytes());
        self
    }

    /// Add a literal input such as a tool name, flag, or version string.
    ///
    /// Label and value are both length-prefixed, so `("ab", "c")` and
    /// `("a", "bc")` produce different keys.
    pub fn push_bytes(&mut self, label: impl AsRef<[u8]>, bytes: impl AsRef<[u8]>) -> &mut Self {
        self.hasher.update(&[2]);
        push_field(&mut self.hasher, label.as_ref());
        push_field(&mut self.hasher, bytes.as_ref());
        self
    }

    /// Consume the builder and return the action key.
    ///
    /// Use the result as the action digest passed to
    /// [`Cache::get_action_result`](crate::Cache::get_action_result) and
    /// [`Cache::put_action_result`](crate::Cache::put_action_result).
    pub fn finish(self) -> Digest {
        Digest::from_bytes(*self.hasher.finalize().as_bytes())
    }
}

fn push_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    hasher.update(&length.to_le_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_keys_match_the_original_buffered_encoding() {
        fn field(encoded: &mut Vec<u8>, bytes: &[u8]) {
            encoded.extend_from_slice(&u64::try_from(bytes.len()).unwrap().to_le_bytes());
            encoded.extend_from_slice(bytes);
        }

        for size in [0, 1, 1023, 1024, 1025, 1024 * 1024] {
            let payload = vec![0xa5; size];
            let digest = Digest::of_bytes(&payload);
            let mut encoded = DOMAIN.to_vec();
            field(&mut encoded, b"namespace\0");
            encoded.push(2);
            field(&mut encoded, b"literal\0");
            field(&mut encoded, &payload);
            encoded.push(1);
            field(&mut encoded, b"digest");
            encoded.extend_from_slice(digest.as_bytes());
            encoded.push(2);
            field(&mut encoded, b"");
            field(&mut encoded, b"");

            let mut key = ActionKeyBuilder::new(b"namespace\0");
            key.push_bytes(b"literal\0", &payload)
                .push_digest("digest", digest)
                .push_bytes("", "");
            assert_eq!(key.finish(), Digest::of_bytes(&encoded));
        }
    }

    #[test]
    fn an_empty_builder_matches_the_original_encoding() {
        for namespace in ["", "compile"] {
            let mut encoded = DOMAIN.to_vec();
            encoded.extend_from_slice(&u64::try_from(namespace.len()).unwrap().to_le_bytes());
            encoded.extend_from_slice(namespace.as_bytes());
            assert_eq!(
                ActionKeyBuilder::new(namespace).finish(),
                Digest::of_bytes(&encoded)
            );
        }
    }

    #[test]
    fn cloning_a_builder_produces_independent_hash_states() {
        let mut original = ActionKeyBuilder::new("compile");
        original.push_bytes("shared", vec![0xa5; 2048]);
        let mut fork = original.clone();
        assert_eq!(original.clone().finish(), fork.clone().finish());
        original.push_bytes("mode", "debug");
        fork.push_bytes("mode", "release");
        assert_ne!(original.finish(), fork.finish());
    }

    #[test]
    fn keys_match_the_released_v1_golden_vector() {
        let mut key = ActionKeyBuilder::new("compile");
        key.push_bytes("tool", "swiftc")
            .push_digest("source", Digest::of_bytes(b"source"));
        assert_eq!(
            key.finish().to_string(),
            "c67ca7a97ebabdf6129cf60224eb10b9a235a4244bb023495bb4bd22ab9ad747"
        );
    }

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
