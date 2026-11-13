//! One-time installation of the external tools the downloader needs.
//!
//! The player itself is a single executable; `yt-dlp.exe`, `spotdl.exe` and
//! `ffmpeg.exe` are fetched into a `bin` folder next to the executable (or into
//! `%LOCALAPPDATA%\BR1 Player\bin` when that folder is read-only) the first
//! time it starts, while the UI shows a loading screen. [`start`] does all the
//! work on a worker thread and reports progress through a channel.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Executable name, label shown in the loading screen and download weight in MB.
const TOOLS: [(&str, &str, f32); 3] = [
    ("yt-dlp.exe", "yt-dlp", 24.0),
    ("spotdl.exe", "spotdl", 47.0),
    ("ffmpeg.exe", "FFmpeg", 115.0),
];

const YT_DLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const SPOTDL_RELEASE_API: &str =
    "https://api.github.com/repos/spotDL/spotify-downloader/releases/latest";
/// Used when the GitHub API cannot be reached (rate limit, offline proxy, ...).
const SPOTDL_FALLBACK_URL: &str =
    "https://github.com/spotDL/spotify-downloader/releases/download/v4.6.0/spotdl-4.6.0-win32.exe";
const FFMPEG_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";

/// Progress report from the installer thread.
pub enum Event {
    /// `tool` is an index into [`TOOLS`]; `fraction` is the overall progress
    /// (`None` while the size is unknown, e.g. while unpacking the zip).
    Progress {
        tool: usize,
        status: String,
        fraction: Option<f32>,
    },
    Finished(Result<(), String>),
}

/// Tools that are missing from the folder the player loads them from.
pub fn missing() -> Vec<usize> {
    match crate::download::tools_dir() {
        Some(dir) => TOOLS
            .iter()
            .enumerate()
            .filter(|(_, (file, _, _))| !dir.join(file).is_file())
            .map(|(index, _)| index)
            .collect(),
        None => (0..TOOLS.len()).collect(),
    }
}

/// True when every download tool is already in place.
pub fn ready() -> bool {
    missing().is_empty()
}

/// Label of a tool, for status lines and the loading screen.
pub fn label(index: usize) -> &'static str {
    TOOLS
        .get(index)
        .map(|(_, label, _)| *label)
        .unwrap_or("tool")
}

/// Number of tools the loading screen lists.
pub fn count() -> usize {
    TOOLS.len()
}

/// Folder the tools are installed into.
pub fn target_dir() -> PathBuf {
    if let Some(dir) = crate::download::tools_dir() {
        if writable(&dir) {
            return dir;
        }
    }

    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("bin"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("BR1 Player").join("bin"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("bin"));
    }

    candidates
        .into_iter()
        .find(|dir| writable(dir))
        .unwrap_or_else(|| PathBuf::from("bin"))
}

/// Starts the installation on a worker thread.
pub fn start() -> Receiver<Event> {
    let (sender, receiver) = channel();
    thread::spawn(move || {
        let result = run(&sender);
        let _ = sender.send(Event::Finished(result));
    });
    receiver
}

fn run(sender: &Sender<Event>) -> Result<(), String> {
    let dir = target_dir();
    fs::create_dir_all(&dir).map_err(|err| format!("Cannot create {}: {err}", dir.display()))?;

    let pending = missing();
    let total: f32 = pending
        .iter()
        .map(|index| TOOLS[*index].2)
        .sum::<f32>()
        .max(1.0);

    let agent = agent();

    let mut done_weight = 0.0f32;
    for index in pending {
        let span = TOOLS[index].2 / total;
        let (file, label, _) = TOOLS[index];

        match file {
            "ffmpeg.exe" => install_ffmpeg(&agent, &dir, sender, index, done_weight, span)?,
            "spotdl.exe" => {
                let url = spotdl_url(&agent).unwrap_or_else(|| SPOTDL_FALLBACK_URL.to_string());
                download_tool(
                    &agent,
                    &url,
                    &dir,
                    file,
                    label,
                    sender,
                    index,
                    done_weight,
                    span,
                )?;
            }
            _ => {
                download_tool(
                    &agent,
                    YT_DLP_URL,
                    &dir,
                    file,
                    label,
                    sender,
                    index,
                    done_weight,
                    span,
                )?;
            }
        }

        done_weight += span;
    }

    let _ = sender.send(Event::Progress {
        tool: TOOLS.len(),
        status: "All set — starting BR1 Player…".to_string(),
        fraction: Some(1.0),
    });
    Ok(())
}

/// HTTP client used for every download. Native TLS keeps the build free of a
/// bundled certificate stack and uses the platform store on Windows.
fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .build();

    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .user_agent("BR1-Player")
            .tls_config(tls)
            .build(),
    )
}

