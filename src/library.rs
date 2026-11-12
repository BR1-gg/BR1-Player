use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "mp4", "aac", "flac", "wav", "ogg", "oga", "opus", "wma", "aiff", "aif", "weba",
];

/// Small RGBA image kept in memory for list thumbnails.
pub struct CoverAsset {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f32,
    /// ~96px thumbnail, uploaded once as a texture.
    pub thumb: Option<CoverAsset>,
    /// Original artwork bytes, decoded on demand for the big cover.
    pub art: Option<Vec<u8>>,
    /// Folder the file lives in (used to find a playlist cover).
    pub folder: Option<PathBuf>,
    /// Playlist name = path relative to the music root, `None` for loose files.
    pub playlist: Option<String>,
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Expands files and folders (recursively) into a sorted list of audio files.
pub fn collect(inputs: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack: Vec<PathBuf> = inputs.to_vec();

    while let Some(path) = stack.pop() {
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        } else if is_audio(&path) {
            files.push(path);
        }
    }

    files.sort();
    files
}

pub fn scan(inputs: &[PathBuf], root: Option<&Path>) -> Vec<Track> {
    collect(inputs)
        .iter()
        .map(|path| {
            let mut track = read_track(path);
            track.playlist = root.and_then(|root| playlist_name(path, root));
            track
        })
        .collect()
}

/// Folder name relative to the music root, e.g. `Popular Music Videos`.
fn playlist_name(path: &Path, root: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parent = relative.parent()?;

    let parts: Vec<String> = parent
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(|part| part.to_string())
        .collect();

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

fn read_track(path: &Path) -> Track {
    let mut track = Track {
        path: path.to_path_buf(),
        title: path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Unknown title")
            .to_string(),
        artist: "Unknown artist".to_string(),
        album: String::new(),
        duration: 0.0,
        thumb: None,
        art: None,
        folder: path.parent().map(|parent| parent.to_path_buf()),
        playlist: None,
    };

    let Ok(tagged) = lofty::read_from_path(path) else {
        return track;
    };

    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        if let Some(title) = tag.title().filter(|value| !value.trim().is_empty()) {
            track.title = title.to_string();
        }
        if let Some(artist) = tag.artist().filter(|value| !value.trim().is_empty()) {
            track.artist = artist.to_string();
        }
        if let Some(album) = tag.album().filter(|value| !value.trim().is_empty()) {
            track.album = album.to_string();
        }

        if let Some(picture) = tag.pictures().first() {
            let data = picture.data().to_vec();
            if let Ok(image) = image::load_from_memory(&data) {
                // Square, centre-cropped so the image fills the UI box without stretching
                // and without wasting resolution on pixels that get cropped away.
                let thumb = image
                    .resize_to_fill(96, 96, image::imageops::FilterType::Triangle)
                    .to_rgba8();
                track.thumb = Some(CoverAsset {
                    width: thumb.width() as usize,
                    height: thumb.height() as usize,
                    rgba: thumb.into_raw(),
                });
                track.art = Some(data);
            }
        }
    }

    track.duration = tagged.properties().duration().as_secs_f32();
    track
}
