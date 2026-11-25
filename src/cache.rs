use crate::quota::Quota;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub quota: Quota,
    pub last_success_at: i64,
}

pub fn path() -> Option<PathBuf> {
    let root = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".cache"))
        })?;
    Some(root.join("opendeck-codex-limits/quota.json"))
}

impl Snapshot {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let snapshot: Self = serde_json::from_slice(&bytes)?;
        ensure!(snapshot.last_success_at > 0, "Invalid quota timestamp");
        snapshot.quota.validate()?;
        Ok(Some(snapshot))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let directory = path.parent().expect("quota cache has a parent directory");
        fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer(file.as_file_mut(), self)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }
}
