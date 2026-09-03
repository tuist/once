use std::path::PathBuf;
use std::str::FromStr;

use once_cas::Digest;
use usage::Subcommands;

/// Default size cap used when `--max-size` is not passed. 20 GB is
/// large enough that a healthy dev loop does not hit it, small enough
/// that a runaway build cannot fill a 256 GB SSD before the user
/// notices.
pub(crate) const DEFAULT_CACHE_SIZE_CAP_BYTES: u64 = 20 * 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheSize(u64);

impl CacheSize {
    pub(crate) const fn bytes(self) -> u64 {
        self.0
    }
}

impl FromStr for CacheSize {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        parse_size(raw).map(Self)
    }
}

#[derive(Debug)]
pub(crate) struct OutputDigest(String, Digest);

impl OutputDigest {
    pub(crate) fn into_inner(self) -> (String, Digest) {
        (self.0, self.1)
    }
}

impl FromStr for OutputDigest {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (path, digest) = raw
            .split_once('=')
            .ok_or_else(|| "expected workspace/path=blob_digest".to_string())?;
        if path.is_empty() {
            return Err("output path must not be empty".into());
        }
        Ok(Self(path.to_string(), digest.parse()?))
    }
}

#[derive(Subcommands)]
pub enum CacheCmd {
    /// Print blob and action counts plus on-disk size.
    Stats,

    /// Reclaim local cache space down to a size budget.
    ///
    /// Removes the oldest blobs and action results until the local
    /// store fits within `--max-size`, defaulting to 20 GB when the
    /// flag is omitted. Safe to run at any time: an action whose
    /// outputs get evicted simply re-executes on its next build. Only
    /// the local tier is collected; a remote cache is never touched.
    Gc {
        /// Maximum local cache size to keep. Plain bytes or a human
        /// suffix: `500MB`, `2GiB`, `750k`. Decimal suffixes (KB/MB/GB/
        /// TB) are powers of 1000; binary suffixes (KiB/MiB/GiB/TiB) are
        /// powers of 1024. Defaults to 20 GB when omitted.
        #[usage(long = "max-size", value_name = "SIZE")]
        max_size: Option<CacheSize>,

        /// Report what would be reclaimed without deleting anything.
        #[usage(long)]
        dry_run: bool,
    },

    /// Read and write content-addressed blobs.
    Blob {
        #[usage(subcommand)]
        cmd: Option<CacheBlobCmd>,
    },

    /// Read and write cached action results.
    Action {
        #[usage(subcommand)]
        cmd: Option<CacheActionCmd>,
    },
}

#[derive(Subcommands)]
pub enum CacheBlobCmd {
    /// Store bytes from a file or stdin and print their BLAKE3 digest.
    Put {
        /// File to store. Use `-` or omit the path to read stdin.
        path: Option<PathBuf>,
    },

    /// Fetch blob bytes by content digest.
    Get {
        /// Blob digest to fetch.
        digest: Digest,

        /// File to write. Defaults to stdout; use `-` for stdout.
        #[usage(short, long)]
        output: Option<PathBuf>,
    },

    /// Check whether a blob exists. Exits 0 on hit, 1 on miss; with
    /// `--format json|toon`, always exits 0 and reports `present` in
    /// the structured output.
    Exists {
        /// Blob digest to probe.
        digest: Digest,
    },
}

#[derive(Subcommands)]
pub enum CacheActionCmd {
    /// Fetch an action result.
    ///
    /// Identify the action either by passing its digest directly, or
    /// by declaring its inputs with `--input`; the same declaration
    /// must be used on `put` to write under the same key.
    Get {
        /// Pre-computed action digest.
        #[usage(conflicts = "inputs")]
        action: Option<Digest>,

        /// Input spec (see `cache hash` for the grammar). Repeatable;
        /// inputs are hashed in order and combined into the action
        /// digest.
        #[usage(long = "input", value_name = "SPEC", conflicts = "action")]
        inputs: Vec<String>,

        /// Exit 0 only when there is a hit AND the recorded exit code
        /// is 0. On miss or on a cached failure, exit non-zero.
        #[usage(long)]
        if_success: bool,
    },

