use std::path::{Path, PathBuf};

pub(in super::super) fn root() -> Option<PathBuf> {
    std::env::var_os("SHUCKED_PROVIDER_ROOT")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.join("packs/manifest.json").is_file())
        .or_else(|| {
            let path = std::env::current_exe().ok()?.parent()?.join("providers");
            path.join("packs/manifest.json").is_file().then_some(path)
        })
        .or_else(|| {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tooling/providers");
            path.join("packs/manifest.json").is_file().then_some(path)
        })
}

pub(in super::super) fn shell(name: &str) -> Option<PathBuf> {
    root()
        .map(|path| path.join("runtime/bin").join(name))
        .filter(|path| path.is_file())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(name))
                    .find(|path| path.is_file())
            })
        })
        .or_else(|| {
            let path = Path::new("/bin").join(name);
            path.is_file().then_some(path)
        })
}
