use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    YouTube,
    Spotify,
    Direct,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::YouTube => "YouTube",
            Source::Spotify => "Spotify",
            Source::Direct => "Direct link",
        }
    }
}

pub fn detect(url: &str) -> Source {
    let lower = url.trim().to_lowercase();
    if lower.contains("spotify.com") || lower.starts_with("spotify:") {
        Source::Spotify
    } else if lower.contains("youtube.com") || lower.contains("youtu.be") {
        Source::YouTube
    } else {
        Source::Direct
    }
}

/// True when the link points at a playlist, album, channel or artist page.
pub fn is_list_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    lower.contains("list=")
        || lower.contains("/playlist")
        || lower.contains("/album")
        || lower.contains("/sets/")
        || lower.contains("/artist")
        || lower.contains("/channel/")
        || lower.contains("/user/")
        || lower.contains("/@")
        || lower.contains("/c/")
}

/// Folder that holds the bundled yt-dlp / spotdl / ffmpeg binaries.
pub fn tools_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("bin"));
            candidates.push(dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("bin"));
    }

    candidates
        .into_iter()
        .find(|dir| dir.join("yt-dlp.exe").is_file())
}

fn tool(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = dir.join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("{name} is missing from the bin folder."))
    }
}

pub enum Message {
    Line(String),
    Finished { code: Option<i32>, cancelled: bool },
}

pub struct Handle {
    pub receiver: Receiver<Message>,
    pub cancel: Arc<AtomicBool>,
    pub pid: u32,
}

