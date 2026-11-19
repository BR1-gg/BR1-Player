use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::download::{self, Handle, Message, Source};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_active(self) -> bool {
        matches!(self, JobState::Queued | JobState::Running)
    }
}

pub struct Job {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub source: Source,
    pub state: JobState,
    /// 0..1, `None` while the size is unknown (bar shows activity instead).
    pub progress: Option<f32>,
    pub detail: String,
    pub error: Option<String>,
    pub queue_index: usize,
    /// 1-based position on the resolved YouTube playlist page; 0 = unknown.
    position: u64,
    resolve: bool,
    subfolder: Option<String>,
}

pub struct Settings {
    pub root: PathBuf,
    pub quality: u16,
    pub parallel: usize,
}

enum Event {
    Progress {
        id: u64,
        fraction: Option<f32>,
        detail: String,
    },
    Title {
        id: u64,
        title: String,
    },
    Line {
        id: u64,
        line: String,
    },
    Expanded {
        id: u64,
        playlist: Option<String>,
        covers: Vec<String>,
        items: Vec<(String, String, u64)>,
    },
    Finished {
        id: u64,
        code: Option<i32>,
        cancelled: bool,
        error: Option<String>,
    },
}

struct RunningJob {
    id: u64,
    cancel: Arc<AtomicBool>,
    pid: u32,
}

pub struct Manager {
    jobs: Vec<Job>,
    next_id: u64,
    running: Vec<RunningJob>,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    pub settings: Settings,
    pub log: Vec<String>,
    rescan_pending: bool,
}

impl Manager {
    pub fn new(settings: Settings) -> Self {
        let (sender, receiver) = channel();
        Self {
            jobs: Vec::new(),
            next_id: 1,
            running: Vec::new(),
            sender,
            receiver,
            settings,
            log: Vec::new(),
            rescan_pending: false,
        }
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn queued_or_running(&self) -> usize {
        self.jobs.iter().filter(|job| job.state.is_active()).count()
    }

    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// Adds a link to the queue. Playlists are resolved into one job per track.
    pub fn submit(&mut self, url: String) -> u64 {
        let source = download::detect(&url);
        let resolve = source != Source::Spotify && download::is_list_url(&url);
        let id = self.next_id;
        self.next_id += 1;

        let title = if resolve {
            "Reading playlist…".to_string()
        } else if download::is_list_url(&url) {
            format!("{} playlist", source.label())
        } else {
            "New track".to_string()
        };

        let queue_index = self.jobs.len();
        self.jobs.push(Job {
            id,
            title,
            url,
            source,
            state: JobState::Queued,
            progress: None,
            detail: if resolve {
                "Looking up tracks".to_string()
            } else {
                "Queued".to_string()
            },
            error: None,
            queue_index,
            position: 0,
            resolve,
            subfolder: None,
        });

        id
    }

    pub fn cancel(&mut self, id: u64) {
        if let Some(job) = self.running.iter().find(|job| job.id == id) {
            job.cancel.store(true, Ordering::Release);
        }
        if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
            if job.state == JobState::Queued {
                job.state = JobState::Cancelled;
                job.detail = "Cancelled".to_string();
            } else if job.state == JobState::Running {
                job.detail = "Cancelling…".to_string();
            }
        }
    }

