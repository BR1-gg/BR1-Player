use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
    /// Last ffmpeg stderr text, used to explain a failed decode.
    stderr_log: Arc<Mutex<String>>,
    /// Blur strength currently baked into the ffmpeg filter chain.
    blur: f32,
    /// Debounced target used while the user drags the blur slider.
    pending_blur: f32,
    blur_changed_at: Instant,
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
            stderr_log: Arc::new(Mutex::new(String::new())),
            blur: 0.0,
            pending_blur: 0.0,
            blur_changed_at: Instant::now(),
        }
    }

    /// Restarts decoding whenever the configured file or blur changes.
    pub fn sync(&mut self, path: Option<&PathBuf>, blur: f32, ctx: &egui::Context) {
        let blur = blur.clamp(0.0, 1.0);

        if self.path.as_ref() != path {
            self.shutdown();
            self.path = path.cloned();
            if let Some(path) = path {
                self.start(path, blur, ctx);
            }
            return;
        }

        let Some(path) = path else {
            return;
        };
        if (blur - self.blur).abs() <= 0.005 {
            return;
        }

        // Debounce restarts while the slider is being dragged.
        if (blur - self.pending_blur).abs() > 0.005 {
            self.pending_blur = blur;
            self.blur_changed_at = Instant::now();
            return;
        }
        if self.blur_changed_at.elapsed() < Duration::from_millis(160) {
            return;
        }

        let path = path.clone();
        self.shutdown();
        self.path = Some(path.clone());
        self.start(&path, blur, ctx);
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
                let detail = self
                    .stderr_log
                    .lock()
                    .ok()
                    .map(|text| {
                        text.lines()
                            .filter(|line| !line.trim().is_empty())
                            .rev()
                            .take(3)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                self.error = Some(if detail.is_empty() {
                    "The background video stopped early — ffmpeg exited. \
                     The file may be damaged or use an unsupported codec."
                        .to_string()
                } else {
                    format!("The background video stopped early — ffmpeg: {detail}")
                });
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

    fn start(&mut self, path: &Path, blur: f32, ctx: &egui::Context) {
        if !path.is_file() {
            self.error = Some(format!("Background video not found: {}", path.display()));
            return;
        }

        let Some(ffmpeg) = find_ffmpeg() else {
            self.error = Some("Background video needs ffmpeg.exe in the bin folder.".to_string());
            return;
        };

        let input = path.display().to_string();
        let filter = build_filter(blur);

        let mut command = Command::new(&ffmpeg);
        command
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-re",
                "-stream_loop",
                "-1",
                "-i",
                &input,
                "-map",
                "0:V:0",
                "-an",
                "-sn",
                "-dn",
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
            .stderr(Stdio::piped());

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
        self.stderr_log = Arc::new(Mutex::new(String::new()));
        self.blur = blur;
        self.pending_blur = blur;
        self.blur_changed_at = Instant::now();

        let stop = self.stop.clone();
        let died = self.died.clone();
        let stderr_log = self.stderr_log.clone();
        let (sender, receiver) = sync_channel::<Vec<u8>>(2);
        self.receiver = Some(receiver);

        // Pre-create the texture so the first frame has a destination.
        let blank = vec![0u8; FRAME_BYTES];
        let image = ColorImage::from_rgba_unmultiplied([WIDTH, HEIGHT], &blank);
        self.texture = Some(ctx.load_texture("background-video", image, TextureOptions::LINEAR));

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

            // Collect ffmpeg's error output before the receiver disconnects,
            // so the UI can report the real codec/file error.
            if let Some(mut stderr) = child.stderr.take() {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                if let Ok(mut sink) = stderr_log.lock() {
                    *sink = text;
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
        self.stderr_log = Arc::new(Mutex::new(String::new()));
        self.blur = 0.0;
        self.pending_blur = 0.0;
    }
}

impl Drop for VideoBackground {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// ffmpeg filter chain for the decoded background frames. A non-zero blur is
/// done on a small copy and scaled back up, which looks like a strong
/// Gaussian blur without the cost of blurring 1280x720 at 30 fps.
fn build_filter(blur: f32) -> String {
    let base = format!(
        "scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=increase,\
         crop={WIDTH}:{HEIGHT},fps=30"
    );
    let blur = blur.clamp(0.0, 1.0);
    if blur <= 0.004 {
        base
    } else {
        format!(
            "{base},scale=320:180,gblur=sigma={:.2},\
             scale={WIDTH}:{HEIGHT}:flags=bilinear",
            blur * 6.0
        )
    }
}

fn find_ffmpeg() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // Prefer the folder the app installs its tools into.
    if let Some(dir) = crate::download::tools_dir() {
        candidates.push(dir.join("ffmpeg.exe"));
    }

    // Look next to the exe and in neighbouring `bin` folders. `tools_dir`
    // requires yt-dlp to be present, so the video background must not depend
    // on it: ffmpeg is searched for independently.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("ffmpeg.exe"));
            candidates.push(dir.join("bin").join("ffmpeg.exe"));
            let mut ancestor = dir.to_path_buf();
            for _ in 0..4 {
                match ancestor.parent() {
                    Some(parent) => {
                        ancestor = parent.to_path_buf();
                        candidates.push(ancestor.join("ffmpeg.exe"));
                        candidates.push(ancestor.join("bin").join("ffmpeg.exe"));
                    }
                    None => break,
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("ffmpeg.exe"));
        candidates.push(cwd.join("bin").join("ffmpeg.exe"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let dir = PathBuf::from(local).join("BR1 Player").join("bin");
        candidates.push(dir.join("ffmpeg.exe"));
    }

    if let Some(found) = candidates.into_iter().find(|path| path.is_file()) {
        return Some(found);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_without_blur_only_scales_and_crops() {
        let filter = build_filter(0.0);
        assert!(filter.contains("scale=1280:720"));
        assert!(filter.contains("crop=1280:720"));
        assert!(!filter.contains("gblur"));
    }

    #[test]
    fn filter_with_blur_adds_a_cheap_gaussian_pass() {
        let filter = build_filter(1.0);
        assert!(filter.contains("gblur=sigma=6.00"));
        assert!(filter.contains("scale=320:180"));
        assert!(filter.ends_with("scale=1280:720:flags=bilinear"));
    }

    #[test]
    fn filter_with_a_tiny_blur_stays_sharp() {
        assert!(!build_filter(0.001).contains("gblur"));
    }
}
