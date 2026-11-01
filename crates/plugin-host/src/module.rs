#[cfg(target_os = "linux")]
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use vst3::{ComPtr, Steinberg::*};

#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub name: String,
    pub vendor: String,
    pub path: PathBuf,
    pub class_id: [u8; 16],
    pub is_instrument: bool,
}

pub fn default_vst3_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("CommonProgramFiles") {
            v.push(PathBuf::from(p).join("VST3"));
        }
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            v.push(PathBuf::from(p).join("Programs").join("Common").join("VST3"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join(".vst3"));
        }
        v.push(PathBuf::from("/usr/lib/vst3"));
        v.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    #[cfg(target_os = "macos")]
    {
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    }
    v
}

/// Recursively find `*.vst3` files / bundles and read their class lists.
pub fn scan(dirs: &[PathBuf]) -> Vec<PluginInfo> {
    scan_with_errors(dirs).0
}

/// Like [`scan`], but also reports every plugin file that could not be read, with the reason,
/// so the UI can show why a plugin is missing.
pub fn scan_with_errors(dirs: &[PathBuf]) -> (Vec<PluginInfo>, Vec<(PathBuf, String)>) {
    // Many plugins (JUCE, Native Instruments, reFX ...) need COM on the thread that loads them.
    #[cfg(windows)]
    let com = unsafe {
        use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        let hr = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        hr >= 0 // S_OK / S_FALSE: initialised (or already) -> balance with CoUninitialize
    };
    let mut found = Vec::new();
    for d in dirs {
        collect(d, 0, &mut found);
    }
    found.sort();
    found.dedup();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for path in found {
        match classes(&path) {
            Ok(list) if list.is_empty() => errors.push((path, "no audio plugin classes in this file".to_string())),
            Ok(list) => out.extend(list),
            Err(e) => errors.push((path, e)),
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    #[cfg(windows)]
    if com {
        unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
    }
    (out, errors)
}

fn collect(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let is_vst3 = p.extension().map_or(false, |x| x.eq_ignore_ascii_case("vst3"));
        if is_vst3 {
            out.push(p); // file or bundle directory: do not descend into bundles
        } else if p.is_dir() && depth < 3 {
            collect(&p, depth + 1, out);
        }
    }
}

fn binary_path(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    let stem = p.file_stem()?.to_string_lossy().to_string();
    #[cfg(windows)]
    let rel = format!("Contents/x86_64-win/{stem}.vst3");
    #[cfg(target_os = "linux")]
    let rel = format!("Contents/x86_64-linux/{stem}.so");
    #[cfg(target_os = "macos")]
    let rel = format!("Contents/MacOS/{stem}");
    let b = p.join(&rel);
    if b.exists() {
        return Some(b);
    }
    // The binary is not always named like the bundle folder: take the only/first binary in the arch folder.
    #[cfg(any(windows, target_os = "linux"))]
    {
        let arch = if cfg!(windows) { "x86_64-win" } else { "x86_64-linux" };
        if let Ok(rd) = std::fs::read_dir(p.join("Contents").join(arch)) {
            for e in rd.flatten() {
                let f = e.path();
                let ext = f.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                if f.is_file() && (ext == "vst3" || ext == "so" || ext == "dll") {
                    return Some(f);
                }
            }
        }
    }
    None
}

/// A loaded shared library plus its plugin factory.
pub(crate) struct Module {
    pub factory: Option<ComPtr<IPluginFactory>>,
    lib: Option<libloading::Library>,
}

impl Module {
    pub fn open(path: &Path) -> Result<Arc<Module>, String> {
        let bin = binary_path(path).ok_or_else(|| format!("no VST3 binary in {}", path.display()))?;
        unsafe {
            #[cfg(unix)]
            let lib: libloading::Library = {
                let l = libloading::os::unix::Library::new(&bin).map_err(|e| e.to_string())?;
                let l: libloading::Library = l.into();
                l
            };
            // Windows: search the plugin's own folder for its DLL dependencies.
            #[cfg(windows)]
            let lib: libloading::Library = {
                let abs = std::path::absolute(&bin).unwrap_or_else(|_| bin.clone());
                match libloading::os::windows::Library::load_with_flags(&abs, libloading::os::windows::LOAD_WITH_ALTERED_SEARCH_PATH) {
                    Ok(l) => l.into(),
                    Err(_) => libloading::Library::new(&bin).map_err(|e| e.to_string())?,
                }
            };
            #[cfg(not(any(unix, windows)))]
            let lib = libloading::Library::new(&bin).map_err(|e| e.to_string())?;

            #[cfg(target_os = "linux")]
            if let Ok(entry) = lib.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"ModuleEntry\0") {
                if !entry(std::ptr::null_mut()) {
                    return Err("ModuleEntry failed".into());
                }
            }
            #[cfg(windows)]
            if let Ok(init) = lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") {
                if !init() {
                    return Err("InitDll failed".into());
                }
            }
            let get = lib.get::<unsafe extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory\0").map_err(|e| format!("not a VST3 module: {e}"))?;
            let raw = get();
            let factory = ComPtr::from_raw(raw).ok_or("GetPluginFactory returned null")?;
            Ok(Arc::new(Module { factory: Some(factory), lib: Some(lib) }))
        }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        self.factory = None; // release the factory before the library goes away
        if let Some(lib) = self.lib.take() {
            unsafe {
                #[cfg(target_os = "linux")]
                if let Ok(exit) = lib.get::<unsafe extern "C" fn() -> bool>(b"ModuleExit\0") {
                    exit();
                }
                #[cfg(windows)]
                if let Ok(exit) = lib.get::<unsafe extern "system" fn() -> bool>(b"ExitDll\0") {
                    exit();
                }
            }
            drop(lib);
        }
    }
}

