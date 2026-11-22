use std::path::{Path, PathBuf};
use tauri::Manager;
use crate::connection::ConnectionError;

fn ordinary(path: &Path, directory: bool) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false; };
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if meta.file_attributes() & 0x400 != 0 { return false; } }
    !meta.file_type().is_symlink() && if directory { meta.is_dir() } else { meta.is_file() }
}
pub(crate) fn producer_in(root: &Path) -> Result<PathBuf, ConnectionError> {
    let parent=root.join("producer"); let file=parent.join("mindtrail-producer.exe");
    if !root.is_absolute() || !ordinary(root,true) || !ordinary(&parent,true) || !ordinary(&file,false) { return Err(ConnectionError::ProducerNotPackaged); }
    let root=root.canonicalize().map_err(|_|ConnectionError::PathUnavailable)?;
    let parent=parent.canonicalize().map_err(|_|ConnectionError::PathUnavailable)?;
    let file=file.canonicalize().map_err(|_|ConnectionError::PathUnavailable)?;
    if parent != root.join("producer") || file != parent.join("mindtrail-producer.exe") { return Err(ConnectionError::PathUnavailable); }
    Ok(file)
}
pub(crate) fn producer_path(app: &tauri::AppHandle) -> Result<PathBuf, ConnectionError> {
    producer_in(&app.path().resource_dir().map_err(|_|ConnectionError::PathUnavailable)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn connection_producer_requires_fixed_packaged_file() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap();
        let root=std::env::temp_dir().join(format!("mindtrail-dist-{random:?}"));
        std::fs::create_dir(&root).unwrap();
        assert!(producer_in(&root).is_err());
        std::fs::create_dir(root.join("producer")).unwrap();
        std::fs::write(root.join("producer/mindtrail-producer.exe"),b"fixture").unwrap();
        assert!(producer_in(&root).is_ok());
        std::fs::remove_file(root.join("producer/mindtrail-producer.exe")).unwrap();
        std::fs::remove_dir(root.join("producer")).unwrap(); std::fs::remove_dir(root).unwrap();
    }
}
