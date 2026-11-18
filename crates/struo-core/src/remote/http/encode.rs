use crate::{Error, Result};
use serde::Serialize;
use std::io::{self, Write};

pub(in crate::remote) fn encode(
    value: &impl Serialize,
    executor: &'static str,
    limit: u64,
) -> Result<Vec<u8>> {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut writer = Bounded {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        return Err(if writer.exceeded {
            Error::ExecutorResourceLimit {
                executor: executor.into(),
                resource: "request metadata bytes",
                limit: u64::try_from(limit).unwrap_or(u64::MAX),
            }
        } else {
            Error::RemoteProviderApi {
                provider: executor.into(),
                message: "request metadata could not be encoded".into(),
            }
        });
    }
    Ok(writer.bytes)
}

struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self.bytes.len().checked_add(bytes.len());
        if next.is_none_or(|next| next > self.limit) {
            self.exceeded = true;
            return Err(io::Error::other("request metadata limit"));
        }
        let next = next.expect("length was checked");
        if self.bytes.capacity() < next {
            let capacity = next
                .checked_next_power_of_two()
                .unwrap_or(self.limit)
                .min(self.limit);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|_| io::Error::other("request metadata buffer unavailable"))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_bytes_and_escaping_are_bounded_before_sending() {
        let value = "\u{0000}".repeat(16);
        assert!(matches!(
            encode(&value, "test", 32),
            Err(Error::ExecutorResourceLimit { limit: 32, .. })
        ));
        let bytes = encode(&value, "test", 128).unwrap();
        assert_eq!(bytes, serde_json::to_vec(&value).unwrap());
    }

    #[test]
    fn oversized_metadata_does_not_escape_into_errors() {
        let error = encode(&"secret-do-not-log", "test", 1).unwrap_err();
        assert!(!error.to_string().contains("secret-do-not-log"));
    }

    #[test]
    fn capacity_never_exceeds_the_explicit_byte_limit() {
        let mut writer = Bounded {
            bytes: Vec::new(),
            limit: 65,
            exceeded: false,
        };
        writer.write_all(&[b'a'; 65]).unwrap();
        assert!(writer.bytes.capacity() <= 65);
        assert!(writer.write_all(b"b").is_err());
        assert_eq!(writer.bytes.len(), 65);
    }
}
