use std::path::PathBuf;
use std::process::Command;

/// Finds the Windows SDK resource compiler.
fn find_rc() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("RC_EXE") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let base = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("x64").join("rc.exe");
            if !candidate.is_file() {
                continue;
            }
            let version: Vec<u32> = entry
                .file_name()
                .to_string_lossy()
                .split('.')
                .filter_map(|part| part.parse().ok())
                .collect();
            if best
                .as_ref()
                .map(|(current, _)| version > *current)
                .unwrap_or(true)
            {
                best = Some((version, candidate));
            }
        }
    }

    best.map(|(_, path)| path)
}

fn version_info() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = version.split('.').collect();
    let major = parts.first().and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);

    format!(
        "1 VERSIONINFO\n\
         FILEVERSION {major},{minor},{patch},0\n\
         PRODUCTVERSION {major},{minor},{patch},0\n\
         FILEOS 0x40004L\n\
         FILETYPE 0x1L\n\
         BEGIN\n\
           BLOCK \"StringFileInfo\"\n\
           BEGIN\n\
             BLOCK \"040904B0\"\n\
             BEGIN\n\
               VALUE \"CompanyName\", \"BR1\"\n\
               VALUE \"FileDescription\", \"BR1 Player\"\n\
               VALUE \"FileVersion\", \"{version}\"\n\
               VALUE \"InternalName\", \"br1_player\"\n\
               VALUE \"OriginalFilename\", \"BR1 Player.exe\"\n\
               VALUE \"ProductName\", \"BR1 Player\"\n\
               VALUE \"ProductVersion\", \"{version}\"\n\
             END\n\
           END\n\
           BLOCK \"VarFileInfo\"\n\
           BEGIN\n\
             VALUE \"Translation\", 0x409, 1200\n\
           END\n\
         END\n"
    )
}

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/icon.png");

    #[cfg(windows)]
    {
        let icon = match std::fs::canonicalize("assets/icon.ico") {
            Ok(path) => path,
            Err(_) => {
                println!("cargo:warning=assets/icon.ico not found, building without an icon");
                return;
            }
        };

        let Some(rc) = find_rc() else {
            println!("cargo:warning=rc.exe (Windows SDK) not found, building without an icon");
            return;
        };

        let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        let script = out_dir.join("br1_player.rc");

        // The .rc lives in OUT_DIR so the icon path is resolved absolutely.
        let icon_path = icon.display().to_string().replace('\\', "\\\\");
        let contents = format!("1 ICON \"{icon_path}\"\n{}", version_info());
        if std::fs::write(&script, contents).is_err() {
            println!("cargo:warning=could not write the resource script");
            return;
        }

        let resource = out_dir.join("br1_player.res");
        let status = Command::new(&rc)
            .arg("/nologo")
            .arg("/fo")
            .arg(&resource)
            .arg(&script)
            .status();

        match status {
            Ok(status) if status.success() => {
                println!("cargo:rustc-link-arg={}", resource.display());
            }
            _ => println!("cargo:warning=rc.exe failed, building without an icon"),
        }
    }
}
