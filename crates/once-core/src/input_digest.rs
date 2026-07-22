//! Streaming builder for action `input_digest` values.
//!
//! Plugins all need the same shape: a domain-separation prefix, a
//! deterministic concatenation of `(label, content_digest)` pairs, and
//! a small set of plugin-specific tail values (toolchain identity,
//! Apple SDK env, dep action digests). Open-coding it in every plugin
//! invites subtle drift: a missed sort, a forgotten NUL terminator, a
//! domain prefix that collides across two plugins. This builder is the
//! one canonical implementation.

use std::path::Path;

use once_cas::Digest;

use crate::directory_blob::capture_directory_blob;
use crate::OutputSymlinkMode;

#[derive(Debug)]
/// Build an input digest piece by piece.
///
/// The encoding is `domain || (label || 0 || digest_bytes || 0)*` plus
/// any caller-appended tail bytes. NUL terminators delimit fields so
/// that two distinct `(label, digest)` sequences can never collide
/// after concatenation. Sources should be added in sorted order; the
/// builder does not sort on the caller's behalf because some plugins
/// (the Rust compiler, for example) need to mix sources, dep digests,
/// and tail keys in a specific order that's part of their cache-key
/// contract.
#[must_use]
pub struct InputDigestBuilder {
    buf: Vec<u8>,
}

impl InputDigestBuilder {
    /// Begin a new digest with `domain` as the leading prefix. Bumping
    /// the domain (e.g. from `once.rust.input.v1` to `...v2`) is the
    /// portable way to invalidate an entire plugin's cache namespace
    /// when its encoding changes.
    pub fn new(domain: &[u8]) -> Self {
        let mut buf = Vec::with_capacity(domain.len() + 64);
        buf.extend_from_slice(domain);
        Self { buf }
    }

    /// Append a `(label, digest)` pair. Most plugins use this for
    /// source files (label = workspace path) and for dep records
    /// (label = `dep:<label>` or similar prefix).
    pub fn push_keyed(&mut self, label: &[u8], digest: &Digest) -> &mut Self {
        self.buf.extend_from_slice(label);
        self.buf.push(0);
        self.buf.extend_from_slice(digest.as_bytes());
        self.buf.push(0);
        self
    }

