use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::num::NonZero;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};

use crate::config::{EQ_BANDS, UiConfig};
use crate::spatial::{self, SpatialShared, SpatialSource, TestMode, TestSignal};

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
    S: Source<Item = f32>,
{
    fn new(inner: S, fx: Arc<FxShared>) -> Self {
        let channels = inner.channels().get();
        let sample_rate = inner.sample_rate().get().max(8_000);
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
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let raw = self.inner.next()?;
        if self.frame.is_multiple_of(2_048) {
            self.refresh();
        }

        let input = raw.clamp(-1.0, 1.0);
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
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(self.channels).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(self.sample_rate).unwrap_or(NonZero::<u32>::MIN)
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

/// Thin wrapper around rodio: one sink for music, one optional sink for the
/// HRTF test tone, plus a live EQ and a sample tap for the visualizer.
pub struct AudioEngine {
    _stream: MixerDeviceSink,
    sink: Option<Player>,
    test_sink: Option<Player>,
    volume: f32,
    muted: bool,
    fx: Arc<FxShared>,
    spatial: Arc<SpatialShared>,
    hrtf_sets: Vec<spatial::HrtfSetInfo>,
    hrtf_set_id: String,
    manual_position: [f32; 3],
    test_rate: Option<u32>,
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let stream = DeviceSinkBuilder::open_default_sink()
            .map_err(|err| format!("No audio output: {err}"))?;
        Ok(Self {
            _stream: stream,
            sink: None,
            test_sink: None,
            volume: 0.8,
            muted: false,
            fx: Arc::new(FxShared::new()),
            spatial: Arc::new(SpatialShared::new()),
            hrtf_sets: spatial::available_hrtf_sets(),
            hrtf_set_id: String::new(),
            manual_position: [0.0, 0.0, spatial::MIN_DISTANCE],
            test_rate: None,
        })
    }

    /// Apply the persisted spatial settings. Returns a warning when the saved
    /// SOFA set could not be restored (the built-in set is used instead).
    pub fn configure(&mut self, ui: &mut UiConfig) -> Option<String> {
        let position = spatial::position_from_pad(
            ui.spatial_x,
            ui.spatial_y,
            ui.spatial_height,
            ui.room_size,
        );
        self.manual_position = position;
        self.spatial.set_position(position);
        self.spatial.set_room(ui.room_size);
        self.spatial.set_reverb(ui.reverb);
        self.spatial.set_head_yaw(ui.head_yaw);
        self.spatial.set_hrtf_enabled(ui.hrtf_enabled);
        self.set_eq(&ui.eq_gains, ui.eq_enabled);

        let mut warning = None;
        if let Err(error) = self.set_hrtf_set(&ui.hrtf_set) {
            warning = Some(error);
            if let Err(fallback) = self.set_hrtf_set(spatial::DEFAULT_SET_ID) {
                warning = Some(format!(
                    "{}; fallback HRTF set also failed: {fallback}",
                    warning.unwrap_or_default()
                ));
            } else {
                ui.hrtf_set = spatial::DEFAULT_SET_ID.to_string();
            }
        }
        warning
    }

    pub fn play(&mut self, path: &Path) -> Result<(), String> {
        let file = File::open(path).map_err(|err| format!("Cannot open file: {err}"))?;
        let source = Decoder::new(BufReader::new(file))
            .map_err(|err| format!("Unsupported audio format: {err}"))?;
        let rate = source.sample_rate().get();
        self.ensure_hrtf(rate);
        self.spatial.set_sample_rate(rate);

        let source = FxSource::new(source, self.fx.clone());
        let source = SpatialSource::new(source, self.spatial.clone());

        if let Some(sink) = self.sink.take() {
            sink.stop();
        }

        let sink = Player::connect_new(self._stream.mixer());
        sink.set_volume(self.effective_volume());
        sink.append(source);
        sink.play();
        self.sink = Some(sink);
        Ok(())
    }

    pub fn set_eq(&self, gains: &[f32; EQ_BANDS], enabled: bool) {
        self.fx.set_eq(gains, enabled);
    }

    pub fn hrtf_sets(&self) -> &[spatial::HrtfSetInfo] {
        &self.hrtf_sets
    }

    pub fn hrtf_set_id(&self) -> &str {
        &self.hrtf_set_id
    }

    pub fn rescan_hrtf_sets(&mut self) {
        self.hrtf_sets = spatial::available_hrtf_sets();
        if !self
            .hrtf_sets
            .iter()
            .any(|set| set.id == self.hrtf_set_id)
        {
            self.hrtf_set_id.clear();
            let _ = self.set_hrtf_set(spatial::DEFAULT_SET_ID);
        }
    }

    pub fn set_hrtf_set(&mut self, id: &str) -> Result<(), String> {
        let set = self
            .hrtf_sets
            .iter()
            .find(|set| set.id == id)
            .cloned()
            .ok_or_else(|| format!("HRTF set '{id}' is not available"))?;

        let current_rate = self.spatial.sample_rate();
        let mut rates = vec![current_rate];
        if let Some(rate) = self.test_rate {
            if rate != current_rate {
                rates.push(rate);
            }
        }

        let mut providers = Vec::with_capacity(rates.len());
        for rate in rates {
            match spatial::load_provider(&set.kind, rate) {
                Ok(provider) => providers.push((rate, provider)),
                Err(error) if rate == current_rate => return Err(error),
                Err(_) => {}
            }
        }

        self.spatial.install_set(&set.id, providers);
        self.hrtf_set_id = set.id;
        Ok(())
    }

    fn ensure_hrtf(&mut self, rate: u32) {
        if self.hrtf_set_id.is_empty() {
            return;
        }
        if self.spatial.provider_for(rate).is_some() {
            return;
        }
        let Some(set) = self
            .hrtf_sets
            .iter()
            .find(|set| set.id == self.hrtf_set_id)
            .cloned()
        else {
            return;
        };
        if let Ok(provider) = spatial::load_provider(&set.kind, rate) {
            self.spatial
                .ensure_provider(&set.id, rate, provider);
        }
    }

    /// Set the object position in metres (`x` right, `y` up, `z` front).
    pub fn set_source_position(&mut self, position: [f32; 3]) {
        self.manual_position = position;
        self.spatial.set_position(position);
    }

    pub fn spatial_position(&self) -> [f32; 3] {
        self.spatial.position()
    }

    pub fn set_head_yaw(&self, degrees: f32) {
        self.spatial.set_head_yaw(degrees);
    }

    pub fn set_hrtf_enabled(&self, enabled: bool) {
        self.spatial.set_hrtf_enabled(enabled);
    }

    pub fn test_mode(&self) -> TestMode {
        self.spatial.test_mode()
    }

    pub fn set_test_mode(&mut self, mode: TestMode) {
        self.spatial.set_test_mode(mode);
        if !mode.active() {
            if let Some(sink) = &self.test_sink {
                sink.pause();
            }
            self.spatial.set_position(self.manual_position);
            return;
        }

        if self.test_sink.is_none() {
            let rate = self.spatial.sample_rate();
            self.ensure_hrtf(rate);
            let sink = Player::connect_new(self._stream.mixer());
            sink.set_volume(self.effective_volume() * 0.75);
            sink.append(SpatialSource::new(
                TestSignal::new(rate, self.spatial.clone()),
                self.spatial.clone(),
            ));
            sink.play();
            self.test_sink = Some(sink);
            self.test_rate = Some(rate);
        } else if let Some(sink) = &self.test_sink {
            sink.play();
        }
    }

    pub fn set_room_size(&self, meters: f32) {
        self.spatial.set_room(meters);
    }

    pub fn set_reverb(&self, wet: f32) {
        self.spatial.set_reverb(wet);
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
        if let Some(sink) = &self.test_sink {
            sink.set_volume(self.effective_volume() * 0.75);
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if let Some(sink) = &self.sink {
            sink.set_volume(self.effective_volume());
        }
        if let Some(sink) = &self.test_sink {
            sink.set_volume(self.effective_volume() * 0.75);
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
