use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use crate::config::EQ_BANDS;

pub const EQ_FREQS: [f32; EQ_BANDS] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

const TAP_CAPACITY: usize = 8_192;

/// Shared between the UI thread and the audio source that is currently playing.
pub struct FxShared {
    gains: Mutex<[f32; EQ_BANDS]>,
    enabled: AtomicBool,
    tap: Mutex<VecDeque<f32>>,
    sample_rate: AtomicU32,
}

impl FxShared {
    fn new() -> Self {
        Self {
            gains: Mutex::new([0.0; EQ_BANDS]),
            enabled: AtomicBool::new(true),
            tap: Mutex::new(VecDeque::with_capacity(TAP_CAPACITY)),
            sample_rate: AtomicU32::new(44_100),
        }
    }

    pub fn set_eq(&self, gains: &[f32; EQ_BANDS], enabled: bool) {
        if let Ok(mut current) = self.gains.lock() {
            *current = *gains;
        }
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn push_mono(&self, sample: f32) {
        if let Ok(mut buffer) = self.tap.try_lock() {
            if buffer.len() >= TAP_CAPACITY {
                buffer.pop_front();
            }
            buffer.push_back(sample);
        }
    }

    pub fn clear_tap(&self) {
        if let Ok(mut buffer) = self.tap.lock() {
            buffer.clear();
        }
    }

    /// Copies the most recent mono samples for the visualizer.
    pub fn snapshot(&self, count: usize) -> (Vec<f32>, u32) {
        let rate = self.sample_rate.load(Ordering::Relaxed);
        let mut out = vec![0.0; count];
        if let Ok(buffer) = self.tap.lock() {
            let take = buffer.len().min(count);
            let offset = count - take;
            for (index, sample) in buffer.iter().skip(buffer.len() - take).enumerate() {
                out[offset + index] = *sample;
            }
        }
        (out, rate)
    }
}

/// A peaking biquad filter, direct form I.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
}

impl Biquad {
    fn peaking(sample_rate: f32, frequency: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * frequency / sample_rate;
        let cos = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
            ..Self::default()
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Wraps the decoder with the live equalizer and the visualizer tap.
struct FxSource<S> {
    inner: S,
    fx: Arc<FxShared>,
    channels: u16,
    sample_rate: u32,
    filters: Vec<[Biquad; EQ_BANDS]>,
    gains: [f32; EQ_BANDS],
    enabled: bool,
    frame: u64,
    mono_sum: f32,
    mono_count: usize,
    pending: Vec<f32>,
}

impl<S> FxSource<S>
where
    S: Source<Item = i16>,
{
    fn new(inner: S, fx: Arc<FxShared>) -> Self {
        let channels = inner.channels().max(1);
        let sample_rate = inner.sample_rate().max(8_000);
        fx.sample_rate.store(sample_rate, Ordering::Relaxed);
        let gains = fx
            .gains
            .lock()
            .map(|guard| *guard)
            .unwrap_or([0.0; EQ_BANDS]);
        let enabled = fx.enabled.load(Ordering::Relaxed);
        let mut filters = Vec::with_capacity(channels as usize);
        for _ in 0..channels {
            filters.push(std::array::from_fn(|band| {
                Biquad::peaking(sample_rate as f32, EQ_FREQS[band], 1.25, gains[band])
            }));
        }
        Self {
            inner,
            fx,
            channels,
            sample_rate,
            filters,
            gains,
            enabled,
            frame: 0,
            mono_sum: 0.0,
            mono_count: 0,
            pending: Vec::with_capacity(512),
        }
    }

    fn refresh(&mut self) {
        let gains = self
            .fx
            .gains
            .lock()
            .map(|guard| *guard)
            .unwrap_or(self.gains);
        let enabled = self.fx.enabled.load(Ordering::Relaxed);
        if gains != self.gains || enabled != self.enabled {
            self.gains = gains;
            self.enabled = enabled;
            for channel in &mut self.filters {
                for (band, filter) in channel.iter_mut().enumerate() {
                    *filter =
                        Biquad::peaking(self.sample_rate as f32, EQ_FREQS[band], 1.25, gains[band]);
                }
            }
        }
    }

    fn flush_tap(&mut self) {
        if self.pending.len() >= 256 {
            for sample in &self.pending {
                self.fx.push_mono(*sample);
            }
            self.pending.clear();
        }
    }

    fn reset_filters(&mut self) {
        for channel in &mut self.filters {
            for filter in channel {
                filter.reset();
            }
        }
        self.pending.clear();
        self.fx.clear_tap();
    }
}

impl<S> Iterator for FxSource<S>
where
    S: Source<Item = i16>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let raw = self.inner.next()?;
        if self.frame.is_multiple_of(2_048) {
            self.refresh();
        }

        let input = raw as f32 / 32_768.0;
        let channel = (self.frame as usize) % self.channels.max(1) as usize;
        let mut value = input;

        if self.enabled {
            if let Some(filters) = self.filters.get_mut(channel) {
                for filter in filters.iter_mut() {
                    value = filter.process(value);
                }
            }
        }

        self.mono_sum += value;
        self.mono_count += 1;
        if channel + 1 == self.channels as usize {
            let mono = self.mono_sum / self.mono_count.max(1) as f32;
            self.pending.push(mono);
            self.mono_sum = 0.0;
            self.mono_count = 0;
            self.flush_tap();
        }

        self.frame = self.frame.wrapping_add(1);
        Some(value.clamp(-1.0, 1.0))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for FxSource<S>
where
    S: Source<Item = i16>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)?;
        self.frame = 0;
        self.reset_filters();
        Ok(())
    }
}

