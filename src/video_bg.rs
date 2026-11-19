use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;

use egui::{ColorImage, TextureHandle, TextureOptions};

const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
const FRAME_BYTES: usize = WIDTH * HEIGHT * 4;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Looping video background: ffmpeg decodes frames into RGBA and the latest one
/// is uploaded to a texture. Failures are reported through [`Self::take_error`]
/// instead of being silently ignored.
pub struct VideoBackground {
    path: Option<PathBuf>,
    texture: Option<TextureHandle>,
    receiver: Option<Receiver<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    pid: Option<u32>,
    has_frame: bool,
    /// Why decoding could not start, if it could not.
    error: Option<String>,
    /// Set by the reader thread when ffmpeg exited on its own.
    died: Arc<AtomicBool>,
}

impl Default for VideoBackground {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoBackground {
    pub fn new() -> Self {
        Self {
            path: None,
            texture: None,
            receiver: None,
            stop: Arc::new(AtomicBool::new(false)),
            pid: None,
            has_frame: false,
            error: None,
            died: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Restarts decoding whenever the configured file changes.
    pub fn sync(&mut self, path: Option<&PathBuf>, ctx: &egui::Context) {
        if self.path.as_ref() == path {
            return;
        }

        self.shutdown();
        self.path = path.cloned();

        if let Some(path) = path {
            self.start(path, ctx);
        }
    }

    /// Why decoding could not start, if it could not. Cleared when read so
    /// the caller shows each failure exactly once.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// Uploads the freshest decoded frame, if any.
    pub fn update(&mut self) -> bool {
        let mut latest = None;
        let mut disconnected = false;

        if let Some(receiver) = &self.receiver {
            loop {
                match receiver.try_recv() {
                    Ok(frame) => latest = Some(frame),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        if disconnected {
            self.receiver = None;
            if self.died.load(Ordering::Acquire) {
                self.error = Some(
                    "The background video stopped early — ffmpeg exited. \
                     The file may be damaged or use an unsupported codec."
                        .to_string(),
                );
            }
        }

        let Some(frame) = latest else {
            return false;
        };

        let Some(texture) = &mut self.texture else {
            return false;
        };

        let image = ColorImage::from_rgba_unmultiplied([WIDTH, HEIGHT], &frame);
        texture.set(image, TextureOptions::LINEAR);
        self.has_frame = true;
        true
    }

    pub fn texture(&self) -> Option<&TextureHandle> {
        if self.has_frame {
            self.texture.as_ref()
        } else {
            None
        }
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    fn start(&mut self, path: &Path, ctx: &egui::Context) {
        if !path.is_file() {
            self.error = Some(format!(
                "Background video not found: {}",
                path.display()
            ));
            return;
        }

        let Some(ffmpeg) = find_ffmpeg() else {
            self.error =
                Some("Background video needs ffmpeg.exe in the bin folder.".to_string());
            return;
        };

        let input = path.display().to_string();
        let filter = format!(
            "scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=increase,crop={WIDTH}:{HEIGHT},fps=30"
        );

        let mut command = Command::new(&ffmpeg);
        command
            .args([
                "-re",
                "-stream_loop",
                "-1",
                "-i",
                &input,
                "-an",
                "-vf",
                &filter,
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let Ok(mut child) = command.spawn() else {
            self.error = Some(format!(
                "Could not start ffmpeg for the background video ({}).",
                ffmpeg.display()
            ));
            return;
        };
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            self.error = Some("ffmpeg gave no output pipe for the video.".to_string());
            return;
        };

        self.pid = Some(child.id());
        self.stop = Arc::new(AtomicBool::new(false));
        self.has_frame = false;
        self.died = Arc::new(AtomicBool::new(false));

        let stop = self.stop.clone();
        let died = self.died.clone();
        let (sender, receiver) = sync_channel::<Vec<u8>>(2);
        self.receiver = Some(receiver);

        // Pre-create the texture so the first frame has a destination.
        let blank = vec![0u8; FRAME_BYTES];
        let image = ColorImage::from_rgba_unmultiplied([WIDTH, HEIGHT], &blank);
        self.texture = Some(ctx.load_texture(
            "background-video",
            image,
            TextureOptions::LINEAR,
        ));

        thread::spawn(move || {
            let mut buffer = vec![0u8; FRAME_BYTES];

            loop {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                match stdout.read_exact(&mut buffer) {
                    Ok(()) => {
                        if sender.send(buffer.clone()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }

            // Only a genuine failure, not a requested shutdown, is reported.
            if !stop.load(Ordering::Acquire) {
                died.store(true, Ordering::Release);
            }

            let _ = child.kill();
            let _ = child.wait();
        });
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.receiver = None;

        if let Some(pid) = self.pid.take() {
            kill_process(pid);
        }

        self.texture = None;
        self.has_frame = false;
        self.path = None;
    }
}

impl Drop for VideoBackground {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(dir) = crate::download::tools_dir() {
        let candidate = dir.join("ffmpeg.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // Anything on PATH (spawn fails harmlessly when there is none).
    Some(PathBuf::from("ffmpeg.exe"))
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    use std::os::windows::process::CommandExt;

    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(not(windows))]
fn kill_process(_pid: u32) {}