pub fn sanitize_folder(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .map(|ch| {
            if matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                || (ch as u32) < 32
            {
                '_'
            } else {
                ch
            }
        })
        .collect();

    cleaned = cleaned.trim().trim_end_matches('.').trim().to_string();
    if cleaned.chars().count() > 80 {
        cleaned = cleaned
            .chars()
            .take(80)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    if cleaned.is_empty() {
        cleaned = "Playlist".to_string();
    }
    cleaned
}

/// Reads the percentage out of `[download]  45.2% of 3.7MiB at 2.5MiB/s ETA 00:05`.
pub fn parse_percent(line: &str) -> Option<(f32, String)> {
    if !line.starts_with("[download]") {
        return None;
    }

    let percent_index = line.find('%')?;
    let token = line[..percent_index].split_whitespace().last()?;
    let value: f32 = token.parse().ok()?;
    if !(0.0..=100.0).contains(&value) {
        return None;
    }

    let detail = match line.find(" of ") {
        Some(index) => line[index + 4..].trim().to_string(),
        None => String::new(),
    };

    Some((value / 100.0, detail))
}

pub fn parse_resolve_line(line: &str) -> Option<(String, String, String)> {
    let mut parts = line.splitn(3, '|');
    let playlist = parts.next()?.trim().to_string();
    let url = parts.next()?.trim().to_string();
    let title = parts.next().unwrap_or_default().trim().to_string();

    if url.is_empty() || url == "NA" {
        return None;
    }
    Some((playlist, url, title))
}

pub fn parse_found_songs(line: &str) -> Option<usize> {
    let idx = line.find("Found ")?;
    let rest = &line[idx + 6..];
    let count: usize = rest.split_whitespace().next()?.parse().ok()?;
    if rest.contains("song") {
        Some(count)
    } else {
        None
    }
}

/* ---------- commands ---------- */

pub fn yt_dlp_resolve(url: &str) -> Result<Handle, String> {
    let dir = tools_dir().ok_or_else(missing_tools)?;
    let program = tool(&dir, "yt-dlp.exe")?;

    let mut args = vec![
        "--flat-playlist".to_string(),
        "--ignore-errors".to_string(),
        "--no-warnings".to_string(),
        "--print".to_string(),
        "%(playlist_title)s|%(url)s|%(title)s".to_string(),
    ];

    // Largest playlist artwork first; YouTube only signs some of the sizes,
    // so the caller retries down the list.
    for index in ["-1", "-2", "-3", "-4"] {
        args.push("--print".to_string());
        args.push(format!("playlist:%(thumbnails.{index}.url)s"));
    }

    args.push(url.to_string());

    spawn(program, args, Some(dir), None)
}

pub fn yt_dlp_track(
    url: &str,
    root: &Path,
    subfolder: Option<&str>,
    quality: u16,
) -> Result<Handle, String> {
    let dir = tools_dir().ok_or_else(missing_tools)?;
    let program = tool(&dir, "yt-dlp.exe")?;

    let template = match subfolder {
        Some(name) => format!("{}/%(title)s.%(ext)s", sanitize_folder(name)),
        None => "%(title)s.%(ext)s".to_string(),
    };

    let args = vec![
        "--newline".to_string(),
        "--no-warnings".to_string(),
        "--ignore-errors".to_string(),
        "--no-playlist".to_string(),
        "--concurrent-fragments".to_string(),
        "4".to_string(),
        "--extract-audio".to_string(),
        "--audio-format".to_string(),
        "mp3".to_string(),
        "--audio-quality".to_string(),
        format!("{quality}K"),
        "--embed-thumbnail".to_string(),
        "--convert-thumbnails".to_string(),
        "jpg".to_string(),
        "--ppa".to_string(),
        "ThumbnailsConvertor+ffmpeg_o:-vf scale=640:640:force_original_aspect_ratio=decrease -q:v 6"
            .to_string(),
        "--add-metadata".to_string(),
        "--windows-filenames".to_string(),
        "--ffmpeg-location".to_string(),
        dir.display().to_string(),
        "-P".to_string(),
        root.display().to_string(),
        "-o".to_string(),
        template,
        url.to_string(),
    ];

    spawn(program, args, Some(dir), Some(root.to_path_buf()))
}

pub fn spotdl(
    url: &str,
    root: &Path,
    quality: u16,
    threads: usize,
    grouped: bool,
) -> Result<Handle, String> {
    let dir = tools_dir().ok_or_else(missing_tools)?;
    let program = tool(&dir, "spotdl.exe")?;
    let ffmpeg = tool(&dir, "ffmpeg.exe")?;

    let output = if grouped {
        root.join("{list-name}")
            .join("{artist} - {title}.{output-ext}")
    } else {
        root.join("{artist} - {title}.{output-ext}")
    };

    let args = vec![
        "download".to_string(),
        url.to_string(),
        "--output".to_string(),
        output.display().to_string(),
        "--format".to_string(),
        "mp3".to_string(),
        "--bitrate".to_string(),
        format!("{quality}k"),
        "--threads".to_string(),
        threads.max(1).to_string(),
        "--headless".to_string(),
        "--log-level".to_string(),
        "INFO".to_string(),
        "--ffmpeg".to_string(),
        ffmpeg.display().to_string(),
    ];

    spawn(program, args, Some(dir), Some(root.to_path_buf()))
}

/// Downloads the playlist's own artwork (a signed thumbnail URL) and stores it
/// resized as `<root>/<subfolder>/cover.jpg`. Blocking — call it from a thread.
/// Downloads the playlist's own artwork and stores it as
/// `<root>/<subfolder>/cover.jpg`. `urls` are tried from best to worst because
/// YouTube only signs some of the sizes. Blocking — call it from a thread.
pub fn download_playlist_cover(
    urls: &[String],
    root: &Path,
    subfolder: &str,
) -> Result<(), String> {
    let dir = tools_dir().ok_or_else(missing_tools)?;
    let yt_dlp = tool(&dir, "yt-dlp.exe")?;
    let ffmpeg = tool(&dir, "ffmpeg.exe")?;

    let folder = root.join(sanitize_folder(subfolder));
    std::fs::create_dir_all(&folder).map_err(|err| format!("Cannot create folder: {err}"))?;

    let mut source = None;
    for url in urls {
        if let Some(path) = fetch_image(&yt_dlp, &folder, url) {
            source = Some(path);
            break;
        }
    }
    let source = source.ok_or_else(|| "Playlist artwork could not be downloaded".to_string())?;

    let temporary = folder.join("cover.tmp.jpg");
    let mut ffmpeg_cmd = Command::new(&ffmpeg);
    let source_arg = source.display().to_string();
    let target_arg = temporary.display().to_string();
    ffmpeg_cmd
        .args([
            "-y",
            "-i",
            &source_arg,
            "-vf",
            "scale='min(640,iw)':'min(640,ih)':force_original_aspect_ratio=decrease",
            "-pix_fmt",
            "yuvj420p",
            "-q:v",
            "6",
            &target_arg,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        ffmpeg_cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let status = ffmpeg_cmd
        .status()
        .map_err(|err| format!("Could not resize cover: {err}"))?;
    let _ = std::fs::remove_file(&source);

    if !status.success() || !temporary.is_file() {
        return Err("Cover resize failed".to_string());
    }

    let target = folder.join("cover.jpg");
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&temporary, &target).map_err(|err| format!("Cannot save cover: {err}"))?;

    Ok(())
}

fn fetch_image(yt_dlp: &Path, folder: &Path, url: &str) -> Option<std::path::PathBuf> {
    let template = folder.join("cover-source.%(ext)s");
    let template_arg = template.display().to_string();

    let mut command = Command::new(yt_dlp);
    command
        .args(["--no-warnings", "-o", &template_arg, url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    if !command.status().ok()?.success() {
        return None;
    }

    let entries = std::fs::read_dir(folder).ok()?;
    entries
        .flatten()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_lowercase()
                .starts_with("cover-source.")
        })
        .map(|entry| entry.path())
}

/// Fallback: saves the first video's thumbnail as `<subfolder>/cover.jpg`.
pub fn yt_dlp_playlist_cover(url: &str, root: &Path, subfolder: &str) -> Result<Handle, String> {
    let dir = tools_dir().ok_or_else(missing_tools)?;
    let program = tool(&dir, "yt-dlp.exe")?;

    let template = format!("{}/cover.%(ext)s", sanitize_folder(subfolder));
    let args = vec![
        "--skip-download".to_string(),
        "--no-warnings".to_string(),
        "--write-thumbnail".to_string(),
        "--convert-thumbnails".to_string(),
        "jpg".to_string(),
        "--playlist-items".to_string(),
        "1".to_string(),
        "--ppa".to_string(),
        "ThumbnailsConvertor+ffmpeg_o:-vf scale=640:640:force_original_aspect_ratio=decrease -q:v 6"
            .to_string(),
        "-P".to_string(),
        root.display().to_string(),
        "-o".to_string(),
        template,
        url.to_string(),
    ];

    spawn(program, args, Some(dir), Some(root.to_path_buf()))
}

/* ---------- process plumbing ---------- */

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn missing_tools() -> String {
    "Download tools not found. Put yt-dlp.exe, spotdl.exe and ffmpeg.exe in a \"bin\" folder next to the player.".to_string()
}

fn spawn(
    program: PathBuf,
    args: Vec<String>,
    tools: Option<PathBuf>,
    working_dir: Option<PathBuf>,
) -> Result<Handle, String> {
    let mut command = Command::new(&program);
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(dir) = &working_dir {
        let _ = std::fs::create_dir_all(dir);
        command.current_dir(dir);
    }

    if let Some(tools) = tools {
        let mut paths = vec![tools];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            command.env("PATH", joined);
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|err| format!("Could not start {}: {err}", program.display()))?;

    let (sender, receiver) = channel::<Message>();
    let cancel = Arc::new(AtomicBool::new(false));
    let pid = child.id();

    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, sender.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, sender.clone());
    }

    let cancel_flag = cancel.clone();
    thread::spawn(move || {
        let mut cancelled = false;

        let code = loop {
            if cancel_flag.load(Ordering::Acquire) {
                cancelled = true;
                kill_tree(pid);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }

            match child.try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) => thread::sleep(Duration::from_millis(150)),
                Err(_) => break None,
            }
        };

        let _ = sender.send(Message::Finished { code, cancelled });
    });

    Ok(Handle {
        receiver,
        cancel,
        pid,
    })
}

