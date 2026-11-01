//! Host-side COM objects handed to plugins, plus the GUI-thread `PluginController`.
use crate::module::{cstr, Module};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use vst3::{Class, ComPtr, ComWrapper, Steinberg::Vst::*, Steinberg::*};

pub(crate) fn utf16_to_string(s: &[TChar]) -> String {
    let v: Vec<u16> = s.iter().take_while(|&&c| c != 0).map(|&c| c as u16).collect();
    String::from_utf16_lossy(&v)
}

pub(crate) fn string_to_utf16(src: &str, dst: &mut [TChar]) {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s as TChar;
        n += 1;
    }
    if n < dst.len() {
        dst[n] = 0;
    } else if let Some(l) = dst.last_mut() {
        *l = 0;
    }
}

// ---- IHostApplication ----
pub(crate) struct HostApp;
impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}
impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        string_to_utf16("Rust DAW", &mut *name);
        kResultOk
    }
    unsafe fn createInstance(&self, _cid: *mut TUID, _iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        if !obj.is_null() {
            *obj = std::ptr::null_mut();
        }
        kNotImplemented
    }
}

// ---- IBStream (in-memory stream for saving / restoring plug-in state) ----
pub(crate) struct MemStream {
    pub data: RefCell<Vec<u8>>,
    pos: Cell<usize>,
}
impl MemStream {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data: RefCell::new(data), pos: Cell::new(0) }
    }
}
impl Class for MemStream {
    type Interfaces = (IBStream,);
}
impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: i32, num_bytes_read: *mut i32) -> tresult {
        let data = self.data.borrow();
        let pos = self.pos.get();
        let n = (num_bytes.max(0) as usize).min(data.len().saturating_sub(pos));
        if n > 0 && !buffer.is_null() {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(pos), buffer as *mut u8, n);
        }
        self.pos.set(pos + n);
        if !num_bytes_read.is_null() {
            *num_bytes_read = n as i32;
        }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, num_bytes: i32, num_bytes_written: *mut i32) -> tresult {
        let n = num_bytes.max(0) as usize;
        let mut data = self.data.borrow_mut();
        let pos = self.pos.get();
        if data.len() < pos + n {
            data.resize(pos + n, 0);
        }
        if n > 0 && !buffer.is_null() {
            std::ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(pos), n);
        }
        self.pos.set(pos + n);
        if !num_bytes_written.is_null() {
            *num_bytes_written = n as i32;
        }
        kResultOk
    }
    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let len = self.data.borrow().len() as i64;
        // 0 = from start, 1 = from current position, 2 = from end
        let base: i64 = match mode {
            0 => 0,
            1 => self.pos.get() as i64,
            2 => len,
            _ => return kInvalidArgument,
        };
        let np = base + pos;
        if np < 0 {
            return kInvalidArgument;
        }
        self.pos.set(np as usize);
        if !result.is_null() {
            *result = np;
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if !pos.is_null() {
            *pos = self.pos.get() as i64;
        }
        kResultOk
    }
}

/// Let `f` write a plug-in's state into a fresh stream and return the bytes.
pub(crate) fn capture_state(f: impl FnOnce(*mut IBStream) -> tresult) -> Option<Vec<u8>> {
    let stream = ComWrapper::new(MemStream::new(Vec::new()));
    let ptr = stream.to_com_ptr::<IBStream>()?;
    let r = f(ptr.as_ptr());
    if r != kResultOk && r != kResultTrue {
        return None;
    }
    let bytes = stream.data.take();
    Some(bytes)
}

/// Hand `bytes` to a plug-in through a fresh read stream; true when the plug-in accepted them.
pub(crate) fn feed_state(bytes: &[u8], f: impl FnOnce(*mut IBStream) -> tresult) -> bool {
    let stream = ComWrapper::new(MemStream::new(bytes.to_vec()));
    let Some(ptr) = stream.to_com_ptr::<IBStream>() else { return false };
    let r = f(ptr.as_ptr());
    r == kResultOk || r == kResultTrue
}

const STATE_MAGIC: &[u8; 4] = b"RDS1";

