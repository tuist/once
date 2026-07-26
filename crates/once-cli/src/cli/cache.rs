use std::path::PathBuf;

use clap::{ArgGroup, Subcommand};
use once_cas::Digest;

#[derive(Subcommand)]
pub enum CacheCmd {
    /// Print blob and action counts plus on-disk size.
    Stats,

    /// Reclaim local cache space down to a size budget.
    ///
    /// Removes the oldest blobs and action results until the local
    /// store fits within `--max-size`. Safe to run at any time: an
    /// action whose outputs get evicted simply re-executes on its next
    /// build. Only the local tier is collected; a remote cache is never
    /// touched.
    Gc {
        /// Maximum local cache size to keep. Plain bytes or a human
        /// suffix: `500MB`, `2GiB`, `750k`. Decimal suffixes (KB/MB/GB/
        /// TB) are powers of 1000; binary suffixes (KiB/MiB/GiB/TiB) are
        /// powers of 1024.
        #[arg(long = "max-size", value_name = "SIZE", value_parser = parse_size)]
        max_size: u64,

        /// Report what would be reclaimed without deleting anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Read and write content-addressed blobs.
    #[command(arg_required_else_help = true)]
    Blob {
        #[command(subcommand)]
        cmd: Option<CacheBlobCmd>,
    },

    /// Read and write cached action results.
    #[command(arg_required_else_help = true)]
    Action {
        #[command(subcommand)]
        cmd: Option<CacheActionCmd>,
    },
}

#[derive(Subcommand)]
pub enum CacheBlobCmd {
    /// Store bytes from a file or stdin and print their BLAKE3 digest.
    Put {
        /// File to store. Use `-` or omit the path to read stdin.
        path: Option<PathBuf>,
    },

    /// Fetch blob bytes by content digest.
    Get {
        /// Blob digest to fetch.
        #[arg(value_parser = parse_digest)]
        digest: Digest,

        /// File to write. Defaults to stdout; use `-` for stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Check whether a blob exists. Exits 0 on hit, 1 on miss; with
    /// `--format json|toon`, always exits 0 and reports `present` in
    /// the structured output.
    Exists {
        /// Blob digest to probe.
        #[arg(value_parser = parse_digest)]
        digest: Digest,
    },
}

#[derive(Subcommand)]
pub enum CacheActionCmd {
    /// Fetch an action result.
    ///
    /// Identify the action either by passing its digest directly, or
    /// by declaring its inputs with `--input`; the same declaration
    /// must be used on `put` to write under the same key.
    #[command(group(
        ArgGroup::new("action_key")
            .required(true)
            .args(["action", "inputs"])
            .multiple(false)
    ))]
    Get {
        /// Pre-computed action digest.
        #[arg(value_parser = parse_digest)]
        action: Option<Digest>,

        /// Input spec (see `cache hash` for the grammar). Repeatable;
        /// inputs are hashed in order and combined into the action
        /// digest.
        #[arg(long = "input", value_name = "SPEC")]
        inputs: Vec<String>,

        /// Exit 0 only when there is a hit AND the recorded exit code
        /// is 0. On miss or on a cached failure, exit non-zero.
        #[arg(long)]
        if_success: bool,
    },

    /// Store an action result.
    ///
    /// Identify the action either by passing its digest directly, or
    /// by declaring its inputs with `--input`; the same declaration
    /// can be used on `get` to read back the result.
    #[command(group(
        ArgGroup::new("action_key")
            .required(true)
            .args(["action", "inputs"])
            .multiple(false)
    ))]
    Put {
        /// Pre-computed action digest.
        #[arg(value_parser = parse_digest)]
        action: Option<Digest>,

        /// Input spec (see `cache hash` for the grammar). Repeatable.
        #[arg(long = "input", value_name = "SPEC")]
        inputs: Vec<String>,

        /// Process exit code captured for the action. Defaults to 0
        /// since the common case is recording a success.
        #[arg(long, default_value_t = 0)]
        exit_code: i32,

        /// Optional blob digest containing captured stdout.
        #[arg(long, value_parser = parse_digest)]
        stdout: Option<Digest>,

        /// Optional blob digest containing captured stderr.
        #[arg(long, value_parser = parse_digest)]
        stderr: Option<Digest>,

        /// Declared output as `workspace/path=blob_digest`. Repeatable.
        #[arg(long = "output", value_parser = parse_output_digest)]
        outputs: Vec<(String, Digest)>,
    },

    /// Delete one cached action result.
    Forget {
        /// Action digest to remove.
        #[arg(value_parser = parse_digest)]
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

fn parse_digest(raw: &str) -> std::result::Result<Digest, String> {
    Digest::from_hex(raw).ok_or_else(|| "expected a 64-character lowercase BLAKE3 digest".into())
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

fn parse_output_digest(raw: &str) -> std::result::Result<(String, Digest), String> {
    let (path, digest) = raw
        .split_once('=')
        .ok_or_else(|| "expected workspace/path=blob_digest".to_string())?;
    if path.is_empty() {
        return Err("output path must not be empty".into());
    }
    Ok((path.to_string(), parse_digest(digest)?))
}

#[cfg(test)]
mod tests {
    use super::parse_size;

    #[test]
    fn parses_bare_bytes_and_b_suffix() {
        assert_eq!(parse_size("0").unwrap(), 0);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("512B").unwrap(), 512);
    }

    #[test]
    fn parses_decimal_and_binary_suffixes() {
        assert_eq!(parse_size("500MB").unwrap(), 500_000_000);
        assert_eq!(parse_size("2GiB").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1TB").unwrap(), 1_000_000_000_000);
    }

    #[test]
    fn is_case_insensitive_and_allows_a_space_before_the_unit() {
        assert_eq!(parse_size("750 mib").unwrap(), 750 * 1024 * 1024);
        assert_eq!(parse_size("  4gb ").unwrap(), 4_000_000_000);
    }

    #[test]
    fn short_binary_aliases_match_their_long_forms() {
        assert_eq!(parse_size("3g").unwrap(), parse_size("3GiB").unwrap());
        assert_eq!(parse_size("8k").unwrap(), parse_size("8KiB").unwrap());
    }

    #[test]
    fn rejects_garbage_and_unknown_units() {
        assert!(parse_size("").is_err());
        assert!(parse_size("MB").is_err());
        assert!(parse_size("12ZB").is_err());
        assert!(parse_size("abc").is_err());
    }

    #[test]
    fn rejects_overflow() {
        assert!(parse_size("99999999999999999999TiB").is_err());
    }
}
