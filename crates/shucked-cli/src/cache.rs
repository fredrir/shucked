use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use etcetera::BaseStrategy;

use crate::discover::normalize_path;

pub(crate) fn resolve_cache_root(cwd: &Path, override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        return Ok(normalize_path(&resolved));
    }

    if let Some(explicit) = std::env::var_os("SHUCKED_CACHE_DIR")
        && !explicit.is_empty()
    {
        let path = PathBuf::from(explicit);
        let resolved = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        return Ok(normalize_path(&resolved));
    }

    default_cache_root()
}

#[cfg(not(target_arch = "wasm32"))]
fn default_cache_root() -> Result<PathBuf> {
    let strategy = etcetera::base_strategy::choose_base_strategy()
        .context("resolve the OS cache directory for shucked")?;
    Ok(strategy.cache_dir().join("shucked"))
}

#[cfg(target_arch = "wasm32")]
fn default_cache_root() -> Result<PathBuf> {
    anyhow::bail!("cache directory discovery is not supported on wasm32")
}