/// Container for the two VST3 states: component (audio processor) and edit controller.
pub(crate) fn join_state(comp: &[u8], ctl: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + comp.len() + ctl.len());
    v.extend_from_slice(STATE_MAGIC);
    v.extend_from_slice(&(comp.len() as u32).to_le_bytes());
    v.extend_from_slice(comp);
    v.extend_from_slice(&(ctl.len() as u32).to_le_bytes());
    v.extend_from_slice(ctl);
    v
}

pub(crate) fn split_state(b: &[u8]) -> Option<(&[u8], &[u8])> {
    if b.len() < 12 || &b[..4] != STATE_MAGIC {
        return None;
    }
    let cl = u32::from_le_bytes(b.get(4..8)?.try_into().ok()?) as usize;
    let rest = b.get(8..)?;
    let comp = rest.get(..cl)?;
    let rest = rest.get(cl..)?;
    let tl = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let ctl = rest.get(4..4usize.checked_add(tl)?)?;
    Some((comp, ctl))
}

// ---- IComponentHandler (plugin -> host edit notifications) ----
/// Parameter edits made in the plugin's own window are queued here; the GUI forwards them to the
/// audio processor (see `PluginController::take_edits`).
pub(crate) struct Handler {
    pub edits: Arc<Mutex<Vec<(u32, f64)>>>,
}
impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}
impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        if let Ok(mut q) = self.edits.lock() {
            if let Some(e) = q.iter_mut().find(|e| e.0 == id) {
                e.1 = value;
            } else if q.len() < 4096 {
                q.push((id, value));
            }
        }
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, _flags: i32) -> tresult {
        kResultOk
    }
}

// ---- IEventList (host -> plugin MIDI) ----
pub(crate) struct EventList {
    pub events: RefCell<Vec<Event>>,
}
impl Class for EventList {
    type Interfaces = (IEventList,);
}
impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> i32 {
        self.events.borrow().len() as i32
    }
    unsafe fn getEvent(&self, index: i32, e: *mut Event) -> tresult {
        match self.events.borrow().get(index as usize) {
            Some(ev) => {
                std::ptr::copy_nonoverlapping(ev as *const Event, e, 1);
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn addEvent(&self, _e: *mut Event) -> tresult {
        kResultFalse
    }
}

// ---- IParameterChanges (host -> plugin parameter automation) ----
pub(crate) struct ParamQueue {
    pub id: Cell<u32>,
    pub value: Cell<f64>,
}
impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}
impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }
    unsafe fn getPointCount(&self) -> i32 {
        1
    }
    unsafe fn getPoint(&self, index: i32, sample_offset: *mut i32, value: *mut ParamValue) -> tresult {
        if index == 0 {
            *sample_offset = 0;
            *value = self.value.get();
            kResultTrue
        } else {
            kResultFalse
        }
    }
    unsafe fn addPoint(&self, _sample_offset: i32, _value: ParamValue, _index: *mut i32) -> tresult {
        kResultFalse
    }
}

pub(crate) const MAX_PARAM_QUEUES: usize = 64;

pub(crate) struct ParamChanges {
    pub queues: Vec<ComWrapper<ParamQueue>>,
    ptrs: Vec<ComPtr<IParamValueQueue>>,
    pub count: Cell<usize>,
}

impl ParamChanges {
    pub fn new() -> Self {
        let queues: Vec<ComWrapper<ParamQueue>> = (0..MAX_PARAM_QUEUES).map(|_| ComWrapper::new(ParamQueue { id: Cell::new(0), value: Cell::new(0.0) })).collect();
        let ptrs = queues.iter().map(|q| q.to_com_ptr::<IParamValueQueue>().unwrap()).collect();
        Self { queues, ptrs, count: Cell::new(0) }
    }
}
impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}
impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> i32 {
        self.count.get() as i32
    }
    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        match self.ptrs.get(index as usize) {
            Some(p) if (index as usize) < self.count.get() => p.as_ptr(),
            _ => std::ptr::null_mut(),
        }
    }
    unsafe fn addParameterData(&self, _id: *const ParamID, _index: *mut i32) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
}

// ---- GUI-thread side ----

