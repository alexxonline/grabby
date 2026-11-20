//! 16-bit PCM helpers: mono mixdown, channel conversion, resampling and mixing.

pub fn bytes_to_i16(data: &[u8]) -> Vec<i16> {
    data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
}

pub fn i16_to_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// Mixes interleaved frames to mono: the 0.5/0.5 average for stereo, the integer average
/// of every channel otherwise.
pub fn to_mono(samples: &[i16], channels: usize) -> Vec<i16> {
    match channels {
        0 | 1 => samples.to_vec(),
        2 => samples.chunks_exact(2).map(|f| (f64::from(f[0]) * 0.5 + f64::from(f[1]) * 0.5).floor() as i16).collect(),
        n => samples
            .chunks_exact(n)
            .map(|f| (f.iter().map(|&s| i64::from(s)).sum::<i64>().div_euclid(n as i64)) as i16)
            .collect(),
    }
}

/// Converts between channel counts: mono is duplicated, multichannel is averaged, and other
/// layouts are bridged through mono.
pub fn convert_channels(samples: &[i16], from: usize, to: usize) -> Vec<i16> {
    if from == to {
        return samples.to_vec();
    }
    let mono = if from == 1 { samples.to_vec() } else { to_mono(samples, from) };
    if to == 1 {
        return mono;
    }
    mono.iter().flat_map(|&s| std::iter::repeat_n(s, to)).collect()
}

/// A streaming linear-interpolation resampler for interleaved PCM16 whose state carries
/// across calls.
pub struct Resampler {
    channels: usize,
    step: f64,
    /// Position of the next output frame, in input frames; -1 is the previous call's last frame.
    t: f64,
    prev: Option<Vec<i16>>,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        Resampler { channels: channels.max(1), step: f64::from(in_rate) / f64::from(out_rate.max(1)), t: 0.0, prev: None }
    }

    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        let ch = self.channels;
        let n = input.len() / ch;
        if n == 0 {
            return Vec::new();
        }
        if (self.step - 1.0).abs() < f64::EPSILON {
            return input[..n * ch].to_vec();
        }
        let frame = |i: isize, c: usize| -> f64 {
            if i < 0 {
                self.prev.as_ref().map_or(f64::from(input[c]), |p| f64::from(p[c]))
            } else {
                f64::from(input[i as usize * ch + c])
            }
        };
        let mut out = Vec::with_capacity(((n as f64 / self.step) as usize + 2) * ch);
        let last = (n - 1) as f64;
        while self.t <= last {
            let idx = self.t.floor();
            let frac = self.t - idx;
            let i = idx as isize;
            for c in 0..ch {
                let a = frame(i, c);
                let v = if frac == 0.0 { a } else { a + (frame(i + 1, c) - a) * frac };
                out.push(v.round().clamp(-32768.0, 32767.0) as i16);
            }
            self.t += self.step;
        }
        self.t -= n as f64;
        self.prev = Some(input[(n - 1) * ch..n * ch].to_vec());
        out
    }
}

/// Sample-wise saturating addition; the shorter side is zero-padded.
pub fn add(a: &[i16], b: &[i16]) -> Vec<i16> {
    let len = a.len().max(b.len());
    (0..len)
        .map(|i| a.get(i).copied().unwrap_or(0).saturating_add(b.get(i).copied().unwrap_or(0)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_and_channels() {
        assert_eq!(to_mono(&[100, 200, -3, -4], 2), vec![150, -4]);
        assert_eq!(convert_channels(&[1, 2], 1, 2), vec![1, 1, 2, 2]);
        assert_eq!(convert_channels(&[2, 4, 6, 8, 10, 12], 3, 2), vec![4, 4, 10, 10]);
    }

    #[test]
    fn resampler_keeps_rate_across_chunks() {
        let mut r = Resampler::new(48000, 16000, 1);
        let input: Vec<i16> = (0..4800).map(|i| (i % 100) as i16).collect();
        let mut total = 0;
        for chunk in input.chunks(1024) {
            total += r.process(chunk).len();
        }
        assert!((1599..=1601).contains(&total), "{total}");
        let mut up = Resampler::new(16000, 48000, 2);
        assert_eq!(up.process(&[0, 0, 300, 300]).len(), 6 * 2 - 4);
    }

    #[test]
    fn add_saturates() {
        assert_eq!(add(&[32000, -32000, 5], &[1000, -1000]), vec![32767, -32768, 5]);
    }
}