    pub fn cancel_all(&mut self) {
        let ids: Vec<u64> = self
            .jobs
            .iter()
            .filter(|job| job.state.is_active())
            .map(|job| job.id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Kills every running process tree right away (used when the window closes).
    pub fn kill_all_now(&mut self) {
        for running in &self.running {
            running.cancel.store(true, Ordering::Release);
            download::kill_tree(running.pid);
        }
        self.running.clear();
    }

    pub fn clear_finished(&mut self) {
        self.jobs.retain(|job| job.state.is_active());
        for (index, job) in self.jobs.iter_mut().enumerate() {
            job.queue_index = index;
        }
    }

    /// True once everything finished — the app then rescans the music folder.
    pub fn take_rescan(&mut self) -> bool {
        let idle = self.running.is_empty() && !self.jobs.iter().any(|job| job.state.is_active());
        if self.rescan_pending && idle {
            self.rescan_pending = false;
            true
        } else {
            false
        }
    }

    pub fn poll(&mut self) {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => self.apply(event),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        self.schedule();
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Progress {
                id,
                fraction,
                detail,
            } => {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    if job.state == JobState::Queued {
                        job.state = JobState::Running;
                    }
                    if fraction.is_some() {
                        job.progress = fraction;
                    }
                    if !detail.is_empty() {
                        job.detail = detail;
                    }
                }
            }
            Event::Title { id, title } => {
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    if !title.trim().is_empty() {
                        job.title = title;
                    }
                }
            }
            Event::Line { id, line } => {
                self.log.push(line);
                if self.log.len() > 200 {
                    self.log.remove(0);
                }
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    if job.state == JobState::Queued {
                        job.state = JobState::Running;
                    }
                }
            }
            Event::Expanded {
                id,
                playlist,
                covers,
                items,
            } => {
                let parent_url = self
                    .jobs
                    .iter()
                    .find(|job| job.id == id)
                    .map(|job| job.url.clone())
                    .unwrap_or_default();

                let subfolder = playlist
                    .as_deref()
                    .filter(|name| !name.is_empty() && *name != "NA")
                    .map(download::sanitize_folder);

                let mut children = Vec::new();
                if !items.is_empty() {
                    for (url, title, position) in items {
                        let child_id = self.next_id;
                        self.next_id += 1;
                        children.push(Job {
                            id: child_id,
                            title,
                            url,
                            source: Source::YouTube,
                            state: JobState::Queued,
                            progress: None,
                            detail: "Queued".to_string(),
                            error: None,
                            queue_index: 0,
                            position,
                            resolve: false,
                            subfolder: subfolder.clone(),
                        });
                    }
                }

                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    job.state = JobState::Done;
                    job.progress = Some(1.0);
                    job.detail = if children.is_empty() {
                        "No tracks found".to_string()
                    } else {
                        format!("{} tracks queued", children.len())
                    };
                    job.title = match playlist {
                        Some(name) if !name.is_empty() && name != "NA" => name,
                        _ => job.title.clone(),
                    };
                }

                for child in children {
                    self.jobs.push(child);
                }
                for (index, job) in self.jobs.iter_mut().enumerate() {
                    job.queue_index = index;
                }

