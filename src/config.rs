use std::path::{Path, PathBuf};

pub const DEFAULT_QUALITY: u16 = 192;
pub const DEFAULT_PARALLEL: usize = 3;

/// User settings, stored in %APPDATA%\DefPlayer\config.txt
#[derive(Clone)]
pub struct Config {
    /// Main folder: every download goes here. Playlists get an automatic subfolder.
    pub library_root: PathBuf,
    /// MP3 bitrate in kbps.
    pub quality: u16,
    /// How many downloads may run at the same time.
    pub parallel: usize,
}

impl Config {
    pub fn with_root(library_root: PathBuf) -> Self {
        Self {
            library_root,
            quality: DEFAULT_QUALITY,
            parallel: DEFAULT_PARALLEL,
        }
    }
}

pub fn default_library_root() -> PathBuf {
    let base = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Music").join("Def Player")
}

pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join("DefPlayer").join("config.txt"))
}

fn legacy_settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join("DefPlayer").join("settings.txt"))
}

pub fn load_from(path: &Path) -> Option<Config> {
    let text = std::fs::read_to_string(path).ok()?;

    let mut root = None;
    let mut quality = DEFAULT_QUALITY;
    let mut parallel = DEFAULT_PARALLEL;

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "library_root" => {
                if !value.is_empty() {
                    root = Some(PathBuf::from(value));
                }
            }
            "quality" => {
                if let Ok(parsed) = value.parse::<u16>() {
                    if (64..=320).contains(&parsed) {
                        quality = parsed;
                    }
                }
            }
            "parallel" => {
                if let Ok(parsed) = value.parse::<usize>() {
                    if (1..=12).contains(&parsed) {
                        parallel = parsed;
                    }
                }
            }
            _ => {}
        }
    }

    root.map(|library_root| Config {
        library_root,
        quality,
        parallel,
    })
}

pub fn save_to(path: &Path, config: &Config) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = format!(
        "library_root={}\nquality={}\nparallel={}\n",
        config.library_root.display(),
        config.quality,
        config.parallel
    );
    let _ = std::fs::write(path, text);
}

/// Reads the config, migrating the old single-path settings file if needed.
pub fn load() -> Option<Config> {
    if let Some(path) = config_path() {
        if path.exists() {
            return load_from(&path);
        }
    }

    let legacy = legacy_settings_path()?;
    let text = std::fs::read_to_string(legacy).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(Config::with_root(PathBuf::from(text)))
}

pub fn save(config: &Config) {
    if let Some(path) = config_path() {
        save_to(&path, config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip() {
        let dir = std::env::temp_dir().join("defplayer_config_test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.txt");

        let original = Config {
            library_root: PathBuf::from(r"D:\Music Library"),
            quality: 320,
            parallel: 6,
        };
        save_to(&path, &original);

        let loaded = load_from(&path).expect("config should load");
        assert_eq!(loaded.library_root, original.library_root);
        assert_eq!(loaded.quality, 320);
        assert_eq!(loaded.parallel, 6);
    }

    #[test]
    fn old_config_without_new_keys_uses_defaults() {
        let dir = std::env::temp_dir().join("defplayer_config_legacy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.txt");
        std::fs::write(&path, "library_root=C:\\Music\n").unwrap();

        let loaded = load_from(&path).expect("config should load");
        assert_eq!(loaded.quality, DEFAULT_QUALITY);
        assert_eq!(loaded.parallel, DEFAULT_PARALLEL);
    }
}
