use crate::host::*;
use crate::module::*;
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use vst3::{ComPtr, ComWrapper, Interface, Steinberg::Vst::*, Steinberg::*};

#[derive(Clone, Copy, Debug)]
pub enum EventKind {
    NoteOn { key: u8, velocity: u8 },
    NoteOff { key: u8 },
}

#[derive(Clone, Copy, Debug)]
pub struct MidiEvent {
    pub frame: u32,
    pub kind: EventKind,
}

/// Host transport information passed to the plugin each block.
#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub playing: bool,
    pub tempo: f64,
    /// Song position in quarter notes at the start of the block.
    pub quarters: f64,
}

const MAX_EVENTS: usize = 1024;

/// The real-time half of a plugin. `Send` so it can be moved into the audio thread.
pub struct PluginProcessor {
    processor: ComPtr<IAudioProcessor>,
    component: ComPtr<IComponent>,
    events: ComWrapper<EventList>,
    events_ptr: ComPtr<IEventList>,
    changes: ComWrapper<ParamChanges>,
    changes_ptr: ComPtr<IParameterChanges>,
    pending: Vec<(u32, f64)>,
    panic: bool,
    out_l: Box<[f32]>,
    out_r: Box<[f32]>,
    silence: Box<[f32]>,
    num_in_buses: usize,
    num_out_buses: usize,
    has_event_in: bool,
    sample_rate: f64,
    pub name: String,
    pub is_instrument: bool,
    _host: ComWrapper<HostApp>,
    _module: Arc<Module>,
}

// COM pointers are !Send, but the VST3 contract allows `process` on any single thread.
unsafe impl Send for PluginProcessor {}

impl PluginProcessor {
    /// Queue a normalized parameter change for the next block (no allocation).
    pub fn set_param(&mut self, id: u32, value: f64) {
        if let Some(p) = self.pending.iter_mut().find(|p| p.0 == id) {
            p.1 = value;
        } else if self.pending.len() < self.pending.capacity() {
            self.pending.push((id, value));
        }
    }

    /// Release all sounding notes on the next block.
    pub fn all_notes_off(&mut self) {
        self.panic = true;
    }

    /// Render an instrument; its output is ADDED to `out_l/out_r`.
    pub fn process_instrument(&mut self, events: &[MidiEvent], out_l: &mut [f32], out_r: &mut [f32], t: &Transport) {
        let n = out_l.len().min(self.out_l.len());
        if self.run(n, None, events, t) {
            for i in 0..n {
                out_l[i] += self.out_l[i];
                out_r[i] += self.out_r[i];
            }
        }
    }

    /// Process audio in place (insert effect).
    pub fn process_effect(&mut self, io_l: &mut [f32], io_r: &mut [f32], t: &Transport) {
        let n = io_l.len().min(self.out_l.len());
        let ptrs = [io_l.as_mut_ptr(), io_r.as_mut_ptr()];
        if self.run(n, Some(ptrs), &[], t) {
            io_l[..n].copy_from_slice(&self.out_l[..n]);
            io_r[..n].copy_from_slice(&self.out_r[..n]);
        }
    }