/// Downloads an executable straight into `dir`.
#[allow(clippy::too_many_arguments)]
fn download_tool(
    agent: &ureq::Agent,
    url: &str,
    dir: &Path,
    file: &str,
    label: &str,
    sender: &Sender<Event>,
    tool: usize,
    base: f32,
    span: f32,
) -> Result<(), String> {
    let target = dir.join(file);
    let temporary = dir.join(format!(".{file}.download"));

    let mut response = agent
        .get(url)
        .call()
        .map_err(|err| format!("Could not download {label}: {err}"))?;
    let total = response.body().content_length();
    let mut reader = response.body_mut().as_reader();
    let mut out = File::create(&temporary)
        .map_err(|err| format!("Cannot write to {}: {err}", temporary.display()))?;

    let mut buffer = vec![0u8; 256 * 1024];
    let mut written: u64 = 0;
    let mut last = Instant::now();

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| format!("Download of {label} failed: {err}"))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read])
            .map_err(|err| format!("Cannot write {label}: {err}"))?;
        written += read as u64;

        if last.elapsed() >= Duration::from_millis(90) {
            last = Instant::now();
            let fraction =
                total.map(|total| base + span * (written as f32 / total as f32).min(1.0));
            let _ = sender.send(Event::Progress {
                tool,
                status: format!("Downloading {label} — {}", size_text(written, total)),
                fraction,
            });
        }
    }

    out.flush().ok();
    drop(out);

    if let Some(total) = total {
        if written != total {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "Download of {label} stopped early ({} of {}).",
                bytes_text(written),
                bytes_text(total)
            ));
        }
    }

    replace_file(&temporary, &target)?;
    let _ = sender.send(Event::Progress {
        tool,
        status: format!("{label} installed"),
        fraction: Some(base + span),
    });
    Ok(())
}

/// FFmpeg ships as a zip; only `ffmpeg.exe` and `ffprobe.exe` are kept.
fn install_ffmpeg(
    agent: &ureq::Agent,
    dir: &Path,
    sender: &Sender<Event>,
    tool: usize,
    base: f32,
    span: f32,
) -> Result<(), String> {
    let archive = dir.join(".ffmpeg-setup.zip");
    let _ = fs::remove_file(&archive);

    let mut response = agent
        .get(FFMPEG_URL)
        .call()
        .map_err(|err| format!("Could not download FFmpeg: {err}"))?;
    let total = response.body().content_length();
    let mut reader = response.body_mut().as_reader();
    let mut out = File::create(&archive)
        .map_err(|err| format!("Cannot write {}: {err}", archive.display()))?;

    let mut buffer = vec![0u8; 256 * 1024];
    let mut written: u64 = 0;
    let mut last = Instant::now();

    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| format!("Download of FFmpeg failed: {err}"))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read])
            .map_err(|err| format!("Cannot write FFmpeg: {err}"))?;
        written += read as u64;

        if last.elapsed() >= Duration::from_millis(90) {
            last = Instant::now();
            let fraction =
                total.map(|total| base + span * 0.92 * (written as f32 / total as f32).min(1.0));
            let _ = sender.send(Event::Progress {
                tool,
                status: format!("Downloading FFmpeg — {}", size_text(written, total)),
                fraction,
            });
        }
    }

    out.flush().ok();
    drop(out);

    if let Some(total) = total {
        if written != total {
            let _ = fs::remove_file(&archive);
            return Err(format!(
                "Download of FFmpeg stopped early ({} of {}).",
                bytes_text(written),
                bytes_text(total)
            ));
        }
    }

    let _ = sender.send(Event::Progress {
        tool,
        status: "Unpacking FFmpeg…".to_string(),
        fraction: None,
    });

    let extracted = unpack_ffmpeg(&archive, dir);
    let _ = fs::remove_file(&archive);
    extracted?;

    let _ = sender.send(Event::Progress {
        tool,
        status: "FFmpeg installed".to_string(),
        fraction: Some(base + span),
    });
    Ok(())
}

