use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

/// Thin wrapper around rodio: one sink, one track at a time.
pub struct AudioEngine {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    volume: f32,
    muted: bool,
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
        })
    }

    pub fn play(&mut self, path: &Path) -> Result<(), String> {
        let file = File::open(path).map_err(|err| format!("Cannot open file: {err}"))?;
        let source = Decoder::new(BufReader::new(file))
            .map_err(|err| format!("Unsupported audio format: {err}"))?;

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
