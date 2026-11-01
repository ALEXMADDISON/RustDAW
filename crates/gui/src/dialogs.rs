//! Native file dialogs (Windows). Other platforms use the text fields in the UI.
use std::path::PathBuf;

#[cfg(windows)]
pub fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

#[cfg(not(windows))]
pub fn pick_folder() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
pub fn pick_save(file_name: &str, ext: &str, dir: Option<PathBuf>) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().set_file_name(file_name).add_filter(ext, &[ext]);
    if let Some(dir) = dir {
        d = d.set_directory(dir);
    }
    d.save_file()
}

#[cfg(not(windows))]
pub fn pick_save(_file_name: &str, _ext: &str, _dir: Option<PathBuf>) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
pub fn pick_open(ext: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter(ext, &[ext]).pick_file()
}

#[cfg(not(windows))]
pub fn pick_open(_ext: &str) -> Option<PathBuf> {
    None
}

pub fn has_native_dialogs() -> bool {
    cfg!(windows)
}
