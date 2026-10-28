//! The record of the last build (build/manifest.json): for every source
//! file, the hashes that decide whether it needs rebuilding, and the hash and
//! size of its output. `pastel mod` takes its list of recolored files from it.
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Bump when transform code changes, to force a rebuild.
pub const CODE_VERSION: &str = "0.4";

/// File name of the manifest, in the build folder.
pub const MANIFEST_FILE: &str = "manifest.json";

/// What the last build did with one source file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Record {
    /// SHA-1 of the source file.
    #[serde(default)]
    pub src: Option<String>,
    /// Fingerprint of the code version and the category's settings (and
    /// statistics) the file was built with.
    #[serde(default)]
    pub cfg: Option<String>,
    /// The file's category.
    #[serde(default)]
    pub category: Option<String>,
    /// Hash of the recolored output; None when the file stays vanilla.
    #[serde(default)]
    pub out: Option<String>,
    /// Size of the output in bytes; None when there is no output.
    #[serde(default)]
    pub size: Option<u64>,
    /// What failed validation (empty = fine).
    #[serde(default)]
    pub problems: Vec<String>,
}

/// The record of a whole build.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    version: String,
    /// Every source file (relative to data/) and what the build did with it.
    pub files: BTreeMap<String, Record>,
}

impl Manifest {
    /// A manifest made by this version of the code.
    pub fn new(files: BTreeMap<String, Record>) -> Manifest {
        Manifest {
            version: CODE_VERSION.into(),
            files,
        }
    }

    /// The code version the build was made with.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Path of the manifest in a build folder.
    pub fn path(build_dir: &Path) -> PathBuf {
        build_dir.join(MANIFEST_FILE)
    }

    /// The record of the last build, if there was one.
    pub fn load(build_dir: &Path) -> Option<Manifest> {
        let text = fs::read_to_string(Self::path(build_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write the manifest into a build folder.
    pub fn save(&self, build_dir: &Path) -> Result<()> {
        fs::create_dir_all(build_dir)?;
        fs::write(Self::path(build_dir), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// SHA-1 of a file's contents, as lowercase hex.
pub fn sha1_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut h = sha1_smol::Sha1::new();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.digest().to_string())
}
