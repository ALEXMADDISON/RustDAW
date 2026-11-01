#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::cell::Cell;
use std::ffi::{c_char, c_void, CString};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{ptr, slice};

use vst3::{uid, Class, ComRef, ComWrapper, Steinberg::Vst::*, Steinberg::*};

fn copy_cstring(src: &str, dst: &mut [c_char]) {
    let c_string = CString::new(src).unwrap_or_else(|_| CString::default());
    let bytes = c_string.as_bytes_with_nul();

    for (src, dst) in bytes.iter().zip(dst.iter_mut()) {
        *dst = *src as c_char;
    }

    if bytes.len() > dst.len() {
        if let Some(last) = dst.last_mut() {
            *last = 0;
        }
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut len = 0;
    for (src, dst) in src.encode_utf16().zip(dst.iter_mut()) {
        *dst = src as TChar;
        len += 1;
    }

    if len < dst.len() {
        dst[len] = 0;
    } else if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

unsafe fn len_wstring(string: *const TChar) -> usize {
    let mut len = 0;

    while *string.offset(len) != 0 {
        len += 1;
    }

    len as usize
}

const PLUGIN_NAME: &'static str = "TestSynth";

struct GainProcessor {
    gain: AtomicU64,
    phase: Cell<f32>,
    freq: Cell<f32>,
    amp: Cell<f32>,
}

impl Class for GainProcessor {
    type Interfaces = (IComponent, IAudioProcessor, IProcessContextRequirements);
}

impl GainProcessor {
    const CID: TUID = uid(0x11111111, 0x22222222, 0x33333333, 0x44444444);

    fn new() -> GainProcessor {
        GainProcessor {
            gain: AtomicU64::new(1.0f64.to_bits()),
            phase: Cell::new(0.0),
            freq: Cell::new(0.0),
            amp: Cell::new(0.0),
        }
    }
}

impl IPluginBaseTrait for GainProcessor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for GainProcessor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        *class_id = GainController::CID;
        kResultOk
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, mediaType: MediaType, dir: BusDirection) -> i32 {
        match mediaType as BusDirections {
            MediaTypes_::kAudio => match dir as BusDirections {
                BusDirections_::kInput => 0,
                BusDirections_::kOutput => 1,
                _ => 0,
            },
            MediaTypes_::kEvent => match dir as BusDirections { BusDirections_::kInput => 1, _ => 0 },
            _ => 0,
        }
    }

    unsafe fn getBusInfo(
        &self,
        mediaType: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        match mediaType as MediaTypes {
            MediaTypes_::kAudio => match dir as BusDirections {
                BusDirections_::kInput => match index {
                    0 => {
                        let bus = &mut *bus;

                        bus.mediaType = MediaTypes_::kAudio as MediaType;
                        bus.direction = BusDirections_::kInput as BusDirection;
                        bus.channelCount = 2;
                        copy_wstring("Input", &mut bus.name);
                        bus.busType = BusTypes_::kMain as BusType;
                        bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;

                        kResultOk
                    }
                    _ => kInvalidArgument,
                },
                BusDirections_::kOutput => match index {
                    0 => {
                        let bus = &mut *bus;

                        bus.mediaType = MediaTypes_::kAudio as MediaType;
                        bus.direction = BusDirections_::kOutput as BusDirection;
                        bus.channelCount = 2;
                        copy_wstring("Output", &mut bus.name);
                        bus.busType = BusTypes_::kMain as BusType;
                        bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32 as uint32;

                        kResultOk
                    }
                    _ => kInvalidArgument,
                },
                _ => kInvalidArgument,
            },
            MediaTypes_::kEvent => {
                if index != 0 { return kInvalidArgument; }
                let bus = &mut *bus;
                bus.mediaType = MediaTypes_::kEvent as MediaType;
                bus.direction = BusDirections_::kInput as BusDirection;
                bus.channelCount = 1;
                copy_wstring("MIDI in", &mut bus.name);
                bus.busType = BusTypes_::kMain as BusType;
                bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32 as uint32;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getRoutingInfo(
        &self,
        _in_info: *mut RoutingInfo,
        _out_info: *mut RoutingInfo,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        _media_type: MediaType,
        _dir: BusDirection,
        _index: i32,
        _state: TBool,
    ) -> tresult {
        kResultOk
    }

    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }
}

impl IAudioProcessorTrait for GainProcessor {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        let _ = inputs;
        if num_ins != 0 || num_outs != 1 {
            return kResultFalse;
        }

        if *outputs != SpeakerArr::kStereo {
            return kResultFalse;
        }

        kResultTrue
    }

    unsafe fn getBusArrangement(
        &self,
        dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        match dir as BusDirections {
            BusDirections_::kInput => {
                if index == 0 {
                    *arr = SpeakerArr::kStereo;
                    kResultOk
                } else {
                    kInvalidArgument
                }
            }
            BusDirections_::kOutput => {
                if index == 0 {
                    *arr = SpeakerArr::kStereo;
                    kResultOk
                } else {
                    kInvalidArgument
                }
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn canProcessSampleSize(&self, symbolic_sample_size: i32) -> tresult {
        match symbolic_sample_size as SymbolicSampleSizes {
            SymbolicSampleSizes_::kSample32 => kResultOk as i32,
            SymbolicSampleSizes_::kSample64 => kNotImplemented as i32,
            _ => kInvalidArgument,
        }
    }

    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }

    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let process_data = &*data;

        if let Some(param_changes) = ComRef::from_raw(process_data.inputParameterChanges) {
            let param_count = param_changes.getParameterCount();
            for param_index in 0..param_count {
                if let Some(param_queue) =
                    ComRef::from_raw(param_changes.getParameterData(param_index))
                {
                    let param_id = param_queue.getParameterId();
                    let point_count = param_queue.getPointCount();

                    match param_id {
                        0 => {
                            let mut sample_offset = 0;
                            let mut value = 0.0;
                            let result = param_queue.getPoint(
                                point_count - 1,
                                &mut sample_offset,
                                &mut value,
                            );

                            if result == kResultTrue {
                                self.gain.store(value.to_bits(), Ordering::Relaxed);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let gain = f64::from_bits(self.gain.load(Ordering::Relaxed)) as f32;
        let n = process_data.numSamples as usize;
        if process_data.numOutputs != 1 {
            return kResultOk;
        }
        let outb = slice::from_raw_parts(process_data.outputs, 1);
        let chans = slice::from_raw_parts(outb[0].__field0.channelBuffers32, 2);
        let l = slice::from_raw_parts_mut(chans[0], n);
        let r = slice::from_raw_parts_mut(chans[1], n);

        // collect events sorted by offset (host sends them sorted)
        let mut evs: Vec<(i32, bool, i16)> = Vec::new();
        if let Some(list) = ComRef::from_raw(process_data.inputEvents) {
            for i in 0..list.getEventCount() {
                let mut e: Event = std::mem::zeroed();
                if list.getEvent(i, &mut e) == kResultOk {
                    if e.r#type == Event_::EventTypes_::kNoteOnEvent as u16 {
                        evs.push((e.sampleOffset, true, e.__field0.noteOn.pitch));
                    } else if e.r#type == Event_::EventTypes_::kNoteOffEvent as u16 {
                        evs.push((e.sampleOffset, false, e.__field0.noteOff.pitch));
                    }
                }
            }
        }
        let mut ei = 0;
        for i in 0..n {
            while ei < evs.len() && evs[ei].0 as usize <= i {
                let (_, on, pitch) = evs[ei];
                if on {
                    self.freq.set(440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0) / 48000.0);
                    self.amp.set(0.4);
                } else {
                    self.amp.set(0.0);
                }
                ei += 1;
            }
            let ph = self.phase.get();
            let v = (ph * std::f32::consts::TAU).sin() * self.amp.get() * gain;
            l[i] = v;
            r[i] = v;
            let mut np = ph + self.freq.get();
            if np >= 1.0 { np -= 1.0; }
            self.phase.set(np);
        }
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}

impl IProcessContextRequirementsTrait for GainProcessor {
    unsafe fn getProcessContextRequirements(&self) -> u32 {
        0
    }
}

struct GainController {
    gain: Cell<f64>,
}

impl Class for GainController {
    type Interfaces = (IEditController,);
}

impl GainController {
    const CID: TUID = uid(0x55555555, 0x66666666, 0x77777777, 0x88888888);

    fn new() -> GainController {
        GainController {
            gain: Cell::new(1.0),
        }
    }
}

impl IPluginBaseTrait for GainController {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IEditControllerTrait for GainController {
    unsafe fn setComponentState(&self, _state: *mut IBStream) -> tresult {
        kNotImplemented
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getParameterCount(&self) -> i32 {
        1
    }

    unsafe fn getParameterInfo(&self, param_index: i32, info: *mut ParameterInfo) -> tresult {
        match param_index {
            0 => {
                let info = &mut *info;

                info.id = 0;
                copy_wstring("Gain", &mut info.title);
                copy_wstring("Gain", &mut info.shortTitle);
                copy_wstring("", &mut info.units);
                info.stepCount = 0;
                info.defaultNormalizedValue = 1.0;
                info.unitId = 0;
                info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate as i32;

                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getParamStringByValue(
        &self,
        id: u32,
        value_normalized: f64,
        string: *mut String128,
    ) -> tresult {
        let slice = unsafe { &mut *string };

        match id {
            0 => {
                let display = value_normalized.to_string();
                copy_wstring(&display, slice);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn getParamValueByString(
        &self,
        id: u32,
        string: *mut TChar,
        value_normalized: *mut f64,
    ) -> tresult {
        match id {
            0 => {
                let len = len_wstring(string as *const TChar);
                if let Ok(string) =
                    String::from_utf16(slice::from_raw_parts(string as *const u16, len))
                {
                    if let Ok(value) = f64::from_str(&string) {
                        *value_normalized = value;
                        return kResultOk;
                    }
                }
                kInvalidArgument
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn normalizedParamToPlain(&self, id: u32, value_normalized: f64) -> f64 {
        match id {
            0 => value_normalized,
            _ => 0.0,
        }
    }

    unsafe fn plainParamToNormalized(&self, id: u32, plain_value: f64) -> f64 {
        match id {
            0 => plain_value,
            _ => 0.0,
        }
    }

    unsafe fn getParamNormalized(&self, id: u32) -> f64 {
        match id {
            0 => self.gain.get(),
            _ => 0.0,
        }
    }

    unsafe fn setParamNormalized(&self, id: u32, value: f64) -> tresult {
        match id {
            0 => {
                self.gain.set(value);
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }

    unsafe fn createView(&self, _name: *const c_char) -> *mut IPlugView {
        ptr::null_mut()
    }
}

struct Factory {}

impl Class for Factory {
    type Interfaces = (IPluginFactory,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let info = &mut *info;

        copy_cstring("Vendor", &mut info.vendor);
        copy_cstring("https://example.com", &mut info.url);
        copy_cstring("someone@example.com", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;

        kResultOk
    }

    unsafe fn countClasses(&self) -> i32 {
        2
    }

    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        match index {
            0 => {
                let info = &mut *info;
                info.cid = GainProcessor::CID;
                info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
                copy_cstring("Audio Module Class", &mut info.category);
                copy_cstring(PLUGIN_NAME, &mut info.name);

                kResultOk
            }
            1 => {
                let info = &mut *info;
                info.cid = GainController::CID;
                info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
                copy_cstring("Component Controller Class", &mut info.category);
                copy_cstring(PLUGIN_NAME, &mut info.name);

                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        let instance = match *(cid as *const TUID) {
            GainProcessor::CID => Some(
                ComWrapper::new(GainProcessor::new())
                    .to_com_ptr::<FUnknown>()
                    .unwrap(),
            ),
            GainController::CID => Some(
                ComWrapper::new(GainController::new())
                    .to_com_ptr::<FUnknown>()
                    .unwrap(),
            ),
            _ => None,
        };

        if let Some(instance) = instance {
            let ptr = instance.as_ptr();
            ((*(*ptr).vtbl).queryInterface)(ptr, iid as *mut TUID, obj)
        } else {
            kInvalidArgument
        }
    }
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[no_mangle]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleEntry(_bundle_ref: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[no_mangle]
extern "system" fn BundleExit() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleEntry(_library_handle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[no_mangle]
extern "system" fn ModuleExit() -> bool {
    true
}

#[no_mangle]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory {})
        .to_com_ptr::<IPluginFactory>()
        .unwrap()
        .into_raw()
}