    /// Append arbitrary bytes (e.g. a toolchain identifier).
    pub fn push_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self.buf.push(0);
        self
    }

    /// Hash a workspace-relative source file or directory and append
    /// the result keyed by the workspace-relative path. Files stream
    /// through [`Digest::of_reader`]; directories use the same stable
    /// encoding as cached directory outputs.
    pub fn push_source<P: AsRef<Path>>(
        &mut self,
        workspace_root: P,
        ws_rel: &str,
    ) -> std::io::Result<&mut Self> {
        let abs = workspace_root.as_ref().join(ws_rel);
        let digest = if std::fs::metadata(&abs)?.is_dir() {
            let bytes = capture_directory_blob(&abs, OutputSymlinkMode::Preserve)?;
            Digest::of_bytes(&bytes)
        } else {
            let file = std::fs::File::open(&abs)?;
            Digest::of_reader(std::io::BufReader::new(file))?
        };
        Ok(self.push_keyed(ws_rel.as_bytes(), &digest))
    }

    /// Finalise the buffer into a [`Digest`].
    pub fn finish(self) -> Digest {
        Digest::of_bytes(&self.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn empty_digest_only_hashes_the_domain() {
        let a = InputDigestBuilder::new(b"d").finish();
        let b = InputDigestBuilder::new(b"d").finish();
        assert_eq!(a, b, "deterministic for the same domain");
        let c = InputDigestBuilder::new(b"d2").finish();
        assert_ne!(a, c, "domain bump partitions cache slots");
    }

    #[test]
    fn pushed_pairs_change_the_digest() {
        let one = {
            let mut b = InputDigestBuilder::new(b"d");
            b.push_keyed(b"path", &Digest::of_bytes(b"X"));
            b.finish()
        };
        let two = {
            let mut b = InputDigestBuilder::new(b"d");
            b.push_keyed(b"path", &Digest::of_bytes(b"Y"));
            b.finish()
        };
        assert_ne!(one, two);
    }

    #[test]
    fn push_source_hashes_file_contents() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
        std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() {}").unwrap();

        let first = {
            let mut b = InputDigestBuilder::new(b"test.input.v1");
            b.push_source(tmp.path(), "pkg/main.rs").unwrap();
            b.finish()
        };
        let second = {
            let mut b = InputDigestBuilder::new(b"test.input.v1");
            b.push_source(tmp.path(), "pkg/main.rs").unwrap();
            b.finish()
        };
        assert_eq!(first, second, "stable for identical content");

        std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() { /*!*/ }").unwrap();
        let third = {
            let mut b = InputDigestBuilder::new(b"test.input.v1");
            b.push_source(tmp.path(), "pkg/main.rs").unwrap();
            b.finish()
        };
        assert_ne!(first, third, "content change invalidates the digest");
    }

    #[test]
    fn push_source_hashes_directory_contents() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("Framework.framework");
        std::fs::create_dir_all(source.join("Modules")).unwrap();
        std::fs::write(source.join("Framework"), b"binary").unwrap();
        std::fs::write(
            source.join("Modules/module.modulemap"),
            b"module Framework {}",
        )
        .unwrap();

        let digest = |tmp: &TempDir| {
            let mut builder = InputDigestBuilder::new(b"test.input.v1");
            builder
                .push_source(tmp.path(), "Framework.framework")
                .unwrap();
            builder.finish()
        };
        let first = digest(&tmp);
        assert_eq!(first, digest(&tmp));

        std::fs::write(source.join("Framework"), b"changed").unwrap();
        assert_ne!(first, digest(&tmp));
    }

    #[test]
    fn push_source_distinguishes_an_empty_file_from_an_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let entry = tmp.path().join("entry");
        std::fs::write(&entry, []).unwrap();
        let file_digest = {
            let mut builder = InputDigestBuilder::new(b"test.input.v1");
            builder.push_source(tmp.path(), "entry").unwrap();
            builder.finish()
        };

        std::fs::remove_file(&entry).unwrap();
        std::fs::create_dir(&entry).unwrap();
        let directory_digest = {
            let mut builder = InputDigestBuilder::new(b"test.input.v1");
            builder.push_source(tmp.path(), "entry").unwrap();
            builder.finish()
        };

        assert_ne!(file_digest, directory_digest);
    }

    #[test]
    fn push_source_hashes_directory_entries_and_contents() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("tree/nested")).unwrap();
        std::fs::write(tmp.path().join("tree/nested/value.txt"), b"one").unwrap();

        let digest = || {
            let mut builder = InputDigestBuilder::new(b"test.directory.v1");
            builder.push_source(tmp.path(), "tree").unwrap();
            builder.finish()
        };
        let first = digest();
        let second = digest();
        assert_eq!(first, second);

        std::fs::write(tmp.path().join("tree/nested/value.txt"), b"two").unwrap();
        let changed_content = digest();
        assert_ne!(first, changed_content);

        std::fs::rename(
            tmp.path().join("tree/nested/value.txt"),
            tmp.path().join("tree/nested/renamed.txt"),
        )
        .unwrap();
        assert_ne!(changed_content, digest());
    }

    #[cfg(unix)]
    #[test]
    fn push_source_hashes_directory_symbolic_link_targets() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("tree")).unwrap();
        std::fs::write(tmp.path().join("tree/one.txt"), b"same").unwrap();
        std::fs::write(tmp.path().join("tree/two.txt"), b"same").unwrap();
        symlink("one.txt", tmp.path().join("tree/current.txt")).unwrap();

        let digest = || {
            let mut builder = InputDigestBuilder::new(b"test.directory.v1");
            builder.push_source(tmp.path(), "tree").unwrap();
            builder.finish()
        };
        let first = digest();

        std::fs::remove_file(tmp.path().join("tree/current.txt")).unwrap();
        symlink("two.txt", tmp.path().join("tree/current.txt")).unwrap();
        assert_ne!(first, digest());
    }

    #[test]
    fn push_source_propagates_io_errors() {
        let tmp = TempDir::new().unwrap();
        let mut b = InputDigestBuilder::new(b"d");
        let err = b.push_source(tmp.path(), "missing.rs").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
