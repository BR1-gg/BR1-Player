//! One-time installation of the external assets the app needs.
//!
//! The player itself is a single executable. On the very first launch — tracked
//! by a marker in `%APPDATA%\BR1 Player` — it downloads:
//!
//! * `yt-dlp.exe`, `spotdl.exe`, `ffmpeg.exe` and `ffprobe.exe`, into a stable
//!   `bin` folder under `%LOCALAPPDATA%\BR1 Player` (or next to the exe for
//!   portable installs), and
//! * two measured MIT KEMAR SOFA HRTF sets into `%APPDATA%\BR1 Player\hrtf`.
//!
//! The loading screen shows the progress, and nothing is downloaded again on
//! later starts — the tools folder is remembered in `%APPDATA%\BR1 Player`, so
//! even an app update into a different folder reuses the same binaries.
//! Settings → Downloads has a button that can re-run the same installer if a
//! user later removes a file.

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

/// Row used by the HRTF downloads in the loading screen.
const HRTF_ROW: usize = TOOLS.len();
/// Loading-screen rows: three tools plus one combined HRTF row.
const ROWS: usize = TOOLS.len() + 1;

const KEMAR_NORMAL_URL: &str =
    "https://sofacoustics.org/data/database/mit/mit_kemar_normal_pinna.sofa";
const KEMAR_LARGE_URL: &str =
    "https://sofacoustics.org/data/database/mit/mit_kemar_large_pinna.sofa";

/// Target file name, loading-screen label, weight in MB and source URL.
const HRTF_ASSETS: [(&str, &str, f32, &str); 2] = [
    (
        "MIT_KEMAR_normal.sofa",
        "MIT KEMAR normal pinna",
        1.2,
        KEMAR_NORMAL_URL,
    ),
    (
        "MIT_KEMAR_large.sofa",
        "MIT KEMAR large pinna",
        1.2,
        KEMAR_LARGE_URL,
    ),
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
    /// `tool` is a loading-screen row: 0..3 are the tools, 3 is the combined
    /// HRTF row. `fraction` is the overall progress (`None` while the size is
    /// unknown, e.g. while unpacking the zip).
    Progress {
        tool: usize,
        status: String,
        fraction: Option<f32>,
    },
    Finished(Result<(), String>),
}

