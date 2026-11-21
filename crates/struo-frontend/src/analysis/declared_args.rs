use std::ops::Deref;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use starlark::values::Value;

use super::transitive_set::TransitiveSets;
use super::DeclaredInputs;

/// Ordered command arguments, retaining shared sets and formatting until consumed.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeclaredArgs {
    entries: Vec<Entry>,
    #[serde(skip)]
    flattened: OnceLock<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum Entry {
    String(String),
    Fragment {
        args: DeclaredInputs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_each: Option<String>,
    },
}

impl DeclaredArgs {
    /// Add literal arguments before the existing fragments without expanding sets.
    pub fn prepend(&mut self, prefix: &[String]) {
        if prefix.is_empty() {
            return;
        }
        self.flattened.take();
        let mut entries = Vec::with_capacity(prefix.len() + self.entries.len());
        entries.extend(prefix.iter().cloned().map(Entry::String));
        entries.append(&mut self.entries);
        self.entries = entries;
    }

    pub(super) fn unpack(
        value: Value<'_>,
        format: Option<String>,
        before_each: Option<String>,
    ) -> Result<Self> {
        let args = DeclaredInputs::unpack(value, "cmd_args.args")?;
        if format
            .as_ref()
            .is_some_and(|format| format.matches("{}").count() != 1)
        {
            anyhow::bail!("expected `cmd_args.format` to contain exactly one `{{}}` placeholder");
        }
        Ok(Self {
            entries: vec![Entry::Fragment {
                args,
                format,
                before_each,
            }],
            flattened: OnceLock::new(),
        })
    }

    pub(super) fn push(&mut self, arg: String) {
        self.flattened.take();
        self.entries.push(Entry::String(arg));
    }

    pub(super) fn append(&mut self, args: Self) {
        self.flattened.take();
        self.entries.extend(args.entries);
    }

    pub fn resolve(&mut self, workspace: &Path) -> Result<()> {
        self.resolve_with(workspace, &TransitiveSets::default())
    }

    pub(super) fn resolve_with(
        &mut self,
        workspace: &Path,
        registry: &TransitiveSets,
    ) -> Result<()> {
        for entry in &mut self.entries {
            if let Entry::Fragment { args, .. } = entry {
                args.resolve_with(workspace, registry)?;
            }
        }
        Ok(())
    }

    /// Structural identity, independent of materialization and requiring no resolution.
    pub fn structural_digest(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"struo.declared_args.v1\0");
        hash.update((self.entries.len() as u64).to_le_bytes());
        for entry in &self.entries {
            match entry {
                Entry::String(arg) => {
                    hash.update([0]);
                    hash_string(&mut hash, arg);
                }
                Entry::Fragment {
                    args,
                    format,
                    before_each,
                } => {
                    hash.update([1]);
                    args.hash_structure(&mut hash);
                    for option in [format, before_each] {
                        hash.update([u8::from(option.is_some())]);
                        if let Some(value) = option {
                            hash_string(&mut hash, value);
                        }
                    }
                }
            }
        }
        hash.finalize().into()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|entry| match entry {
            Entry::String(_) => false,
            Entry::Fragment { args, .. } => args.is_empty(),
        })
    }

    pub fn len(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| match entry {
                Entry::String(_) => 1,
                Entry::Fragment {
                    args, before_each, ..
                } => args.len() * if before_each.is_some() { 2 } else { 1 },
            })
            .sum()
    }

    /// Iteration materializes a compatibility slice once. Hashing never uses it.
    pub fn iter(&self) -> std::slice::Iter<'_, String> {
        self.deref().iter()
    }

    /// Materialize arguments in execution order, preserving duplicates and empty values.
    pub fn to_vec(&self) -> Vec<String> {
        let mut flattened = Vec::new();
        for entry in &self.entries {
            match entry {
                Entry::String(arg) => flattened.push(arg.clone()),
                Entry::Fragment {
                    args,
                    format,
                    before_each,
                } => {
                    for arg in args {
                        if let Some(before_each) = before_each {
                            flattened.push(before_each.clone());
                        }
                        flattened.push(
                            format
                                .as_ref()
                                .map_or_else(|| arg.clone(), |format| format.replace("{}", arg)),
                        );
                    }
                }
            }
        }
        flattened
    }

    pub(super) fn validate_line_delimited(&self, path: &str) -> Result<()> {
        for entry in &self.entries {
            let invalid = match entry {
                Entry::String(arg) => has_newline(arg),
                Entry::Fragment {
                    args,
                    format,
                    before_each,
                } => {
                    !args.is_empty()
                        && (format.as_deref().is_some_and(has_newline)
                            || before_each.as_deref().is_some_and(has_newline)
                            || args.any_string(has_newline))
                }
            };
            if invalid {
                anyhow::bail!(
                    "line-delimited arg file `{path}` contains an argument with a newline"
                );
            }
        }
        Ok(())
    }
}

/// Slice-based compatibility operations materialize lazily, never during declaration.
impl Deref for DeclaredArgs {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        self.flattened.get_or_init(|| self.to_vec())
    }
}

impl Clone for DeclaredArgs {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            flattened: OnceLock::new(),
        }
    }
}

impl<'a> IntoIterator for &'a DeclaredArgs {
    type Item = &'a String;
    type IntoIter = std::slice::Iter<'a, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl From<Vec<String>> for DeclaredArgs {
    fn from(args: Vec<String>) -> Self {
        Self {
            entries: args.into_iter().map(Entry::String).collect(),
            flattened: OnceLock::new(),
        }
    }
}

impl FromIterator<String> for DeclaredArgs {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        Self::from(iter.into_iter().collect::<Vec<_>>())
    }
}

impl PartialEq for DeclaredArgs {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl Eq for DeclaredArgs {}

impl<T: AsRef<str>> PartialEq<Vec<T>> for DeclaredArgs {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.iter()
            .map(String::as_str)
            .eq(other.iter().map(AsRef::as_ref))
    }
}

impl<T: AsRef<str>, const N: usize> PartialEq<[T; N]> for DeclaredArgs {
    fn eq(&self, other: &[T; N]) -> bool {
        self.iter()
            .map(String::as_str)
            .eq(other.iter().map(AsRef::as_ref))
    }
}

fn hash_string(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value.as_bytes());
}

fn has_newline(value: &str) -> bool {
    value.contains(['\r', '\n'])
}

impl super::DeclaredAction {
    pub fn resolve_args(&mut self, workspace: &Path) -> Result<()> {
        let registry = TransitiveSets::default();
        self.argv.resolve_with(workspace, &registry)?;
        for file in &mut self.arg_files {
            file.args.resolve_with(workspace, &registry)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