    fn run(&mut self, n: usize, input: Option<[*mut f32; 2]>, events: &[MidiEvent], t: &Transport) -> bool {
        if n == 0 {
            return false;
        }
        unsafe {
            // events
            {
                let mut list = self.events.events.borrow_mut();
                list.clear();
                let push = |frame: u32, kind: EventKind, list: &mut Vec<Event>| {
                    if list.len() >= MAX_EVENTS {
                        return;
                    }
                    let mut ev: Event = std::mem::zeroed();
                    ev.sampleOffset = frame as i32;
                    match kind {
                        EventKind::NoteOn { key, velocity } => {
                            ev.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
                            ev.__field0.noteOn = NoteOnEvent { channel: 0, pitch: key as i16, tuning: 0.0, velocity: velocity as f32 / 127.0, length: 0, noteId: key as i32 };
                        }
                        EventKind::NoteOff { key } => {
                            ev.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
                            ev.__field0.noteOff = NoteOffEvent { channel: 0, pitch: key as i16, velocity: 0.0, noteId: key as i32, tuning: 0.0 };
                        }
                    }
                    list.push(ev);
                };
                if self.panic {
                    self.panic = false;
                    for key in 0..128u8 {
                        push(0, EventKind::NoteOff { key }, &mut list);
                    }
                }
                if self.has_event_in {
                    for e in events {
                        push(e.frame.min(n as u32 - 1), e.kind, &mut list);
                    }
                }
            }
            // parameters
            let k = self.pending.len().min(MAX_PARAM_QUEUES);
            for (i, &(id, v)) in self.pending.iter().take(k).enumerate() {
                self.changes.queues[i].id.set(id);
                self.changes.queues[i].value.set(v);
            }
            self.changes.count.set(k);
            if self.pending.len() > k {
                self.pending.drain(..k);
            } else {
                self.pending.clear();
            }

            // process context
            let mut ctx: ProcessContext = std::mem::zeroed();
            ctx.state = (ProcessContext_::StatesAndFlags_::kTempoValid | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid | ProcessContext_::StatesAndFlags_::kTimeSigValid) as u32
                | if t.playing { ProcessContext_::StatesAndFlags_::kPlaying as u32 } else { 0 };
            ctx.sampleRate = self.sample_rate;
            ctx.tempo = t.tempo;
            ctx.projectTimeMusic = t.quarters;
            ctx.timeSigNumerator = 4;
            ctx.timeSigDenominator = 4;

            // buffers
            let mut in_ptrs: [*mut f32; 2] = match input {
                Some(p) => p,
                None => [self.silence.as_ptr() as *mut f32, self.silence.as_ptr() as *mut f32],
            };
            let mut out_ptrs: [*mut f32; 2] = [self.out_l.as_mut_ptr(), self.out_r.as_mut_ptr()];
            let mut in_buses: [AudioBusBuffers; 4] = std::mem::zeroed();
            let mut out_buses: [AudioBusBuffers; 4] = std::mem::zeroed();
            if self.num_in_buses > 0 {
                in_buses[0].numChannels = 2;
                in_buses[0].__field0.channelBuffers32 = in_ptrs.as_mut_ptr();
            }
            if self.num_out_buses > 0 {
                out_buses[0].numChannels = 2;
                out_buses[0].__field0.channelBuffers32 = out_ptrs.as_mut_ptr();
            }
            let mut data = ProcessData {
                processMode: ProcessModes_::kRealtime as i32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
                numSamples: n as i32,
                numInputs: self.num_in_buses.min(4) as i32,
                numOutputs: self.num_out_buses.min(4) as i32,
                inputs: if self.num_in_buses > 0 { in_buses.as_mut_ptr() } else { std::ptr::null_mut() },
                outputs: out_buses.as_mut_ptr(),
                inputParameterChanges: self.changes_ptr.as_ptr(),
                outputParameterChanges: std::ptr::null_mut(),
                inputEvents: self.events_ptr.as_ptr(),
                outputEvents: std::ptr::null_mut(),
                processContext: &mut ctx,
            };
            let r = self.processor.process(&mut data);
            if r != kResultOk && r != kResultTrue {
                return false;
            }
            true
        }
    }
}

impl Drop for PluginProcessor {
    fn drop(&mut self) {
        unsafe {
            let _ = self.processor.setProcessing(0);
            let _ = self.component.setActive(0);
            if let Some(base) = self.component.cast::<IPluginBase>() {
                let _ = base.terminate();
            }
        }
    }
}

fn create<I: Interface>(factory: &ComPtr<IPluginFactory>, cid: &[u8; 16]) -> Result<ComPtr<I>, String> {
    let tuid = tuid_from(cid);
    let mut obj: *mut c_void = std::ptr::null_mut();
    let r = unsafe { factory.createInstance(tuid.as_ptr(), I::IID.as_ptr() as *const _, &mut obj) };
    if r != kResultOk || obj.is_null() {
        return Err("createInstance failed".into());
    }
    unsafe { ComPtr::from_raw(obj as *mut I) }.ok_or_else(|| "null instance".to_string())
}