/// Tool indices that are missing from the folder the player loads them from.
pub fn missing_tools() -> Vec<usize> {
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
pub fn tools_ready() -> bool {
    missing_tools().is_empty()
}

/// HRTF files that are missing (or not valid SOFA/HDF5 files).
fn missing_hrtf() -> Vec<usize> {
    let dir = crate::spatial::hrtf_folder();
    HRTF_ASSETS
        .iter()
        .enumerate()
        .filter(|(_, (file, _, _, _))| !is_sofa_file(&dir.join(file)))
        .map(|(index, _)| index)
        .collect()
}

/// Quick sanity check so an HTML error page is never treated as HRTF data.
fn is_sofa_file(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut header = [0u8; 8];
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    header == *b"\x89HDF\r\n\x1a\n" || header.starts_with(b"CDF")
}

/// True when both measured KEMAR sets are present.
pub fn hrtf_ready() -> bool {
    missing_hrtf().is_empty()
}

/// True when every tool and both HRTF sets are present.
pub fn assets_ready() -> bool {
    tools_ready() && hrtf_ready()
}

/// Marker written after the first-run installer completes (or is skipped).
fn marker_path() -> Option<PathBuf> {
    crate::config::config_path().map(|path| path.with_file_name("first_run_complete.txt"))
}

/// True until the first-run installer has completed once on this machine.
pub fn needs_first_run() -> bool {
    marker_path().map(|path| !path.is_file()).unwrap_or(true)
}

/// Record that the first-run installer has finished, so it is not run
/// automatically again. The user can still re-run it from Settings.
pub fn mark_first_run_complete() -> Result<(), String> {
    let path = marker_path().ok_or_else(|| "Cannot find the APPDATA folder.".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Cannot create {}: {err}", parent.display()))?;
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    fs::write(
        &path,
        format!(
            "BR1 Player first-run setup\nversion {}\ncompleted {timestamp}\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .map_err(|err| format!("Cannot write {}: {err}", path.display()))
}

/// Label of a loading-screen row.
pub fn label(index: usize) -> &'static str {
    if index < TOOLS.len() {
        TOOLS[index].1
    } else {
        "HRTF sets"
    }
}

/// Number of rows the loading screen lists.
pub fn count() -> usize {
    ROWS
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
    let hrtf_dir = crate::spatial::hrtf_folder();
    fs::create_dir_all(&hrtf_dir)
        .map_err(|err| format!("Cannot create {}: {err}", hrtf_dir.display()))?;

    let pending_tools = missing_tools();
    let pending_hrtf = missing_hrtf();
    let hrtf_weight: f32 = pending_hrtf
        .iter()
        .map(|index| HRTF_ASSETS[*index].2)
        .sum();
    let total: f32 = pending_tools
        .iter()
        .map(|index| TOOLS[*index].2)
        .sum::<f32>()
        + hrtf_weight;
    let total = total.max(1.0);

    let agent = agent();
    let mut done_weight = 0.0f32;

    for index in pending_tools {
        let span = TOOLS[index].2 / total;
        let (file, label, _) = TOOLS[index];

        match file {
            "ffmpeg.exe" => install_ffmpeg(&agent, &dir, sender, index, done_weight, span)?,
            "spotdl.exe" => {
                let url = spotdl_url(&agent).unwrap_or_else(|| SPOTDL_FALLBACK_URL.to_string());
                download_file(
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
                download_file(
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

    if !pending_hrtf.is_empty() {
        let row_span = hrtf_weight / total;
        let mut sub_done = 0.0f32;
        for index in pending_hrtf {
            let (file, label, weight, url) = HRTF_ASSETS[index];
            let span = weight / total;
            download_file(
                &agent,
                url,
                &hrtf_dir,
                file,
                label,
                sender,
                HRTF_ROW,
                done_weight + sub_done,
                span,
            )?;
            let path = hrtf_dir.join(file);
            if !is_sofa_file(&path) {
                let _ = fs::remove_file(&path);
                return Err(format!(
                    "The {label} download is not a valid SOFA file."
                ));
            }
            sub_done += span;
        }
        write_hrtf_notice(&hrtf_dir);
        let _ = sender.send(Event::Progress {
            tool: HRTF_ROW,
            status: "HRTF sets installed".to_string(),
            fraction: Some(done_weight + row_span),
        });
    }

    let _ = sender.send(Event::Progress {
        tool: ROWS,
        status: "All set — starting BR1 Player…".to_string(),
        fraction: Some(1.0),
    });
    // Remember where the tools live so later updates (which may run from a
    // different folder) find them again without re-downloading.
    crate::download::remember_tools_dir(&dir);
    Ok(())
}

/// Attribution next to the downloaded measured HRTFs.
fn write_hrtf_notice(dir: &Path) {
    let text = "\
MIT KEMAR HRTF data
===================

Downloaded from the SOFA database:
  https://sofacoustics.org/data/database/mit/

Please cite:
  W. G. Gardner and K. D. Martin, \"HRTF measurements of a KEMAR\",
  Journal of the Acoustical Society of America 97(6), 3907-3908 (1995).

The files are used locally by BR1 Player for binaural headphone rendering.
";
    let _ = fs::write(dir.join("ATTRIBUTION.txt"), text);
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

/// Downloads a file straight into `dir` (an executable or a SOFA file).
#[allow(clippy::too_many_arguments)]
fn download_file(
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
    fn loading_screen_rows() {
        assert_eq!(count(), 4);
        assert_eq!(label(0), "yt-dlp");
        assert_eq!(label(1), "spotdl");
        assert_eq!(label(2), "FFmpeg");
        assert_eq!(label(3), "HRTF sets");
    }

    #[test]
    #[ignore = "downloads ~2.3 MB from sofacoustics.org"]
    fn downloads_the_kemar_sets() {
        let dir = std::env::temp_dir().join("br1-kemar-download-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let (sender, _receiver) = channel();
        let agent = agent();
        for (index, (file, label, _, url)) in HRTF_ASSETS.iter().enumerate() {
            download_file(
                &agent,
                url,
                &dir,
                file,
                label,
                &sender,
                HRTF_ROW,
                0.0,
                (index + 1) as f32 * 0.5,
            )
            .unwrap();
            let path = dir.join(file);
            assert!(path.is_file(), "{file} was not installed");
            assert!(
                fs::metadata(&path).unwrap().len() > 1_000_000,
                "{file} looks truncated"
            );
        }

        let _ = fs::remove_dir_all(&dir);
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

        download_file(
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
        download_file(
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
