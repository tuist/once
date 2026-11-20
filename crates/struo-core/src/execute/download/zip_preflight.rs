//! Bounds a ZIP archive's metadata before the decoder reads it.
//!
//! The decoder allocates for the central directory while it opens an
//! archive, before any per-entry check can run: it reserves one record per
//! declared entry, then reads every entry's name, extra field, and comment.
//! When the last end-of-central-directory record does not parse, it retries
//! with earlier signatures anywhere in the file. So the preflight:
//!
//! - validates the final end-of-central-directory record, and its ZIP64
//!   records when present, refusing multi-disk, prefixed, and inconsistent
//!   archives, and bounds the declared entry count and central-directory
//!   size by the archive entry and metadata limits;
//! - scans the whole file for every record the decoder could fall back to
//!   and refuses the archive if any declares more entries than the metadata
//!   budget covers, which bounds the decoder's up-front reservation;
//! - and caps the bytes the decoder may read while opening the archive, so
//!   allocations proportional to what it reads stay bounded across every
//!   fallback attempt.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::rc::Rc;

use crate::{Error, Executor, RemoteIoLimits, Result};

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const EOCD64_SIGNATURE: u32 = 0x0606_4b50;
const LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4b50;

const EOCD_BYTES: usize = 22;
const EOCD64_BYTES: usize = 56;
const LOCATOR_BYTES: usize = 20;
const CENTRAL_HEADER_BYTES: u64 = 46;
const MAX_COMMENT_BYTES: usize = 0xffff;

/// Memory budgeted for the decoder's in-memory record of one entry, kept
/// alongside the entry's name and extra field. The decoder's record is a few
/// hundred bytes; this rounds up.
const DECODER_ENTRY_BYTES: u64 = 512;

/// Room for the decoder's own reads beyond the central directory: the
/// backwards signature search, the trailing records and comment, and a
/// local header per entry, which the central-directory bound already
/// covers once more.
const DECODER_READ_SLACK_BYTES: u64 = 256 * 1024;

/// The end of a validated archive's metadata.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CentralDirectory {
    pub entries: u64,
    pub bytes: u64,
}

/// Validate `file`'s metadata against `limits` before it is decoded.
pub(super) fn preflight(file: &mut File, limits: RemoteIoLimits) -> Result<CentralDirectory> {
    let length = file.seek(SeekFrom::End(0)).map_err(io_error)?;
    let directory = final_directory(file, length)?;
    if directory.entries > limits.max_archive_entries as u64 {
        return Err(limit("archive entries", limits.max_archive_entries as u64));
    }
    if directory.bytes > limits.max_metadata_bytes {
        return Err(limit("archive metadata bytes", limits.max_metadata_bytes));
    }
    let cap = entry_cap(limits);
    if directory.entries > cap {
        return Err(limit("archive metadata bytes", limits.max_metadata_bytes));
    }
    scan_fallback_records(file, cap, limits)?;
    Ok(directory)
}

/// The most entries any record may declare: the archive entry limit, and
/// what the metadata budget covers at [`DECODER_ENTRY_BYTES`] each.
fn entry_cap(limits: RemoteIoLimits) -> u64 {
    (limits.max_archive_entries as u64).min(limits.max_metadata_bytes / DECODER_ENTRY_BYTES)
}

/// Open `file` with the decoder after [`preflight`] accepts it, reading at
/// most [`decoder_read_budget`] bytes until the archive is open.
pub(super) fn open(
    mut file: File,
    limits: RemoteIoLimits,
) -> Result<zip::ZipArchive<BudgetedReader<File>>> {
    preflight(&mut file, limits)?;
    let (reader, budget) = BudgetedReader::new(file, decoder_read_budget(limits));
    let archive = zip::ZipArchive::new(reader).map_err(|source| {
        if budget.exhausted() {
            limit("archive metadata bytes", limits.max_metadata_bytes)
        } else {
            Error::InvalidDownloadAndExtract {
                reason: format!("opening ZIP archive: {source}"),
            }
        }
    })?;
    budget.lift();
    Ok(archive)
}