fn unpack_ffmpeg(archive: &Path, dir: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|err| format!("Cannot read the FFmpeg zip: {err}"))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|err| format!("The FFmpeg download is not a valid zip: {err}"))?;

    let wanted = [
        ("bin/ffmpeg.exe", "ffmpeg.exe"),
        ("bin/ffprobe.exe", "ffprobe.exe"),
    ];
    let mut stored: Vec<&str> = Vec::new();

    for index in 0..zip.len() {
        let mut entry = match zip.by_index(index) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let name = match entry.name() {
            Ok(name) => name.replace('\\', "/"),
            Err(_) => continue,
        };
        let Some((_, target)) = wanted.iter().find(|(suffix, _)| name.ends_with(suffix)) else {
            continue;
        };

        let path = dir.join(format!(".{target}.unpack"));
        let mut out =
            File::create(&path).map_err(|err| format!("Cannot unpack {target}: {err}"))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|err| format!("Cannot unpack {target}: {err}"))?;
        out.flush().ok();
        drop(out);
        replace_file(&path, &dir.join(target))?;
        stored.push(target);
    }

    if !stored.contains(&"ffmpeg.exe") {
        return Err("The FFmpeg archive did not contain ffmpeg.exe.".to_string());
    }
    Ok(())
}

/// Resolves the current spotDL Windows build from the GitHub release metadata.
fn spotdl_url(agent: &ureq::Agent) -> Option<String> {
    let body = agent
        .get(SPOTDL_RELEASE_API)
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    spotdl_asset(&body)
}

/// Picks the Windows executable out of a GitHub release API payload.
fn spotdl_asset(body: &str) -> Option<String> {
    let mut found = None;
    for part in body.split("\"browser_download_url\"").skip(1) {
        let Some(start) = part.find("https://") else {
            continue;
        };
        let rest = &part[start..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        let url = &rest[..end];
        let lower = url.to_lowercase();
        if lower.ends_with(".exe") && lower.contains("win") {
            found = Some(url.to_string());
            if lower.contains("win32") || lower.contains("win64") {
                break;
            }
        }
    }
    found
}

fn replace_file(temporary: &Path, target: &Path) -> Result<(), String> {
    let _ = fs::remove_file(target);
    fs::rename(temporary, target)
        .or_else(|_| {
            // Rename fails across volumes; fall back to a copy.
            fs::copy(temporary, target).map(|_| {
                let _ = fs::remove_file(temporary);
            })
        })
        .map_err(|err| format!("Cannot save {}: {err}", target.display()))
}

fn writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".br1-write-test");
    match File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn bytes_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn size_text(written: u64, total: Option<u64>) -> String {
    match total {
        Some(total) => format!(
            "{} of {} ({:.0}%)",
            bytes_text(written),
            bytes_text(total),
            (written as f32 / total.max(1) as f32 * 100.0).min(100.0)
        ),
        None => bytes_text(written),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(bytes_text(512), "512 B");
        assert_eq!(bytes_text(1536), "1.5 KB");
        assert_eq!(size_text(1024, Some(2048)), "1.0 KB of 2.0 KB (50%)");
    }

    #[test]
    fn picks_the_windows_spotdl_asset() {
        let json = r#"{
            "assets": [
                { "name": "spotDL", "browser_download_url": "https://example.com/spotDL" },
                { "name": "spotdl-4.6.0-darwin", "browser_download_url": "https://example.com/spotdl-4.6.0-darwin" },
                { "name": "spotdl-4.6.0-linux", "browser_download_url": "https://example.com/spotdl-4.6.0-linux" },
                { "name": "spotdl-4.6.0-win32.exe", "browser_download_url": "https://example.com/spotdl-4.6.0-win32.exe" }
            ]
        }"#;

        assert_eq!(
            spotdl_asset(json).as_deref(),
            Some("https://example.com/spotdl-4.6.0-win32.exe")
        );
        assert_eq!(spotdl_asset(r#"{"assets": []}"#), None);
    }

    #[test]
    #[ignore = "downloads ~190 MB from the internet"]
    fn installs_the_real_tools() {
        let dir = std::env::temp_dir().join("br1-deps-install-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let (sender, _receiver) = channel();
        let agent = agent();

        let url = spotdl_url(&agent).expect("spotdl release url");
        assert!(
            url.to_lowercase().ends_with(".exe"),
            "unexpected spotdl url: {url}"
        );

        download_tool(
            &agent,
            YT_DLP_URL,
            &dir,
            "yt-dlp.exe",
            "yt-dlp",
            &sender,
            0,
            0.0,
            0.2,
        )
        .unwrap();
        download_tool(
            &agent,
            &url,
            &dir,
            "spotdl.exe",
            "spotdl",
            &sender,
            1,
            0.2,
            0.4,
        )
        .unwrap();
        install_ffmpeg(&agent, &dir, &sender, 2, 0.6, 0.4).unwrap();

        for file in ["yt-dlp.exe", "spotdl.exe", "ffmpeg.exe", "ffprobe.exe"] {
            let path = dir.join(file);
            assert!(path.is_file(), "{file} was not installed");
            assert!(
                fs::metadata(&path).unwrap().len() > 100_000,
                "{file} looks truncated"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
