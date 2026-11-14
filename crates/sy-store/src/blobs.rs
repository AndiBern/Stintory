// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Content-addressed blob files: `<root>/<first two hex chars>/<hash>.parquet`.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use sy_model::BlobHash;

use crate::{Result, StoreError};

/// Directory of telemetry blobs, addressed by their SHA-256 hash.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Opens (and creates) the blob directory.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root })
    }

    /// Path of a blob file (whether it exists or not).
    pub fn path(&self, hash: &BlobHash) -> PathBuf {
        let h = hash.as_str();
        self.root.join(&h[..2]).join(format!("{h}.parquet"))
    }

    /// True if the blob file exists.
    pub fn contains(&self, hash: &BlobHash) -> bool {
        self.path(hash).is_file()
    }

    /// Stores bytes under their hash. Idempotent: an existing blob is kept.
    /// The file is written to a temporary name, synced and then renamed, so
    /// readers never see a partial file.
    pub fn put(&self, bytes: &[u8]) -> Result<BlobHash> {
        let hash = BlobHash::of(bytes);
        let target = self.path(&hash);
        if target.is_file() {
            return Ok(hash);
        }
        fs::create_dir_all(target.parent().expect("blob path has a parent"))?;
        let tmp =
            self.root
                .join("tmp")
                .join(format!("{}.{}.tmp", hash.as_str(), std::process::id()));
        {
            let mut file = File::create(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &target)?;
        Ok(hash)
    }

    /// Reads a blob and verifies its hash.
    pub fn get(&self, hash: &BlobHash) -> Result<Vec<u8>> {
        let bytes = match fs::read(self.path(hash)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::BlobMissing(hash.clone()));
            }
            Err(e) => return Err(e.into()),
        };
        if BlobHash::of(&bytes) != *hash {
            return Err(StoreError::BlobCorrupt(hash.clone()));
        }
        Ok(bytes)
    }

    /// Deletes a blob file (no error if it doesn't exist).
    pub fn remove(&self, hash: &BlobHash) -> Result<()> {
        match fs::remove_file(self.path(hash)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All blob files with their modification time (for cleaning up files
    /// nothing refers to). Other files in the directory are skipped.
    pub fn list(&self) -> Result<Vec<(BlobHash, std::time::SystemTime)>> {
        let mut out = Vec::new();
        for dir in fs::read_dir(&self.root)? {
            let dir = dir?;
            let name = dir.file_name();
            let name = name.to_string_lossy();
            if name.len() != 2 || !dir.file_type()?.is_dir() {
                continue;
            }
            for file in fs::read_dir(dir.path())? {
                let file = file?;
                let file_name = file.file_name();
                let Some(hash) = file_name
                    .to_str()
                    .and_then(|n| n.strip_suffix(".parquet"))
                    .and_then(|h| h.parse::<BlobHash>().ok())
                else {
                    continue;
                };
                if hash.as_str().starts_with(name.as_ref()) {
                    out.push((hash, file.metadata()?.modified()?));
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip_and_idempotence() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
        let hash = blobs.put(b"hello").unwrap();
        assert!(blobs.contains(&hash));
        assert_eq!(blobs.put(b"hello").unwrap(), hash);
        assert_eq!(blobs.get(&hash).unwrap(), b"hello");
        assert!(
            blobs
                .path(&hash)
                .starts_with(dir.path().join("blobs").join(&hash.as_str()[..2]))
        );
    }

    #[test]
    fn detects_missing_and_corrupt_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let missing = BlobHash::of(b"nope");
        assert!(matches!(
            blobs.get(&missing),
            Err(StoreError::BlobMissing(_))
        ));

        let hash = blobs.put(b"original").unwrap();
        fs::write(blobs.path(&hash), b"tampered").unwrap();
        assert!(matches!(blobs.get(&hash), Err(StoreError::BlobCorrupt(_))));
    }

    #[test]
    fn lists_blob_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::open(dir.path()).unwrap();
        let a = blobs.put(b"a").unwrap();
        let b = blobs.put(b"b").unwrap();
        fs::write(dir.path().join("tmp").join("x.tmp"), b"partial").unwrap();
        fs::write(dir.path().join("readme.txt"), b"other").unwrap();
        let mut listed: Vec<String> = blobs
            .list()
            .unwrap()
            .into_iter()
            .map(|(h, _)| h.as_str().to_owned())
            .collect();
        listed.sort();
        let mut expected = vec![a.as_str().to_owned(), b.as_str().to_owned()];
        expected.sort();
        assert_eq!(listed, expected);
    }
}