/// Bytes the decoder may read while opening an archive that passed
/// [`preflight`]: its central directory and a local header per entry, both
/// within the metadata limit, plus fixed slack.
pub(super) fn decoder_read_budget(limits: RemoteIoLimits) -> u64 {
    limits
        .max_metadata_bytes
        .saturating_mul(2)
        .saturating_add(DECODER_READ_SLACK_BYTES)
}

fn final_directory(file: &mut File, length: u64) -> Result<CentralDirectory> {
    let tail_length = length.min((EOCD_BYTES + MAX_COMMENT_BYTES) as u64);
    let tail_start = length - tail_length;
    let tail = read_at(file, tail_start, tail_length)?;
    // The record whose comment ends exactly at the end of the file. Nothing
    // may follow the archive.
    let position = (0..tail.len().saturating_sub(EOCD_BYTES - 1))
        .rev()
        .find(|&at| {
            u32_at(&tail, at) == EOCD_SIGNATURE
                && at + EOCD_BYTES + usize::from(u16_at(&tail, at + 20)) == tail.len()
        })
        .ok_or_else(|| malformed("the end of central directory record is missing"))?;
    let eocd = &tail[position..];
    let eocd_offset = tail_start + position as u64;

    let disk = u16_at(eocd, 4);
    let directory_disk = u16_at(eocd, 6);
    let entries_here = u16_at(eocd, 8);
    let entries = u16_at(eocd, 10);
    let size = u32_at(eocd, 12);
    let offset = u32_at(eocd, 16);
    let zip64 = [disk, directory_disk, entries_here, entries].contains(&u16::MAX)
        || size == u32::MAX
        || offset == u32::MAX;

    let (entries, size, offset, end) = if zip64 {
        zip64_directory(file, eocd_offset)?
    } else {
        if disk != 0 || directory_disk != 0 || entries_here != entries {
            return Err(unsupported("multi-disk archives are not supported"));
        }
        (
            u64::from(entries),
            u64::from(size),
            u64::from(offset),
            eocd_offset,
        )
    };

    if offset.checked_add(size) != Some(end) {
        return Err(malformed(
            "the central directory does not end where the archive's trailing records begin",
        ));
    }
    if entries.saturating_mul(CENTRAL_HEADER_BYTES) > size {
        return Err(malformed(
            "the central directory is too small for its entry count",
        ));
    }
    if entries > 0 && u32_at(&read_at(file, offset, 4)?, 0) != CENTRAL_HEADER_SIGNATURE {
        return Err(unsupported(
            "archives with data before the first entry are not supported",
        ));
    }
    Ok(CentralDirectory {
        entries,
        bytes: size,
    })
}

/// The entry count, size, and offset of a ZIP64 central directory, and where
/// it must end.
fn zip64_directory(file: &mut File, eocd_offset: u64) -> Result<(u64, u64, u64, u64)> {
    let locator_offset = eocd_offset
        .checked_sub(LOCATOR_BYTES as u64)
        .ok_or_else(|| malformed("the ZIP64 locator is missing"))?;
    let locator = read_at(file, locator_offset, LOCATOR_BYTES as u64)?;
    if u32_at(&locator, 0) != LOCATOR_SIGNATURE {
        return Err(malformed("the ZIP64 locator is missing"));
    }
    if u32_at(&locator, 4) != 0 || u32_at(&locator, 16) > 1 {
        return Err(unsupported("multi-disk archives are not supported"));
    }
    let record_offset = u64_at(&locator, 8);
    if record_offset.saturating_add(EOCD64_BYTES as u64) > locator_offset {
        return Err(malformed("the ZIP64 end of central directory is misplaced"));
    }
    let record = read_at(file, record_offset, EOCD64_BYTES as u64)?;
    if u32_at(&record, 0) != EOCD64_SIGNATURE
        || u64_at(&record, 4).checked_add(12) != Some(locator_offset - record_offset)
    {
        return Err(malformed(
            "the ZIP64 end of central directory is inconsistent",
        ));
    }
    let entries_here = u64_at(&record, 24);
    let entries = u64_at(&record, 32);
    if u32_at(&record, 16) != 0 || u32_at(&record, 20) != 0 || entries_here != entries {
        return Err(unsupported("multi-disk archives are not supported"));
    }
    Ok((
        entries,
        u64_at(&record, 40),
        u64_at(&record, 48),
        record_offset,
    ))
}

