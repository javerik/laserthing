//! File access the UI needs, provided by the host application, so the UI itself never
//! touches the file system or a dialog library.

use laserline_core::formats;

/// The kinds of files the UI opens and saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// 3D model (STL, 3MF, OBJ, PLY, glTF, STEP; `laserline_core::formats`), open only.
    Model,
    /// Sliced scan (`.lscan`).
    Scan,
    /// Laser parameters as JSON (`.json`).
    Preset,
}

impl FileKind {
    /// Filter name and extensions for file dialogs (model extensions in both cases, for
    /// case-sensitive file systems).
    pub fn filter(self) -> (&'static str, Vec<String>) {
        match self {
            FileKind::Model => ("3D model", formats::EXTENSIONS.iter().flat_map(|e| [e.to_string(), e.to_ascii_uppercase()]).collect()),
            FileKind::Scan => ("Laser scan", vec!["lscan".into()]),
            FileKind::Preset => ("Laser preset", vec!["json".into()]),
        }
    }
}

/// A file the user picked.
#[derive(Clone, Debug)]
pub struct OpenedFile {
    /// File name, e.g. `part.stl`.
    pub name: String,
    /// Full path or URL, for messages.
    pub location: String,
    /// File content.
    pub bytes: Vec<u8>,
}

/// File dialogs and storage of the host application.
pub trait FileHost {
    /// Asks the user for a file of `kind`; `Ok(None)` if they cancelled.
    fn open(&mut self, kind: FileKind) -> Result<Option<OpenedFile>, String>;

    /// Asks the user where to save `bytes`, suggesting `file_name`. Returns where the
    /// file went, or `Ok(None)` if they cancelled.
    fn save(&mut self, kind: FileKind, file_name: &str, bytes: &[u8]) -> Result<Option<String>, String>;
}