                // Grab the playlist's own cover into its folder.
                if let Some(name) = subfolder {
                    let root = self.settings.root.clone();
                    let url = parent_url;
                    thread::spawn(move || {
                        let _ = std::fs::create_dir_all(root.join(&name));
                        if !covers.is_empty()
                            && download::download_playlist_cover(&covers, &root, &name).is_ok()
                        {
                            return;
                        }
                        if let Ok(handle) = download::yt_dlp_playlist_cover(&url, &root, &name) {
                            while handle.receiver.recv().is_ok() {}
                        }
                    });
                }
            }
            Event::Finished {
                id,
                code,
                cancelled,
                error,
            } => {
                self.running.retain(|running| running.id != id);
                if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                    if job.state == JobState::Done {
                        // Playlist resolve already reported success.
                    } else if cancelled {
                        job.state = JobState::Cancelled;
                        job.detail = "Cancelled".to_string();
                    } else if let Some(message) = error {
                        job.state = JobState::Failed;
                        job.error = Some(message.clone());
                        job.detail = message;
                    } else if code == Some(0) {
                        job.state = JobState::Done;
                        job.progress = Some(1.0);
                        job.detail = "Done".to_string();
                    } else {
                        job.state = JobState::Failed;
                        job.detail = format!("Exited with code {:?}", code);
                        job.error = Some(job.detail.clone());
                    }
                    if !matches!(job.state, JobState::Failed) {
                        self.rescan_pending = true;
                    }
                }
            }
        }
    }

    fn schedule(&mut self) {
        let limit = self.settings.parallel.max(1);

        while self.running.len() < limit {
            let Some(id) = self
                .jobs
                .iter()
                .find(|job| job.state == JobState::Queued)
                .map(|job| job.id)
            else {
                break;
            };

            match self.start_job(id) {
                Ok(job) => self.running.push(job),
                Err(err) => {
                    if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
                        job.state = JobState::Failed;
                        job.error = Some(err.clone());
                        job.detail = err;
                    }
                }
            }
        }
    }

    fn start_job(&mut self, id: u64) -> Result<RunningJob, String> {
        let job = self
            .jobs
            .iter()
            .find(|job| job.id == id)
            .ok_or_else(|| "Job vanished".to_string())?;

        let handle: Handle = if job.resolve {
            download::yt_dlp_resolve(&job.url)?
        } else {
            match job.source {
                Source::Spotify => download::spotdl(
                    &job.url,
                    &self.settings.root,
                    self.settings.quality,
                    self.settings.parallel,
                    download::is_list_url(&job.url),
                )?,
                _ => download::yt_dlp_track(
                    &job.url,
                    &self.settings.root,
                    job.subfolder.as_deref(),
                    self.settings.quality,
                )?,
            }
        };

        let cancel = handle.cancel.clone();
        let sender = self.sender.clone();
        let source = job.source;
        let resolve = job.resolve;
        let position = job.position;

        if let Some(job) = self.jobs.iter_mut().find(|job| job.id == id) {
            job.state = JobState::Running;
            job.detail = if resolve {
                "Reading playlist…".to_string()
            } else {
                "Starting…".to_string()
            };
        }

        let pid = handle.pid;
        thread::spawn(move || run_job(id, handle, source, resolve, position, sender));
        Ok(RunningJob { id, cancel, pid })
    }
}