fn spawn_reader<R: Read + Send + 'static>(stream: R, sender: Sender<Message>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buffer = Vec::new();

        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&buffer);
                    let clean = strip_ansi(text.trim_end_matches(['\r', '\n']));
                    if !clean.trim().is_empty() && sender.send(Message::Line(clean)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

#[cfg(windows)]
pub fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;

    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(not(windows))]
pub fn kill_tree(_pid: u32) {}

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            output.push(ch);
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_child_output_and_reports_exit_code() {
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        let handle = spawn(
            PathBuf::from(shell),
            vec!["/C".to_string(), "echo hello-from-child".to_string()],
            None,
            None,
        )
        .expect("child should start");

        let mut saw_line = false;
        let mut finished = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);

        while std::time::Instant::now() < deadline {
            match handle.receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(Message::Line(line)) => {
                    if line.contains("hello-from-child") {
                        saw_line = true;
                    }
                }
                Ok(Message::Finished { code, .. }) => {
                    finished = Some(code);
                    break;
                }
                Err(_) => {}
            }
        }

        assert!(saw_line, "expected the child output to be streamed");
        assert_eq!(finished, Some(Some(0)));
    }

    #[test]
    fn detects_sources() {
        assert_eq!(
            detect("https://open.spotify.com/playlist/abc"),
            Source::Spotify
        );
        assert_eq!(
            detect("https://www.youtube.com/playlist?list=abc"),
            Source::YouTube
        );
        assert_eq!(detect("https://youtu.be/abc"), Source::YouTube);
        assert_eq!(detect("https://example.com/track.mp3"), Source::Direct);
    }

    #[test]
    fn playlists_get_a_subfolder_but_single_tracks_do_not() {
        assert!(is_list_url("https://www.youtube.com/playlist?list=PL123"));
        assert!(is_list_url(
            "https://www.youtube.com/watch?v=abc&list=PL123"
        ));
        assert!(is_list_url("https://music.youtube.com/playlist?list=OLAK5"));
        assert!(is_list_url("https://open.spotify.com/album/abc"));
        assert!(is_list_url("https://www.youtube.com/@SomeChannel"));

        assert!(!is_list_url("https://www.youtube.com/watch?v=abc"));
        assert!(!is_list_url("https://youtu.be/abc"));
        assert!(!is_list_url("https://open.spotify.com/track/abc"));
    }

    #[test]
    fn strips_terminal_colours() {
        assert_eq!(strip_ansi("\u{1b}[32mgreen\u{1b}[0m text"), "green text");
    }

    #[test]
    fn reads_download_percentages() {
        let (fraction, detail) =
            parse_percent("[download]  45.2% of  3.76MiB at    2.59MiB/s ETA 00:05").unwrap();
        assert!((fraction - 0.452).abs() < 0.001);
        assert_eq!(detail, "3.76MiB at    2.59MiB/s ETA 00:05");

        let (fraction, _) = parse_percent("[download] 100% of 3.76MiB in 00:00:01").unwrap();
        assert!((fraction - 1.0).abs() < 0.001);

        assert!(parse_percent("[download] Destination: song.webm").is_none());
        assert!(parse_percent("[youtube] 45% of something").is_none());
    }

    #[test]
    fn reads_resolve_lines() {
        let (playlist, url, title) = parse_resolve_line(
            "Popular Music Videos|https://www.youtube.com/watch?v=fOT0BUpITw8|BELLAKEO",
        )
        .unwrap();
        assert_eq!(playlist, "Popular Music Videos");
        assert_eq!(url, "https://www.youtube.com/watch?v=fOT0BUpITw8");
        assert_eq!(title, "BELLAKEO");

        assert!(parse_resolve_line("NA|NA|NA").is_none());
    }

    #[test]
    fn sanitizes_folder_names() {
        assert_eq!(sanitize_folder("AC/DC: Live?"), "AC_DC_ Live_");
        assert_eq!(sanitize_folder("  mixed \\ hits | "), "mixed _ hits _");
        assert_eq!(sanitize_folder(""), "Playlist");
    }
}