/// Thin wrapper around rodio: one sink, one track at a time, plus a live EQ and
/// a sample tap used by the spectrum visualizer.
pub struct AudioEngine {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    volume: f32,
    muted: bool,
    fx: Arc<FxShared>,
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let (stream, handle) =
            OutputStream::try_default().map_err(|err| format!("No audio output: {err}"))?;
        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
            volume: 0.8,
            muted: false,
            fx: Arc::new(FxShared::new()),
        })
    }

    pub fn play(&mut self, path: &Path) -> Result<(), String> {
        let file = File::open(path).map_err(|err| format!("Cannot open file: {err}"))?;
        let source = Decoder::new(BufReader::new(file))
            .map_err(|err| format!("Unsupported audio format: {err}"))?;
        let source = FxSource::new(source, self.fx.clone());

        if let Some(sink) = self.sink.take() {
            sink.stop();
        }

        let sink =
            Sink::try_new(&self.handle).map_err(|err| format!("Audio device error: {err}"))?;
        sink.set_volume(self.effective_volume());
        sink.append(source);
        sink.play();
        self.sink = Some(sink);
        Ok(())
    }

    pub fn set_eq(&self, gains: &[f32; EQ_BANDS], enabled: bool) {
        self.fx.set_eq(gains, enabled);
    }

    pub fn visualizer_samples(&self, count: usize) -> (Vec<f32>, u32) {
        self.fx.snapshot(count)
    }

    fn effective_volume(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.volume
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.effective_volume());
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if let Some(sink) = &self.sink {
            sink.set_volume(self.effective_volume());
        }
    }

    pub fn pause(&self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
    }

    pub fn resume(&self) {
        if let Some(sink) = &self.sink {
            sink.play();
        }
    }

    pub fn seek(&self, seconds: f32) {
        if let Some(sink) = &self.sink {
            let _ = sink.try_seek(Duration::from_secs_f32(seconds.max(0.0)));
        }
    }

    pub fn position(&self) -> f32 {
        self.sink
            .as_ref()
            .map(|sink| sink.get_pos().as_secs_f32())
            .unwrap_or(0.0)
    }

    /// True once the queued source has finished playing.
    pub fn finished(&self) -> bool {
        self.sink.as_ref().map(|sink| sink.empty()).unwrap_or(false)
    }
}
