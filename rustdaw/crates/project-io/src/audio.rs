//! Audio file import (WAV, FLAC, MP3, OGG/Vorbis, ...) via Symphonia.
//! GUI/IO thread only: allocates and does file I/O.
use dsp::SampleData;
use std::error::Error;
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Longest clip we keep, in seconds (this is a sampler, not a streaming player).
pub const MAX_SECONDS: f32 = 60.0;

/// Length of an audio file in seconds. Reads the header only when the container states its length,
/// otherwise decodes the file (slow: call from a background thread).
pub fn probe_duration(path: &Path) -> Option<f32> {
    let header = (|| {
        let file = File::open(path).ok()?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).ok()?;
        let track = probed.format.default_track()?;
        let n = track.codec_params.n_frames?;
        let sr = track.codec_params.sample_rate?;
        Some(n as f32 / sr as f32)
    })();
    header.or_else(|| load_sample(path, 44_100.0).ok().map(|s| s.duration_secs()))
}

/// Decode `path` and resample it to `target_rate` so the audio thread can play
/// it back without any conversion.
pub fn load_sample(path: &Path, target_rate: f32) -> Result<SampleData, Box<dyn Error>> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("no audio track in file")?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let src_rate = params.sample_rate.ok_or("unknown sample rate")? as f32;
    let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;

    let max_frames = (MAX_SECONDS * src_rate) as usize;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    'outer: loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = *buf.spec();
                let ch = spec.channels.count().max(1);
                let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
                sb.copy_interleaved_ref(buf);
                for frame in sb.samples().chunks(ch) {
                    left.push(frame[0]);
                    right.push(if ch > 1 { frame[1] } else { frame[0] });
                    if left.len() >= max_frames {
                        break 'outer;
                    }
                }
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    if left.is_empty() {
        return Err("file contains no audio".into());
    }
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("sample").to_string();
    Ok(SampleData { name, sample_rate: src_rate, left, right }.resampled(target_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_bytes(sr: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut v = Vec::new();
        v.extend(b"RIFF");
        v.extend((36 + data_len).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes()); // PCM
        v.extend(1u16.to_le_bytes()); // mono
        v.extend(sr.to_le_bytes());
        v.extend((sr * 2).to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend(data_len.to_le_bytes());
        for s in samples {
            v.extend(s.to_le_bytes());
        }
        v
    }

    #[test]
    fn loads_and_resamples_wav() {
        let samples: Vec<i16> = (0..2205).map(|i| ((i as f32 * 0.1).sin() * 12000.0) as i16).collect();
        let path = std::env::temp_dir().join("rustdaw_test.wav");
        std::fs::write(&path, wav_bytes(22_050, &samples)).unwrap();
        let s = load_sample(&path, 44_100.0).unwrap();
        assert_eq!(s.sample_rate, 44_100.0);
        assert!((s.len() as i64 - 4410).abs() <= 2, "len {}", s.len());
        assert!(s.left.iter().any(|x| x.abs() > 0.1));
        assert_eq!(s.name, "rustdaw_test");
    }
}
