use std::path::PathBuf;

use clap::Subcommand;
use fabrik_cas::Digest;

#[derive(Subcommand)]
pub enum CacheCmd {
    /// Print blob and action counts plus on-disk size.
    Stats,

    /// Compute a digest without storing bytes in the cache.
    ///
    /// With one path (or stdin), prints the digest of those bytes.
    /// With multiple paths, hashes each file and combines the digests
    /// in order, producing a single key suitable for `cache blob` ops.
    Hash {
        /// Combine already-computed digests into one ordered digest.
        #[arg(long)]
        combine: bool,

        /// File(s) to hash, or digests to combine with --combine. Use
        /// `-` or omit the path to hash stdin.
        inputs: Vec<String>,
    },

    /// Read and write blobs. Defaults to the content-addressed
    /// namespace; `put --key`, `get --key`, and `exists --key` operate
    /// on the keyed namespace, which is local-only and stores bytes
    /// under a caller-chosen digest.
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
    /// Store bytes from a file or stdin.
    ///
    /// Without `--key`, prints the BLAKE3 digest of the bytes (content
    /// addressing). With `--key`, stores the bytes under the
    /// caller-chosen digest so scripts can memoize artifacts under a
    /// key derived from their inputs.
    Put {
        /// Store under this digest instead of the content hash.
        #[arg(long, value_parser = parse_digest)]
        key: Option<Digest>,

        /// File to store. Use `-` or omit the path to read stdin.
        path: Option<PathBuf>,
    },

    /// Fetch blob bytes by digest. Defaults to the content-addressed
    /// namespace; pass `--key` to fetch from the keyed namespace
    /// instead. The two are kept separate so a keyed blob cannot be
    /// silently substituted for a content-addressed lookup.
    Get {
        /// Look up `digest` in the keyed namespace.
        #[arg(long)]
        key: bool,

        /// Blob digest (or key) to fetch.
        #[arg(value_parser = parse_digest)]
        digest: Digest,

        /// File to write. Defaults to stdout; use `-` for stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Check whether a blob exists. Exits 0 on hit, 1 on miss; with
    /// `--format json|toon`, always exits 0 and reports `present` in
    /// the structured output. Defaults to the content-addressed
    /// namespace; pass `--key` to probe the keyed namespace.
    Exists {
        /// Probe the keyed namespace instead of the content-addressed
        /// one.
        #[arg(long)]
        key: bool,

        /// Blob digest (or key) to probe.
        #[arg(value_parser = parse_digest)]
        digest: Digest,
    },
}

#[derive(Subcommand)]
pub enum CacheActionCmd {
    /// Fetch an action result by action digest.
    Get {
        /// Action digest to fetch.
        #[arg(value_parser = parse_digest)]
        action: Digest,
    },

    /// Store an action result for an action digest.
    Put {
        /// Action digest to store the result under.
        #[arg(value_parser = parse_digest)]
        action: Digest,

        /// Process exit code captured for the action.
        #[arg(long)]
        exit_code: i32,

        /// Blob digest containing captured stdout.
        #[arg(long, value_parser = parse_digest)]
        stdout: Digest,

        /// Blob digest containing captured stderr.
        #[arg(long, value_parser = parse_digest)]
        stderr: Digest,

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
            Self::Hash { .. } => vec!["hash"],
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

fn parse_output_digest(raw: &str) -> std::result::Result<(String, Digest), String> {
    let (path, digest) = raw
        .split_once('=')
        .ok_or_else(|| "expected workspace/path=blob_digest".to_string())?;
    if path.is_empty() {
        return Err("output path must not be empty".into());
    }
    Ok((path.to_string(), parse_digest(digest)?))
}
