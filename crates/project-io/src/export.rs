//! Write rendered audio as WAV (16/24-bit PCM or 32-bit float) or MP3.
use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, InterleavedPcm, Quality};
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Wav16,
    Wav24,
    WavFloat32,
    /// Constant bitrate in kbit/s (128, 192, 256 or 320).
    Mp3(u32),
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Mp3(_) => "mp3",
            _ => "wav",
        }
    }
    pub fn label(self) -> String {
        match self {
            ExportFormat::Wav16 => "WAV 16-bit".into(),
            ExportFormat::Wav24 => "WAV 24-bit".into(),
            ExportFormat::WavFloat32 => "WAV 32-bit float".into(),
            ExportFormat::Mp3(k) => format!("MP3 {k} kbps"),
        }
    }
}

/// `pcm` is interleaved stereo f32 in [-1, 1].
pub fn write_audio(path: &Path, pcm: &[f32], sample_rate: u32, format: ExportFormat) -> Result<(), Box<dyn Error>> {
    match format {
        ExportFormat::Mp3(kbps) => write_mp3(path, pcm, sample_rate, kbps),
        f => write_wav(path, pcm, sample_rate, f),
    }
}

fn write_wav(path: &Path, pcm: &[f32], sr: u32, format: ExportFormat) -> Result<(), Box<dyn Error>> {
    let (bits, float): (u16, bool) = match format {
        ExportFormat::Wav16 => (16, false),
        ExportFormat::Wav24 => (24, false),
        _ => (32, true),
    };
    let bytes_per = (bits / 8) as u32;
    let data_len = pcm.len() as u32 * bytes_per;
    let mut w = BufWriter::new(File::create(path)?);
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&(if float { 3u16 } else { 1u16 }).to_le_bytes())?;
    w.write_all(&2u16.to_le_bytes())?;
    w.write_all(&sr.to_le_bytes())?;
    w.write_all(&(sr * 2 * bytes_per).to_le_bytes())?;
    w.write_all(&(2 * bytes_per as u16).to_le_bytes())?;
    w.write_all(&bits.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    for &s in pcm {
        let s = s.clamp(-1.0, 1.0);
        match bits {
            16 => w.write_all(&((s * 32767.0).round() as i16).to_le_bytes())?,
            24 => {
                let v = (s * 8_388_607.0).round() as i32;
                w.write_all(&v.to_le_bytes()[..3])?;
            }
            _ => w.write_all(&s.to_le_bytes())?,
        }
    }
    w.flush()?;
    Ok(())
}

fn write_mp3(path: &Path, pcm: &[f32], sr: u32, kbps: u32) -> Result<(), Box<dyn Error>> {
    let mut b = Builder::new().ok_or("could not create the MP3 encoder")?;
    b.set_num_channels(2).map_err(|e| format!("mp3: {e:?}"))?;
    b.set_sample_rate(sr).map_err(|e| format!("mp3 sample rate: {e:?}"))?;
    b.set_brate(match kbps {
        0..=128 => Bitrate::Kbps128,
        129..=192 => Bitrate::Kbps192,
        193..=256 => Bitrate::Kbps256,
        _ => Bitrate::Kbps320,
    })
    .map_err(|e| format!("mp3 bitrate: {e:?}"))?;
    b.set_quality(Quality::Best).map_err(|e| format!("mp3 quality: {e:?}"))?;
    let mut enc = b.build().map_err(|e| format!("mp3 build: {e:?}"))?;

    let mut out: Vec<u8> = Vec::with_capacity(pcm.len() / 4);
    for chunk in pcm.chunks(2 * 16_384) {
        let ints: Vec<i16> = chunk.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect();
        // LAME's worst case output size: 1.25 * samples-per-channel + 7200 bytes.
        out.reserve(ints.len() / 2 * 5 / 4 + 7200);
        enc.encode_to_vec(InterleavedPcm(&ints), &mut out).map_err(|e| format!("mp3 encode: {e:?}"))?;
    }
    out.reserve(8192);
    enc.flush_to_vec::<FlushNoGap>(&mut out).map_err(|e| format!("mp3 flush: {e:?}"))?;
    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::load_sample;

    fn tone(secs: f32, sr: u32) -> Vec<f32> {
        (0..(secs * sr as f32) as usize).flat_map(|i| {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() * 0.5;
            [v, v]
        }).collect()
    }

    #[test]
    fn wav_round_trips_in_all_bit_depths() {
        for (fmt, tol) in [(ExportFormat::Wav16, 1e-3), (ExportFormat::Wav24, 1e-5), (ExportFormat::WavFloat32, 1e-6)] {
            let pcm = tone(0.5, 44_100);
            let p = std::env::temp_dir().join(format!("rustdaw_exp_{:?}.wav", fmt));
            write_audio(&p, &pcm, 44_100, fmt).unwrap();
            let s = load_sample(&p, 44_100.0).unwrap();
            assert_eq!(s.len(), pcm.len() / 2, "{fmt:?} length");
            let err = (0..s.len()).map(|i| (s.left[i] - pcm[2 * i]).abs()).fold(0.0f32, f32::max);
            assert!(err < tol, "{fmt:?} max error {err}");
        }
    }

    #[test]
    fn mp3_is_decodable_with_the_right_duration_and_pitch_energy() {
        let pcm = tone(2.0, 44_100);
        let p = std::env::temp_dir().join("rustdaw_exp.mp3");
        write_audio(&p, &pcm, 44_100, ExportFormat::Mp3(192)).unwrap();
        let size = std::fs::metadata(&p).unwrap().len();
        assert!(size > 20_000 && size < 80_000, "192 kbps for 2 s should be ~48 kB, got {size}");
        let s = load_sample(&p, 44_100.0).unwrap();
        let secs = s.len() as f32 / 44_100.0;
        assert!((secs - 2.0).abs() < 0.12, "decoded duration {secs}");
        let peak = s.left.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.3 && peak < 0.8, "decoded peak {peak}");
    }
}
