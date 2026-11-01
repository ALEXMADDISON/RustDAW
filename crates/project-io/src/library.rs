//! Sample-library folders: recursive scanning plus a background "watch folder" poller
//! that reports when files appear or disappear.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const AUDIO_EXTENSIONS: [&str; 5] = ["wav", "mp3", "flac", "ogg", "aiff"];

pub fn is_audio_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map_or(false, |e| AUDIO_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// All audio files below `dir` (sorted, bounded depth and count so a huge disk can't hang the app).
pub fn scan_folder(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        if depth > 8 || out.len() > 50_000 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, depth + 1, out);
            } else if is_audio_file(&p) {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, 0, &mut v);
    v.sort();
    v
}

/// The current file list of one watched folder.
pub struct ScanResult {
    pub folder: PathBuf,
    pub files: Vec<PathBuf>,
}

enum Cmd {
    SetFolders(Vec<PathBuf>),
}

/// Polls the watched folders every `interval` on a background thread and sends a
/// [`ScanResult`] whenever a folder's file list changed (and once after it is added).
pub struct FolderWatcher {
    tx: Sender<Cmd>,
    pub rx: Receiver<ScanResult>,
    _thread: JoinHandle<()>,
}

impl FolderWatcher {
    pub fn new(interval: Duration) -> Self {
        let (tx, cmd_rx) = channel::<Cmd>();
        let (res_tx, rx) = channel::<ScanResult>();
        let handle = thread::spawn(move || {
            let mut folders: Vec<PathBuf> = Vec::new();
            let mut last: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
            loop {
                match cmd_rx.recv_timeout(interval) {
                    Ok(Cmd::SetFolders(f)) => {
                        last.retain(|k, _| f.contains(k));
                        folders = f;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                for f in &folders {
                    let files = scan_folder(f);
                    if last.get(f) != Some(&files) {
                        last.insert(f.clone(), files.clone());
                        if res_tx.send(ScanResult { folder: f.clone(), files }).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Self { tx, rx, _thread: handle }
    }

    pub fn set_folders(&self, folders: Vec<PathBuf>) {
        let _ = self.tx.send(Cmd::SetFolders(folders));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_recursively_and_watcher_reports_adds_and_removals() {
        let root = std::env::temp_dir().join(format!("rustdaw_lib_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("drums/kicks")).unwrap();
        std::fs::write(root.join("a.wav"), b"x").unwrap();
        std::fs::write(root.join("drums/kicks/k.MP3"), b"x").unwrap();
        std::fs::write(root.join("notes.txt"), b"x").unwrap();
        assert_eq!(scan_folder(&root).len(), 2);

        let w = FolderWatcher::new(Duration::from_millis(100));
        w.set_folders(vec![root.clone()]);
        let first = w.rx.recv_timeout(Duration::from_secs(3)).expect("initial scan");
        assert_eq!(first.files.len(), 2);
        std::fs::write(root.join("drums/new.flac"), b"x").unwrap();
        let second = w.rx.recv_timeout(Duration::from_secs(3)).expect("add detected");
        assert_eq!(second.files.len(), 3);
        std::fs::remove_file(root.join("a.wav")).unwrap();
        let third = w.rx.recv_timeout(Duration::from_secs(3)).expect("removal detected");
        assert_eq!(third.files.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