pub(crate) fn cstr(a: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = a.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(crate) fn tuid_bytes(t: &TUID) -> [u8; 16] {
    let mut o = [0u8; 16];
    for (i, c) in t.iter().enumerate() {
        o[i] = *c as u8;
    }
    o
}

pub(crate) fn tuid_from(b: &[u8; 16]) -> TUID {
    let mut t: TUID = [0; 16];
    for (i, c) in b.iter().enumerate() {
        t[i] = *c as _;
    }
    t
}

fn classes(path: &Path) -> Result<Vec<PluginInfo>, String> {
    let module = Module::open(path)?;
    let factory = module.factory.as_ref().ok_or("no factory")?;
    let mut out = Vec::new();
    unsafe {
        let f2 = factory.cast::<IPluginFactory2>();
        let n = factory.countClasses();
        let mut vendor_default = String::new();
        {
            let mut fi: PFactoryInfo = std::mem::zeroed();
            if factory.getFactoryInfo(&mut fi) == kResultOk {
                vendor_default = cstr(&fi.vendor);
            }
        }
        for i in 0..n {
            let mut info: PClassInfo = std::mem::zeroed();
            if factory.getClassInfo(i, &mut info) != kResultOk {
                continue;
            }
            if cstr(&info.category) != "Audio Module Class" {
                continue;
            }
            let (mut sub, mut vendor) = (String::new(), vendor_default.clone());
            if let Some(f2) = &f2 {
                let mut i2: PClassInfo2 = std::mem::zeroed();
                if f2.getClassInfo2(i, &mut i2) == kResultOk {
                    sub = cstr(&i2.subCategories);
                    let v = cstr(&i2.vendor);
                    if !v.is_empty() {
                        vendor = v;
                    }
                }
            }
            out.push(PluginInfo { name: cstr(&info.name), vendor, path: path.to_path_buf(), class_id: tuid_bytes(&info.cid), is_instrument: sub.contains("Instrument") });
        }
    }
    Ok(out)
}
