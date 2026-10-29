//! File dialogs (rfd) and file system access for the UI.

use laserline_egui::{FileHost, FileKind, OpenedFile};
use std::path::Path;

pub struct NativeFiles;

/// Reads `path` for the UI.
pub fn read(path: &Path) -> Result<OpenedFile, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    Ok(OpenedFile { name, location: path.display().to_string(), bytes })
}

fn dialog(kind: FileKind) -> rfd::FileDialog {
    let (name, extensions) = kind.filter();
    rfd::FileDialog::new().add_filter(name, &extensions)
}

impl FileHost for NativeFiles {
    fn open(&mut self, kind: FileKind) -> Result<Option<OpenedFile>, String> {
        dialog(kind).pick_file().map(|path| read(&path)).transpose()
    }

    fn save(&mut self, kind: FileKind, file_name: &str, bytes: &[u8]) -> Result<Option<String>, String> {
        let Some(path) = dialog(kind).set_file_name(file_name).save_file() else {
            return Ok(None);
        };
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(path.display().to_string()))
    }
}