/// Refuse the archive if any end-of-central-directory record in it, which
/// the decoder could retry with, declares more entries than `cap`. The
/// decoder reserves memory for a record's declared entries before reading
/// them.
fn scan_fallback_records(file: &mut File, cap: u64, limits: RemoteIoLimits) -> Result<()> {
    const CHUNK: usize = 64 * 1024;
    const CARRY: usize = EOCD64_BYTES;
    let over = || limit("archive metadata bytes", limits.max_metadata_bytes);
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut window = Vec::with_capacity(CHUNK + CARRY);
    let mut chunk = vec![0_u8; CHUNK];
    loop {
        let read = file.read(&mut chunk).map_err(io_error)?;
        window.extend_from_slice(&chunk[..read]);
        let at_end = read == 0;
        // Leave a full record's worth of bytes for the next window unless
        // the file has ended.
        let scan_end = if at_end {
            window.len()
        } else {
            window.len().saturating_sub(CARRY)
        };
        for at in 0..scan_end.min(window.len().saturating_sub(3)) {
            match u32_at(&window, at) {
                EOCD_SIGNATURE if at + EOCD_BYTES <= window.len() => {
                    let declared = u64::from(u16_at(&window, at + 8).max(u16_at(&window, at + 10)));
                    if declared > cap {
                        return Err(over());
                    }
                }
                EOCD64_SIGNATURE if at + EOCD64_BYTES <= window.len() => {
                    let declared = u64_at(&window, at + 24).max(u64_at(&window, at + 32));
                    if declared > cap {
                        return Err(over());
                    }
                }
                _ => {}
            }
        }
        if at_end {
            return Ok(());
        }
        window.drain(..scan_end);
    }
}

/// A reader that fails once it has served `budget` bytes, until the budget
/// is lifted. Wraps the archive while the decoder opens it.
pub(super) struct BudgetedReader<R> {
    inner: R,
    remaining: Rc<Cell<Option<u64>>>,
}

/// Lifts or inspects the budget of a [`BudgetedReader`] the decoder owns.
#[derive(Clone)]
pub(super) struct ReadBudget(Rc<Cell<Option<u64>>>);

impl ReadBudget {
    pub(super) fn lift(&self) {
        self.0.set(None);
    }

    pub(super) fn exhausted(&self) -> bool {
        self.0.get() == Some(0)
    }
}

impl<R> BudgetedReader<R> {
    pub(super) fn new(inner: R, budget: u64) -> (Self, ReadBudget) {
        let remaining = Rc::new(Cell::new(Some(budget)));
        (
            Self {
                inner,
                remaining: Rc::clone(&remaining),
            },
            ReadBudget(remaining),
        )
    }
}

impl<R: Read> Read for BudgetedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let Some(remaining) = self.remaining.get() else {
            return self.inner.read(buffer);
        };
        if remaining == 0 {
            return Err(io::Error::other(
                "the archive's metadata exceeds its read budget",
            ));
        }
        let allowed = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.remaining.set(Some(remaining - read as u64));
        Ok(read)
    }
}

impl<R: Seek> Seek for BudgetedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

pub(super) fn limit(resource: &'static str, limit: u64) -> Error {
    Error::ExecutorResourceLimit {
        executor: Executor::Local.name().to_string(),
        resource,
        limit,
    }
}

fn read_at(file: &mut File, offset: u64, length: u64) -> Result<Vec<u8>> {
    let length = usize::try_from(length).map_err(|_| malformed("a record is out of range"))?;
    let mut bytes = vec![0_u8; length];
    file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
    file.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            malformed("a record extends past the end of the archive")
        } else {
            io_error(error)
        }
    })?;
    Ok(bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

fn malformed(reason: &str) -> Error {
    Error::InvalidDownloadAndExtract {
        reason: format!("malformed ZIP archive: {reason}"),
    }
}

fn unsupported(reason: &str) -> Error {
    Error::InvalidDownloadAndExtract {
        reason: format!("unsupported ZIP archive: {reason}"),
    }
}

fn io_error(source: io::Error) -> Error {
    Error::FileAction {
        action: "download_and_extract",
        path: "downloaded archive".to_string(),
        source,
    }
}

#[cfg(test)]
mod tests;