pub(crate) fn load(info: &PluginInfo, sample_rate: f64, max_block: usize, state: Option<&[u8]>) -> Result<(PluginProcessor, Option<PluginController>), String> {
    let module = Module::open(&info.path)?;
    let factory = module.factory.as_ref().ok_or("no factory")?;
    let host = ComWrapper::new(HostApp);
    let host_unknown = host.to_com_ptr::<FUnknown>().ok_or("host iface")?;

    let component: ComPtr<IComponent> = create(factory, &info.class_id)?;
    unsafe {
        let base = component.cast::<IPluginBase>().ok_or("no IPluginBase")?;
        if base.initialize(host_unknown.as_ptr()) != kResultOk {
            return Err("component initialize failed".into());
        }
    }
    let processor = component.cast::<IAudioProcessor>().ok_or("not an audio processor")?;

    // ---- controller ----
    let edits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let handler = ComWrapper::new(Handler { edits: edits.clone() });
    let handler_ptr = handler.to_com_ptr::<IComponentHandler>().ok_or("handler iface")?;
    let mut controller: Option<ComPtr<IEditController>> = None;
    let mut separate = false;
    unsafe {
        if let Some(c) = component.cast::<IEditController>() {
            controller = Some(c);
        } else {
            let mut cid: TUID = [0; 16];
            if component.getControllerClassId(&mut cid) == kResultOk {
                if let Ok(c) = create::<IEditController>(factory, &tuid_bytes(&cid)) {
                    if let Some(b) = c.cast::<IPluginBase>() {
                        let _ = b.initialize(host_unknown.as_ptr());
                    }
                    separate = true;
                    controller = Some(c);
                }
            }
        }
    }
    let mut connection = None;
    let controller = controller.map(|c| unsafe {
        let _ = c.setComponentHandler(handler_ptr.as_ptr());
        if separate {
            if let (Some(a), Some(b)) = (component.cast::<IConnectionPoint>(), c.cast::<IConnectionPoint>()) {
                let _ = a.connect(b.as_ptr());
                let _ = b.connect(a.as_ptr());
                connection = Some((a, b));
            }
        }
        c
    });

    // ---- saved state (sounds / presets): before the plug-in starts processing ----
    let mut state_applied = false;
    if let Some(bytes) = state {
        if let Some((comp, ctl)) = split_state(bytes) {
            unsafe {
                if !comp.is_empty() {
                    state_applied = feed_state(comp, |s| component.setState(s));
                    if let Some(c) = &controller {
                        let _ = feed_state(comp, |s| c.setComponentState(s));
                    }
                }
                if !ctl.is_empty() {
                    if let Some(c) = &controller {
                        let _ = feed_state(ctl, |s| c.setState(s));
                    }
                }
            }
        }
    }
    let component_for_ctrl = component.cast::<IComponent>();

    // ---- buses + setup ----
    let (num_in, num_out, has_event_in);
    unsafe {
        num_in = component.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32).max(0) as usize;
        num_out = component.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kOutput as i32).max(0) as usize;
        has_event_in = component.getBusCount(MediaTypes_::kEvent as i32, BusDirections_::kInput as i32) > 0;
        if num_out == 0 {
            return Err("plugin has no audio output".into());
        }
        // Ask for stereo on the main buses (extra buses, e.g. side-chains, stay as the plugin wants).
        let mut ins: Vec<SpeakerArrangement> = (0..num_in).map(|_| SpeakerArr::kStereo).collect();
        let mut outs: Vec<SpeakerArrangement> = (0..num_out).map(|_| SpeakerArr::kStereo).collect();
        let _ = processor.setBusArrangements(ins.as_mut_ptr(), num_in as i32, outs.as_mut_ptr(), num_out as i32);
        if num_in > 0 {
            let _ = component.activateBus(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32, 0, 1);
        }
        let _ = component.activateBus(MediaTypes_::kAudio as i32, BusDirections_::kOutput as i32, 0, 1);
        if has_event_in {
            let _ = component.activateBus(MediaTypes_::kEvent as i32, BusDirections_::kInput as i32, 0, 1);
        }
        let mut setup = ProcessSetup {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            maxSamplesPerBlock: max_block as i32,
            sampleRate: sample_rate,
        };
        if processor.setupProcessing(&mut setup) != kResultOk {
            return Err("setupProcessing failed".into());
        }
        if component.setActive(1) != kResultOk {
            return Err("setActive failed".into());
        }
        let _ = processor.setProcessing(1);
    }

    let events = ComWrapper::new(EventList { events: RefCell::new(Vec::with_capacity(MAX_EVENTS)) });
    let events_ptr = events.to_com_ptr::<IEventList>().ok_or("event iface")?;
    let changes = ComWrapper::new(ParamChanges::new());
    let changes_ptr = changes.to_com_ptr::<IParameterChanges>().ok_or("param iface")?;

    let proc = PluginProcessor {
        processor,
        component,
        events,
        events_ptr,
        changes,
        changes_ptr,
        pending: Vec::with_capacity(256),
        panic: false,
        out_l: vec![0.0; max_block].into_boxed_slice(),
        out_r: vec![0.0; max_block].into_boxed_slice(),
        silence: vec![0.0; max_block].into_boxed_slice(),
        num_in_buses: num_in,
        num_out_buses: num_out,
        has_event_in,
        sample_rate,
        name: info.name.clone(),
        is_instrument: info.is_instrument,
        _host: host.clone(),
        _module: module.clone(),
    };
    let ctrl = controller.map(|c| PluginController {
        params: read_params(&c),
        state_applied,
        component: component_for_ctrl,
        controller: c,
        separate,
        _handler: handler,
        _host: host,
        connection,
        edits,
        _module: module,
    });
    Ok((proc, ctrl))
}
