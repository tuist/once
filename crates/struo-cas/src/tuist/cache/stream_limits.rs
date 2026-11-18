pub(super) const CODEC_MEMORY: u64 = 16 * 1024 * 1024;
const FRAMING_BYTES: u64 = 64 * 1024;

pub(super) const fn encoded_bytes(raw: u64) -> u64 {
    raw.saturating_add(raw / 64).saturating_add(FRAMING_BYTES)
}

pub(super) const fn staging_bytes(raw: u64) -> u64 {
    raw.saturating_add(encoded_bytes(raw))
}

pub(super) fn message_bytes(raw: u64, capacity: u64) -> u64 {
    raw.saturating_add(FRAMING_BYTES)
        .min(crate::budget::MAX_STREAM_MESSAGE_BYTES)
        .min(capacity.saturating_sub(CODEC_MEMORY))
        .max(1)
}

pub(super) fn memory_bytes(message: u64) -> u64 {
    CODEC_MEMORY
        .saturating_add(message)
        .max(crate::MIN_TRANSFER_MEMORY_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_memory_covers_decoder_and_codec_buffers() {
        let message = message_bytes(u64::MAX, crate::MIN_TRANSFER_MEMORY_BYTES);
        assert_eq!(memory_bytes(message), crate::MIN_TRANSFER_MEMORY_BYTES);
        assert_eq!(message + CODEC_MEMORY, crate::MIN_TRANSFER_MEMORY_BYTES);
    }

    #[test]
    fn staging_covers_raw_and_encoded_files_without_overflow() {
        assert!(staging_bytes(1024) > 2048);
        assert_eq!(staging_bytes(u64::MAX), u64::MAX);
    }
}
