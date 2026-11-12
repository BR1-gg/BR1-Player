/// Small, dependency-free spectrum analyzer used by the visualizer panel.
pub const SPECTRUM_BINS: usize = 48;

pub fn spectrum(samples: &[f32], sample_rate: u32, bins: usize) -> Vec<f32> {
    if bins == 0 {
        return Vec::new();
    }

    let fft_size = samples.len().clamp(256, 2_048).next_power_of_two();
    let mut re = vec![0.0f32; fft_size];
    let mut im = vec![0.0f32; fft_size];
    let take = samples.len().min(fft_size);
    let offset = fft_size - take;

    for (index, sample) in samples[samples.len() - take..].iter().enumerate() {
        // Hann window.
        let phase = std::f32::consts::TAU * index as f32 / (take.max(1) - 1) as f32;
        let window = 0.5 - 0.5 * phase.cos();
        re[offset + index] = sample * window;
    }

    fft(&mut re, &mut im);

    let nyquist = (sample_rate.max(8_000) / 2) as f32;
    let min_freq = 30.0f32.min(nyquist * 0.5);
    let max_freq = 16_000.0f32.min(nyquist);
    let ratio = (max_freq / min_freq).max(1.0001);
    let hz_per_bin = sample_rate as f32 / fft_size as f32;

    let mut output = vec![0.0f32; bins];
    for (bar, value) in output.iter_mut().enumerate() {
        let low = min_freq * ratio.powf(bar as f32 / bins as f32);
        let high = min_freq * ratio.powf((bar + 1) as f32 / bins as f32);
        let mut start = (low / hz_per_bin).floor().max(1.0) as usize;
        let mut end = (high / hz_per_bin).ceil().max(start as f32) as usize;
        start = start.min(fft_size / 2 - 1);
        end = end.clamp(start + 1, fft_size / 2);

        let mut peak = 0.0f32;
        let mut sum = 0.0f32;
        let mut count = 0usize;
        for bin in start..end {
            let magnitude = (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() / fft_size as f32;
            peak = peak.max(magnitude);
            sum += magnitude;
            count += 1;
        }
        let magnitude = if count > 0 {
            peak * 0.65 + (sum / count as f32) * 0.35
        } else {
            0.0
        };
        let db = 20.0 * (magnitude + 1e-9).log10();
        let normalized = ((db + 66.0) / 66.0).clamp(0.0, 1.0);
        *value = normalized.powf(0.72);
    }

    output
}

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    if n <= 1 {
        return;
    }

    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let angle = -std::f32::consts::TAU / len as f32;
        let w_re = angle.cos();
        let w_im = angle.sin();
        let mut start = 0usize;
        while start < n {
            let mut cur_re = 1.0f32;
            let mut cur_im = 0.0f32;
            for index in 0..len / 2 {
                let even = start + index;
                let odd = even + len / 2;
                let odd_re = re[odd] * cur_re - im[odd] * cur_im;
                let odd_im = re[odd] * cur_im + im[odd] * cur_re;
                re[odd] = re[even] - odd_re;
                im[odd] = im[even] - odd_im;
                re[even] += odd_re;
                im[even] += odd_im;
                let next_re = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = next_re;
            }
            start += len;
        }
        len <<= 1;
    }
}