fn run_job(
    id: u64,
    handle: Handle,
    source: Source,
    resolve: bool,
    position: u64,
    sender: Sender<Event>,
) {
    let mut entries: Vec<(String, String, u64)> = Vec::new();
    let mut playlist: Option<String> = None;
    let mut cover_urls: Vec<String> = Vec::new();
    let mut total_songs: Option<usize> = None;
    let mut done_songs: usize = 0;
    let mut last_error: Option<String> = None;
    // Filled in while a yt-dlp track download runs, then used to stamp the
    // file with the real upload date (or the playlist position as fallback).
    let mut destination: Option<PathBuf> = None;
    let mut upload_stamp: Option<u64> = None;

    while let Ok(message) = handle.receiver.recv() {
        match message {
            Message::Line(line) => {
                if resolve {
                    if let Some(url) = cover_url_from_line(&line) {
                        cover_urls.push(url);
                        continue;
                    }
                    if let Some((name, url, title, position)) = download::parse_resolve_line(&line)
                    {
                        if playlist.is_none() {
                            playlist = Some(name);
                        }
                        entries.push((url, title, position));
                        let _ = sender.send(Event::Progress {
                            id,
                            fraction: None,
                            detail: format!("{} tracks found…", entries.len()),
                        });
                    } else if line.to_lowercase().contains("error") {
                        last_error = Some(line);
                    }
                    continue;
                }

                let _ = sender.send(Event::Line {
                    id,
                    line: line.clone(),
                });

                if let Some(title) = title_from_line(&line) {
                    let _ = sender.send(Event::Title { id, title });
                }

                if source != Source::Spotify {
                    // yt-dlp reports the final file and the real upload date
                    // while it works, no extra requests involved.
                    if let Some(path) = download::destination_from_line(&line) {
                        destination = Some(path);
                    }
                    if let Some(stamp) = download::upload_date_from_line(&line) {
                        upload_stamp = Some(stamp);
                    }
                }

                if let Some(total) = download::parse_found_songs(&line) {
                    total_songs = Some(total);
                }
                if line.contains("Downloaded \"") {
                    done_songs += 1;
                }
                if line.to_lowercase().contains("error") {
                    last_error = Some(line.clone());
                }

                if let Some((fraction, detail)) = download::parse_percent(&line) {
                    let _ = sender.send(Event::Progress {
                        id,
                        fraction: Some(fraction),
                        detail,
                    });
                } else if source == Source::Spotify && (total_songs.is_some() || done_songs > 0) {
                    let fraction = total_songs.map(|total| {
                        if total == 0 {
                            0.0
                        } else {
                            done_songs as f32 / total as f32
                        }
                    });
                    let detail = match total_songs {
                        Some(total) => format!("{done_songs} of {total} songs"),
                        None => format!("{done_songs} songs downloaded"),
                    };
                    let _ = sender.send(Event::Progress {
                        id,
                        fraction,
                        detail,
                    });
                } else if let Some(detail) = stage_detail(&line) {
                    let _ = sender.send(Event::Progress {
                        id,
                        fraction: None,
                        detail,
                    });
                }
            }
            Message::Finished { code, cancelled } => {
                if resolve {
                    if !entries.is_empty() {
                        let _ = sender.send(Event::Expanded {
                            id,
                            playlist: playlist.clone(),
                            covers: cover_urls.clone(),
                            items: entries.clone(),
                        });
                    }
                    let error = if entries.is_empty() {
                        last_error.or_else(|| Some("No tracks found in that link.".to_string()))
                    } else {
                        None
                    };
                    let _ = sender.send(Event::Finished {
                        id,
                        code,
                        cancelled,
                        error,
                    });
                } else {
                    let error = if !cancelled && code != Some(0) {
                        last_error
                            .or_else(|| Some(format!("Downloader exited with code {:?}.", code)))
                    } else {
                        None
                    };

                    // Carry the YouTube upload date (or the playlist position
                    // as fallback) on the file's date, so the newest-first
                    // sort reproduces the YouTube playlist order.
                    if !cancelled && code == Some(0) {
                        if let Some(path) = &destination {
                            let stamp = upload_stamp.or_else(|| {
                                (position > 0).then(|| {
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|duration| duration.as_secs())
                                        .unwrap_or(0)
                                        .saturating_sub(position * 3600)
                                })
                            });
                            if let Some(secs) = stamp {
                                if download::set_file_time(path, secs) {
                                    let _ = sender.send(Event::Progress {
                                        id,
                                        fraction: None,
                                        detail: "Saved with its YouTube date".to_string(),
                                    });
                                }
                            }
                        }
                    }

                    let _ = sender.send(Event::Finished {
                        id,
                        code,
                        cancelled,
                        error,
                    });
                }
                return;
            }
        }
    }
}

fn cover_url_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("http") && !trimmed.contains('|') && !trimmed.contains(' ') {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn title_from_line(line: &str) -> Option<String> {
    for prefix in ["[download] Destination: ", "[ExtractAudio] Destination: "] {
        if let Some(path) = line.strip_prefix(prefix) {
            let path = path.trim();
            let stem = std::path::Path::new(path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(path);
            return Some(stem.to_string());
        }
    }

    if let Some(rest) = line.strip_prefix("Downloaded \"") {
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(rest) = line.strip_prefix("Downloading \"") {
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }

    None
}

fn stage_detail(line: &str) -> Option<String> {
    if line.starts_with("[ExtractAudio]") {
        Some("Converting to MP3…".to_string())
    } else if line.starts_with("[Metadata]") {
        Some("Writing tags…".to_string())
    } else if line.starts_with("[EmbedThumbnail]") {
        Some("Embedding cover…".to_string())
    } else if line.starts_with("[ThumbnailsConvertor]") {
        Some("Resizing cover…".to_string())
    } else if line.starts_with("[youtube]") || line.starts_with("[generic]") {
        Some("Resolving…".to_string())
    } else {
        None
    }
}