#[derive(Clone, Debug)]
pub struct ParamDesc {
    pub id: u32,
    pub name: String,
    pub units: String,
    /// 0 = continuous, 1 = on/off toggle, n = n+1 discrete steps.
    pub steps: i32,
    pub default: f64,
}

/// Parameter access for one plugin; lives on the GUI thread.
pub struct PluginController {
    pub params: Vec<ParamDesc>,
    /// True when a saved state was handed to the plug-in at load time and it accepted it.
    pub state_applied: bool,
    pub(crate) component: Option<ComPtr<IComponent>>,
    pub(crate) controller: ComPtr<IEditController>,
    pub(crate) separate: bool,
    pub(crate) _handler: ComWrapper<Handler>,
    pub(crate) _host: ComWrapper<HostApp>,
    pub(crate) connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    pub(crate) edits: Arc<Mutex<Vec<(u32, f64)>>>,
    pub(crate) _module: Arc<Module>,
}

impl PluginController {
    /// Open the plugin's own editor window (GUI thread). Errors if the plugin has none or the
    /// platform does not host plugin windows yet.
    pub fn create_editor(&self, title: &str) -> Result<crate::editor::PluginEditor, String> {
        #[cfg(windows)]
        {
            crate::editor::PluginEditor::open(&self.controller, self._module.clone(), title)
        }
        #[cfg(not(windows))]
        {
            let _ = title;
            Err("Plugin windows are only hosted on Windows for now".into())
        }
    }

    /// Parameter changes made in the plugin's own window since the last call.
    /// Forward them to the audio processor (`Command::SetPluginParam`).
    pub fn take_edits(&self) -> Vec<(u32, f64)> {
        match self.edits.lock() {
            Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
            _ => Vec::new(),
        }
    }

    /// Snapshot of everything the plug-in saves for a project: sounds, presets, internal settings
    /// (VST3 `getState` of the component and of the controller). GUI thread.
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let comp = match &self.component {
            Some(c) => capture_state(|s| unsafe { c.getState(s) }),
            None => None,
        };
        let ctl = capture_state(|s| unsafe { self.controller.getState(s) });
        if comp.is_none() && ctl.is_none() {
            return None;
        }
        Some(join_state(&comp.unwrap_or_default(), &ctl.unwrap_or_default()))
    }

    pub fn get(&self, id: u32) -> f64 {
        unsafe { self.controller.getParamNormalized(id) }
    }

    /// Update the controller's own copy (UI). Also send the change to the processor.
    pub fn set(&self, id: u32, value: f64) {
        unsafe {
            self.controller.setParamNormalized(id, value.clamp(0.0, 1.0));
        }
    }

    pub fn display(&self, id: u32, normalized: f64) -> String {
        unsafe {
            let mut s: String128 = [0; 128];
            if self.controller.getParamStringByValue(id, normalized, &mut s) == kResultOk {
                let t = utf16_to_string(&s);
                if !t.is_empty() {
                    return t;
                }
            }
        }
        format!("{:.2}", normalized)
    }
}

impl Drop for PluginController {
    fn drop(&mut self) {
        unsafe {
            if let Some((a, b)) = self.connection.take() {
                let _ = a.disconnect(b.as_ptr());
                let _ = b.disconnect(a.as_ptr());
            }
            if self.separate {
                if let Some(base) = self.controller.cast::<IPluginBase>() {
                    let _ = base.terminate();
                }
            }
        }
    }
}

pub(crate) fn read_params(controller: &ComPtr<IEditController>) -> Vec<ParamDesc> {
    let mut out = Vec::new();
    unsafe {
        let n = controller.getParameterCount();
        for i in 0..n {
            let mut info: ParameterInfo = std::mem::zeroed();
            if controller.getParameterInfo(i, &mut info) != kResultOk {
                continue;
            }
            let flags = info.flags;
            if flags & ParameterInfo_::ParameterFlags_::kIsHidden != 0 {
                continue;
            }
            out.push(ParamDesc { id: info.id, name: utf16_to_string(&info.title), units: utf16_to_string(&info.units), steps: info.stepCount, default: info.defaultNormalizedValue });
        }
    }
    let _ = cstr;
    out
}
