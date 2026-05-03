use anyhow::{Result, anyhow, bail};
use std::fmt::Display;
use std::path::Path;

pub(crate) fn with_wasmtime_context<T>(
    result: std::result::Result<T, wasmtime::Error>,
    context: impl Display,
) -> Result<T> {
    result.map_err(|error| anyhow!("{context}: {error:?}"))
}

pub(crate) fn ensure_path(path: &Path, label: &str) -> Result<()> {
    if !path.exists() {
        bail!("{label} not found at {}", path.display());
    }
    Ok(())
}
