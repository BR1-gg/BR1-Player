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

/// Listener/speaker state shared with the audio thread.
pub struct SpatialShared {
    x: AtomicU32,
    y: AtomicU32,
    room: AtomicU32,
    wet: AtomicU32,
}

impl SpatialShared {
    fn new() -> Self {
        Self {
            x: AtomicU32::new(0.0f32.to_bits()),
            y: AtomicU32::new(0.0f32.to_bits()),
            room: AtomicU32::new(8.0f32.to_bits()),
            wet: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    fn read(value: &AtomicU32, fallback: f32) -> f32 {
        let bits = value.load(Ordering::Relaxed);
        if bits == 0 {
            fallback
        } else {
            f32::from_bits(bits)
        }
    }

    pub fn position(&self) -> (f32, f32) {
        (
            Self::read(&self.x, 0.0),
            Self::read(&self.y, 0.0),
        )
    }

    pub fn set_position(&self, x: f32, y: f32) {
        self.x.store(x.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
        self.y.store(y.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn room(&self) -> f32 {
        Self::read(&self.room, 8.0).clamp(2.0, 24.0)
    }

    pub fn set_room(&self, meters: f32) {
        self.room
            .store(meters.clamp(2.0, 24.0).to_bits(), Ordering::Relaxed);
    }

    pub fn reverb(&self) -> f32 {
        Self::read(&self.wet, 0.0).clamp(0.0, 1.0)
    }

    pub fn set_reverb(&self, wet: f32) {
        self.wet
            .store(wet.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

/// One feedback comb, part of the small Schroeder reverb.
struct Comb {
    buffer: Vec<f32>,
    index: usize,
    store: f32,
    feedback: f32,
    damp: f32,
}

impl Comb {
    fn new(samples: usize) -> Self {
        Self {
            buffer: vec![0.0; samples.max(1)],
            index: 0,
            store: 0.0,
            feedback: 0.7,
            damp: 0.28,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.index];
        self.store = output * (1.0 - self.damp) + self.store * self.damp;
        self.buffer[self.index] = input + self.store * self.feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
        self.store = 0.0;
    }
}

struct Allpass {
    buffer: Vec<f32>,
    index: usize,
    gain: f32,
}

impl Allpass {
    fn new(samples: usize) -> Self {
        Self {
            buffer: vec![0.0; samples.max(1)],
            index: 0,
            gain: 0.5,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = buffered - input;
        self.buffer[self.index] = input + buffered * self.gain;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
    }
}

/// Places the stereo signal in a 2D room: equal-power panning by direction,
/// attenuation by distance (scaled by room size) and a Schroeder reverb whose
/// tail grows with the room and whose wet level is user controlled.
struct SpatialSource<S> {
    inner: S,
    shared: Arc<SpatialShared>,
    in_channels: usize,
    sample_rate: u32,
    frame: [f32; 8],
    pending: Option<f32>,
    counter: u64,
    gain: f32,
    gain_l: f32,
    gain_r: f32,
    wet: f32,
    feedback: f32,
    combs: Vec<Comb>,
    allpass: Vec<Allpass>,
}

impl<S> SpatialSource<S>
where
    S: Source<Item = f32>,
{
    fn new(inner: S, shared: Arc<SpatialShared>) -> Self {
        let sample_rate = inner.sample_rate().get().max(8_000);
        let scale = sample_rate as f32 / 44_100.0;
        let comb_delays = [1557.0f32, 1617.0, 1491.0, 1422.0];
        let allpass_delays = [556.0f32, 225.0];

        let mut source = Self {
            in_channels: inner.channels().get().max(1) as usize,
            inner,
            shared,
            sample_rate,
            frame: [0.0; 8],
            pending: None,
            counter: 0,
            gain: 1.0,
            gain_l: 0.85,
            gain_r: 0.85,
            wet: 0.15,
            feedback: 0.72,
            combs: comb_delays
                .iter()
                .map(|delay| Comb::new((delay * scale) as usize))
                .collect(),
            allpass: allpass_delays
                .iter()
                .map(|delay| Allpass::new((delay * scale) as usize))
                .collect(),
        };
        source.refresh();
        source
    }

    fn refresh(&mut self) {
        let (x, y) = self.shared.position();
        let room = self.shared.room();

        let (gain, gain_l, gain_r, _meters) = spatial_gains(x, y, room);
        self.gain = gain;
        self.gain_l = gain_l;
        self.gain_r = gain_r;
        self.wet = self.shared.reverb();
        self.feedback = 0.68 + 0.24 * ((room - 2.0) / 22.0).clamp(0.0, 1.0);

        for comb in &mut self.combs {
            comb.feedback = self.feedback;
        }
    }

    fn fill_frame(&mut self) -> Option<()> {
        for index in 0..self.in_channels.min(self.frame.len()) {
            self.frame[index] = self.inner.next()?;
        }
        Some(())
    }

    fn process_frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.counter % 64 == 0 {
            self.refresh();
        }
        self.counter = self.counter.wrapping_add(1);

        let dry_l = left * self.gain_l;
        let dry_r = right * self.gain_r;

        let mono = (left + right) * 0.5 * self.gain;
        let mut wet_sum = 0.0;
        for comb in &mut self.combs {
            wet_sum += comb.process(mono);
        }
        let mut wet = wet_sum * 0.25;
        for allpass in &mut self.allpass {
            wet = allpass.process(wet);
        }
        wet *= self.wet * 0.9;

        (
            (dry_l + wet).clamp(-1.0, 1.0),
            (dry_r + wet).clamp(-1.0, 1.0),
        )
    }
}

impl<S> Iterator for SpatialSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(sample) = self.pending.take() {
            return Some(sample);
        }

        self.fill_frame()?;
        let (left, right) = if self.in_channels >= 2 {
            (self.frame[0], self.frame[1])
        } else {
            (self.frame[0], self.frame[0])
        };

        let (out_l, out_r) = self.process_frame(left, right);
        self.pending = Some(out_r);
        Some(out_l)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for SpatialSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(2).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(self.sample_rate).unwrap_or(NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)?;
        for comb in &mut self.combs {
            comb.reset();
        }
        for allpass in &mut self.allpass {
            allpass.reset();
        }
        self.counter = 0;
        Ok(())
    }
}

/// Equal-power panning by direction plus distance attenuation in metres.
/// Returns `(attenuation, left gain, right gain, distance in metres)`.
fn spatial_gains(x: f32, y: f32, room: f32) -> (f32, f32, f32, f32) {
    let distance = (x * x + y * y).sqrt().clamp(0.0, 1.0);
    let meters = distance * room.clamp(2.0, 24.0);
    let attenuation = 1.0 / (1.0 + 0.28 * meters);

    let theta = (x.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    // Unity at the centre, equal power at the sides.
    let pan_gain = std::f32::consts::SQRT_2;

    (
        attenuation,
        (theta.cos() * pan_gain * attenuation).max(0.0),
        (theta.sin() * pan_gain * attenuation).max(0.0),
        meters,
    )
}

/// Thin wrapper around rodio: one sink, one track at a time, plus a live EQ and
/// a sample tap used by the spectrum visualizer.
pub struct AudioEngine {    _stream: MixerDeviceSink,
    sink: Option<Player>,
    volume: f32,
    muted: bool,
    fx: Arc<FxShared>,
    spatial: Arc<SpatialShared>,
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let stream = DeviceSinkBuilder::open_default_sink()
            .map_err(|err| format!("No audio output: {err}"))?;
        Ok(Self {
            _stream: stream,
            sink: None,
            volume: 0.8,
            muted: false,
            fx: Arc::new(FxShared::new()),
            spatial: Arc::new(SpatialShared::new()),
        })
    }

    pub fn play(&mut self, path: &Path) -> Result<(), String> {
        let file = File::open(path).map_err(|err| format!("Cannot open file: {err}"))?;
        let source = Decoder::new(BufReader::new(file))
            .map_err(|err| format!("Unsupported audio format: {err}"))?;
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

    /// Listener is at the centre; `x`/`y` are -1..1 inside the room.
    pub fn set_spatial(&self, x: f32, y: f32) {
        self.spatial.set_position(x, y);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_is_transparent() {
        let (attenuation, left, right, meters) = spatial_gains(0.0, 0.0, 8.0);
        assert!((attenuation - 1.0).abs() < 1e-6);
        assert!((left - 1.0).abs() < 1e-4);
        assert!((right - 1.0).abs() < 1e-4);
        assert!(meters.abs() < 1e-6);
    }

    #[test]
    fn direction_pans_and_distance_quiets() {
        let (_near_att, near_l, near_r, near_m) = spatial_gains(0.4, 0.0, 10.0);
        let (_far_att, far_l, far_r, far_m) = spatial_gains(1.0, 0.0, 10.0);

        assert!(near_r > near_l, "speaker on the right should favour the right ear");
        assert!(
            far_r / far_l.max(1e-6) > near_r / near_l,
            "moving further right must widen the stereo image"
        );
        assert!(far_l >= 0.0 && far_r >= 0.0, "gains must never invert");
        assert!(far_m > near_m);

        let (edge_att, _, _, edge_m) = spatial_gains(0.0, 1.0, 10.0);
        assert!(edge_m > 9.9);
        assert!(edge_att < 0.35, "far away should be attenuated");
        assert!(edge_att > 0.0);
    }

    #[test]
    fn room_size_scales_distance() {
        let (_, _, _, small) = spatial_gains(0.5, 0.5, 2.0);
        let (_, _, _, large) = spatial_gains(0.5, 0.5, 20.0);
        assert!(large > small * 4.0);
    }

    #[test]
    fn shared_state_round_trips() {
        let shared = SpatialShared::new();
        assert_eq!(shared.position(), (0.0, 0.0));
        shared.set_position(0.5, -0.25);
        assert_eq!(shared.position(), (0.5, -0.25));
        shared.set_position(7.0, -7.0);
        assert_eq!(shared.position(), (1.0, -1.0));

        shared.set_room(99.0);
        assert_eq!(shared.room(), 24.0);
        shared.set_reverb(2.0);
        assert_eq!(shared.reverb(), 1.0);
        shared.set_reverb(-1.0);
        assert_eq!(shared.reverb(), 0.0);
    }
}