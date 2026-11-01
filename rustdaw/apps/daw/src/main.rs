//! Rust DAW entry point: starts the audio output (and input, if available), then runs the egui window.
use core_engine::{create_engine, share_engine};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gui::AudioInput;

/// Open the default input device and feed stereo frames into a lock-free ring.
/// The callback only converts and pushes: no allocation, no locks.
fn setup_input(host: &cpal::Host) -> Result<(cpal::Stream, AudioInput), Box<dyn std::error::Error>> {
    let dev = host.default_input_device().ok_or("no input device")?;
    let cfg = dev.default_input_config()?;
    let fmt = cfg.sample_format();
    let sample_rate = cfg.sample_rate().0 as f32;
    let ch = cfg.channels() as usize;
    let (mut tx, rx) = rtrb::RingBuffer::<[f32; 2]>::new(sample_rate as usize);
    let err = |e| eprintln!("input error: {e}");

    fn push<T: Copy>(tx: &mut rtrb::Producer<[f32; 2]>, data: &[T], ch: usize, conv: impl Fn(T) -> f32) {
        for frame in data.chunks(ch) {
            let l = conv(frame[0]);
            let r = if ch > 1 { conv(frame[1]) } else { l };
            let _ = tx.push([l, r]);
        }
    }

    let stream = match fmt {
        cpal::SampleFormat::F32 => dev.build_input_stream(&cfg.clone().into(), move |d: &[f32], _| push(&mut tx, d, ch, |x| x), err, None)?,
        cpal::SampleFormat::I16 => dev.build_input_stream(&cfg.clone().into(), move |d: &[i16], _| push(&mut tx, d, ch, |x| x as f32 / 32768.0), err, None)?,
        cpal::SampleFormat::U16 => dev.build_input_stream(&cfg.clone().into(), move |d: &[u16], _| push(&mut tx, d, ch, |x| (x as f32 - 32768.0) / 32768.0), err, None)?,
        other => return Err(format!("unsupported input format {other:?}").into()),
    };
    stream.play()?;
    println!("Input: {} @ {} Hz, {} ch", dev.name()?, sample_rate, ch);
    Ok((stream, AudioInput { rx, sample_rate }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no output device")?;
    let config = device.default_output_config()?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("unsupported sample format {:?} (f32 required)", config.sample_format()).into());
    }
    let sr = config.sample_rate().0 as f32;
    let channels = config.channels() as usize;
    println!("Output: {} @ {} Hz, {} ch", device.name()?, sr, channels);

    let (engine, mut handle) = create_engine(sr);
    // Shared so the exporter can render offline; the audio callback only ever try_locks.
    let engine = share_engine(engine, &mut handle);

    // Scratch buffer allocated now and moved into the callback; never resized in it.
    let max_frames = 8192;
    let mut stereo = vec![0.0f32; max_frames * 2];

    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _| {
            // Exporting holds the lock: output silence instead of ever blocking this thread.
            let Ok(mut engine) = engine.try_lock() else {
                data.fill(0.0);
                return;
            };
            for block in data.chunks_mut(max_frames * channels) {
                let frames = block.len() / channels;
                engine.process(&mut stereo[..frames * 2]);
                for (i, frame) in block.chunks_mut(channels).enumerate() {
                    for (c, s) in frame.iter_mut().enumerate() {
                        *s = stereo[2 * i + (c & 1)];
                    }
                }
            }
        },
        |err| eprintln!("audio error: {err}"),
        None,
    )?;
    stream.play()?;

    // Optional: recording needs an input device; the DAW still works without one.
    let (input_stream, input) = match setup_input(&host) {
        Ok((s, i)) => (Some(s), Some(i)),
        Err(e) => {
            eprintln!("Recording disabled: {e}");
            (None, None)
        }
    };

    // The streams must stay alive while the window is open.
    gui::run(handle, sr, input).map_err(|e| format!("GUI error: {e}"))?;
    drop(input_stream);
    drop(stream);
    Ok(())
}