    /// Store an action result.
    ///
    /// Identify the action either by passing its digest directly, or
    /// by declaring its inputs with `--input`; the same declaration
    /// can be used on `get` to read back the result.
    Put {
        /// Pre-computed action digest.
        #[usage(conflicts = "inputs")]
        action: Option<Digest>,

        /// Input spec (see `cache hash` for the grammar). Repeatable.
        #[usage(long = "input", value_name = "SPEC", conflicts = "action")]
        inputs: Vec<String>,

        /// Process exit code captured for the action. Defaults to 0
        /// since the common case is recording a success.
        #[usage(long, default = "0")]
        exit_code: i32,

        /// Optional blob digest containing captured stdout.
        #[usage(long)]
        stdout: Option<Digest>,

        /// Optional blob digest containing captured stderr.
        #[usage(long)]
        stderr: Option<Digest>,

        /// Declared output as `workspace/path=blob_digest`. Repeatable.
        #[usage(long = "output")]
        outputs: Vec<OutputDigest>,
    },

    /// Delete one cached action result.
    Forget {
        /// Action digest to remove.
        action: Digest,
    },
}

impl CacheCmd {
    pub(super) fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Stats => vec!["stats"],
            Self::Gc { .. } => vec!["gc"],
            Self::Blob { cmd } => {
                let mut path = vec!["blob"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Action { cmd } => {
                let mut path = vec!["action"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
        }
    }
}

impl CacheBlobCmd {
    fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Put { .. } => vec!["put"],
            Self::Get { .. } => vec!["get"],
            Self::Exists { .. } => vec!["exists"],
        }
    }
}

impl CacheActionCmd {
    fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Get { .. } => vec!["get"],
            Self::Put { .. } => vec!["put"],
            Self::Forget { .. } => vec!["forget"],
        }
    }
}

/// Parse a byte size with an optional decimal (KB/MB/GB/TB, powers of
/// 1000) or binary (KiB/MiB/GiB/TiB, powers of 1024) suffix. A bare
/// number, or a `B` suffix, is bytes. Case-insensitive; surrounding
/// whitespace and a space before the unit are allowed.
fn parse_size(raw: &str) -> std::result::Result<u64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("expected a size such as `500MB`, `2GiB`, or a byte count".into());
    }
    let digits_end = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(digits_end);
    if number.is_empty() {
        return Err(format!("`{raw}` must start with a number"));
    }
    let value: u64 = number
        .parse()
        .map_err(|_| format!("`{number}` is not a valid whole number of units"))?;
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1_u64,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        "kib" | "k" => 1_024,
        "mib" | "m" => 1_024 * 1_024,
        "gib" | "g" => 1_024 * 1_024 * 1_024,
        "tib" | "t" => 1_024 * 1_024 * 1_024 * 1_024,
        other => return Err(format!("unknown size unit `{other}`")),
    };
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("`{raw}` overflows a 64-bit byte count"))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::CacheSize;

    #[test]
    fn parses_bare_bytes_and_b_suffix() {
        assert_eq!(CacheSize::from_str("0").unwrap().bytes(), 0);
        assert_eq!(CacheSize::from_str("1024").unwrap().bytes(), 1024);
        assert_eq!(CacheSize::from_str("512B").unwrap().bytes(), 512);
    }

    #[test]
    fn parses_decimal_and_binary_suffixes() {
        assert_eq!(CacheSize::from_str("500MB").unwrap().bytes(), 500_000_000);
        assert_eq!(
            CacheSize::from_str("2GiB").unwrap().bytes(),
            2 * 1024 * 1024 * 1024
        );
        assert_eq!(
            CacheSize::from_str("1TB").unwrap().bytes(),
            1_000_000_000_000
        );
    }

    #[test]
    fn is_case_insensitive_and_allows_a_space_before_the_unit() {
        assert_eq!(
            CacheSize::from_str("750 mib").unwrap().bytes(),
            750 * 1024 * 1024
        );
        assert_eq!(
            CacheSize::from_str("  4gb ").unwrap().bytes(),
            4_000_000_000
        );
    }

    #[test]
    fn short_binary_aliases_match_their_long_forms() {
        assert_eq!(
            CacheSize::from_str("3g").unwrap().bytes(),
            CacheSize::from_str("3GiB").unwrap().bytes()
        );
        assert_eq!(
            CacheSize::from_str("8k").unwrap().bytes(),
            CacheSize::from_str("8KiB").unwrap().bytes()
        );
    }

    #[test]
    fn rejects_garbage_and_unknown_units() {
        assert!(CacheSize::from_str("").is_err());
        assert!(CacheSize::from_str("MB").is_err());
        assert!(CacheSize::from_str("12ZB").is_err());
        assert!(CacheSize::from_str("abc").is_err());
    }

    #[test]
    fn rejects_overflow() {
        assert!(CacheSize::from_str("99999999999999999999TiB").is_err());
    }
}
