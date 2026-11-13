//! `kit-fs`: Plattformübergreifende Pfadnormalisierung, App-Verzeichnisse und Dateisystem-Helfer.
//!
//! Vereint die ehemaligen Crates `kit-paths` und `kit-appdirs` sowie atomare Schreiboperationen,
//! Backups, BOM-Bereinigung, Größenberechnung und Zeitformatierung.

use std::path::{Path, PathBuf};

#[cfg(feature = "serde")]
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub mod appdirs;

/// Fehler bei Dateisystem- oder JSON-Operationen in `kit-fs`.
#[derive(Debug, thiserror::Error)]
pub enum KitFsError {
    #[error("I/O-Fehler: {0}")]
    Io(#[from] std::io::Error),
    #[cfg(feature = "serde")]
    #[error("JSON-Fehler: {0}")]
    Json(#[from] serde_json::Error),
}

/// Betriebssystem-Typ zur expliziten Steuerung von Konvertierungen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
}

impl Os {
    /// Erkennt das aktuelle Betriebssystem zur Laufzeit.
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }

    pub fn is_windows(&self) -> bool {
        matches!(self, Os::Windows)
    }

    pub fn is_unix(&self) -> bool {
        matches!(self, Os::Linux | Os::MacOs)
    }
}

/// Präfix-Mapping, z. B. `R:\` ↔ `/mnt/r/` oder `Z:\` ↔ `/shared/`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PathMapping {
    pub windows: String,
    pub unix: String,
}

impl PathMapping {
    pub fn new(windows: impl Into<String>, unix: impl Into<String>) -> Self {
        Self {
            windows: windows.into(),
            unix: unix.into(),
        }
    }
}

/// Normalisiert einen Pfad-String für Vergleiche (lowercase, Forward-Slashes).
pub fn norm(s: &str) -> String {
    s.replace('\\', "/").to_lowercase()
}

/// Gibt den übergeordneten Ordner zurück, oder den Pfad selbst, falls er bereits ein Ordner ist.
pub fn folder_or_parent(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else if let Some(parent) = path.parent() {
        if parent.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            parent.to_path_buf()
        }
    } else {
        path.to_path_buf()
    }
}

/// Wendet eine Liste von `PathMapping`s an, um einen Pfad von `os_from` nach `os_to` zu übersetzen.
pub fn map_from(os_from: Os, os_to: Os, path: &str, maps: &[PathMapping]) -> Option<String> {
    for m in maps {
        let (from, to) = match (os_from, os_to) {
            (Os::Windows, Os::Windows) | (Os::Windows, _) => (&m.windows, &m.unix),
            (_, Os::Windows) => (&m.unix, &m.windows),
            _ => continue,
        };
        let (np, nf) = (norm(path), norm(from));
        if np.starts_with(&nf) {
            let rest = &path[from.len()..];
            let rest = if os_to == Os::Windows {
                rest.replace('/', "\\")
            } else {
                rest.replace('\\', "/")
            };
            return Some(format!("{to}{rest}"));
        }
    }
    None
}

/// Versucht ohne explizites Mapping intelligente Pfad-Konvertierungen:
/// 1. Slashes umkehren (`\` <-> `/`)
/// 2. Windows-Laufwerksbuchstaben zu Unix (`R:\git\...` -> `/r/git/...`, `/mnt/r/git/...`, `/media/r/git/...`)
/// 3. Unix-Pfade zu Windows-Laufwerksbuchstaben (`/r/git/...`, `/mnt/r/git/...` -> `R:\git\...`)
pub fn auto_map_from(os_from: Os, os_to: Os, path: &str) -> Option<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }

    match (os_from, os_to) {
        (Os::Windows, Os::Linux) | (Os::Windows, Os::MacOs) => {
            let mut chars = trimmed.chars();
            let c1 = chars.next()?;
            let c2 = chars.next()?;
            if c1.is_ascii_alphabetic() && c2 == ':' {
                let drive_lower = c1.to_ascii_lowercase();
                let drive_upper = c1.to_ascii_uppercase();
                let rest = trimmed[2..].replace('\\', "/");
                let rest_clean = rest.trim_start_matches('/');

                let candidates = [
                    format!("/{drive_lower}/{rest_clean}"),
                    format!("/mnt/{drive_lower}/{rest_clean}"),
                    format!("/media/{drive_lower}/{rest_clean}"),
                    format!("/{drive_upper}/{rest_clean}"),
                ];

                for cand in &candidates {
                    if Path::new(cand).exists() {
                        return Some(PathBuf::from(cand));
                    }
                }

                return None;
            }

            let flipped = trimmed.replace('\\', "/");
            if Path::new(&flipped).exists() {
                Some(PathBuf::from(flipped))
            } else {
                None
            }
        }
        (Os::Linux, Os::Windows) | (Os::MacOs, Os::Windows) => {
            let flipped = trimmed.replace('/', "\\");
            if Path::new(&flipped).exists() {
                return Some(PathBuf::from(flipped));
            }

            let rest_after_mnt = trimmed
                .strip_prefix("/mnt/")
                .or_else(|| trimmed.strip_prefix("/media/"));
            let (drive_char, rest_path) = if let Some(after_mnt) = rest_after_mnt {
                let mut parts = after_mnt.splitn(2, '/');
                let d = parts.next()?;
                if d.len() == 1 && d.chars().next()?.is_ascii_alphabetic() {
                    (d.chars().next()?, parts.next().unwrap_or(""))
                } else {
                    return None;
                }
            } else if trimmed.starts_with('/') {
                let mut parts = trimmed[1..].splitn(2, '/');
                let d = parts.next()?;
                if d.len() == 1 && d.chars().next()?.is_ascii_alphabetic() {
                    (d.chars().next()?, parts.next().unwrap_or(""))
                } else {
                    return None;
                }
            } else {
                return None;
            };

            let drive_upper = drive_char.to_ascii_uppercase();
            let win_rest = rest_path.replace('/', "\\");
            let win_path = format!("{drive_upper}:\\{win_rest}");
            if Path::new(&win_path).exists() {
                Some(PathBuf::from(win_path))
            } else {
                None
            }
        }
        _ => {
            let flipped = if os_to == Os::Windows {
                trimmed.replace('/', "\\")
            } else {
                trimmed.replace('\\', "/")
            };
            if Path::new(&flipped).exists() {
                Some(PathBuf::from(flipped))
            } else {
                None
            }
        }
    }
}

/// Entfernt ein optionales führendes UTF-8 BOM (`\u{feff}`).
pub fn strip_utf8_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// Liest eine Datei als UTF-8-String und entfernt ein optionales führendes BOM (`\u{feff}`).
pub fn read_to_string_clean(path: &Path) -> std::io::Result<String> {
    let raw = std::fs::read_to_string(path)?;
    Ok(strip_utf8_bom(&raw).to_string())
}

/// Schreibt Bytes atomar: erstellt Elternverzeichnisse, schreibt in `.tmp` und benennt atomar um.
pub fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let mut tmp_os = path.as_os_str().to_os_string();
    tmp_os.push(".tmp");
    let tmp = PathBuf::from(tmp_os);
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// Kopiert eine bestehende Datei nach `<datei>.bak`. Gibt `Ok(None)` zurück, falls die Datei nicht existiert.
pub fn backup_file(path: &Path) -> std::io::Result<Option<PathBuf>> {
    if path.is_file() {
        let mut bak_os = path.as_os_str().to_os_string();
        bak_os.push(".bak");
        let bak_path = PathBuf::from(bak_os);
        std::fs::copy(path, &bak_path)?;
        Ok(Some(bak_path))
    } else {
        Ok(None)
    }
}

/// Erstellt optional ein `.bak`-Backup und schreibt den Inhalt anschließend atomar.
pub fn write_atomic_with_backup(
    path: &Path,
    content: &[u8],
    create_bak: bool,
) -> std::io::Result<()> {
    if create_bak {
        backup_file(path)?;
    }
    write_atomic(path, content)
}

/// Lädt eine JSON-Datei (inkl. BOM-Bereinigung) oder gibt bei Fehlern `T::default()` zurück.
#[cfg(feature = "serde")]
pub fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    match read_to_string_clean(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

/// Serialisiert einen Wert als formatiertes JSON und schreibt ihn atomar (optional mit `.bak`).
#[cfg(feature = "serde")]
pub fn save_json_atomic<T: Serialize>(
    path: &Path,
    value: &T,
    create_bak: bool,
) -> Result<(), KitFsError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_atomic_with_backup(path, &bytes, create_bak)?;
    Ok(())
}

/// Rekursive Berechnung der Verzeichnisgröße in Bytes.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Formatierung von Byte-Größen (z. B. `"–"`, `"12 KB"`, `"4.5 MB"`, `"1.25 GB"`).
pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        "–".into()
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Formatiert einen Unix-Timestamp relativ zu `now` (z. B. `"vor 5 Min"`, `"gerade eben"`, `"nie"`).
pub fn format_relative_time(ts: Option<i64>, now: i64) -> String {
    let Some(ts) = ts else { return "nie".into() };
    let d = (now - ts).max(0);
    match d {
        0..=59 => "gerade eben".into(),
        60..=3599 => format!("vor {} Min", d / 60),
        3600..=86399 => format!("vor {} Std", d / 3600),
        _ => format!("vor {} Tagen", d / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kit_fs_test_{name}_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_norm_and_map_from() {
        assert_eq!(norm("R:\\Git\\Project"), "r:/git/project");

        let maps = vec![
            PathMapping::new("R:\\", "/r/"),
            PathMapping::new("Z:\\Shared", "/mnt/shared"),
        ];

        let unix = map_from(Os::Windows, Os::Linux, "R:\\repos\\test", &maps);
        assert_eq!(unix, Some("/r/repos/test".to_string()));

        let win = map_from(Os::Linux, Os::Windows, "/mnt/shared/data.csv", &maps);
        assert_eq!(win, Some("Z:\\Shared\\data.csv".to_string()));
    }

    #[test]
    fn test_auto_map_from() {
        let win_path = "C:\\Projects\\repo_non_existent_12345";
        let mapped = auto_map_from(Os::Windows, Os::Linux, win_path);
        assert_eq!(mapped, None);

        // Existierender temporärer Pfad sollte gemappt werden können
        let tmp = unique_temp_dir("automap");
        let tmp_win_slashes = tmp.to_string_lossy().replace('/', "\\");
        let mapped_existing = auto_map_from(Os::Windows, Os::Linux, &tmp_win_slashes);
        assert_eq!(mapped_existing, Some(tmp.clone()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_strip_utf8_bom_and_read_clean() {
        assert_eq!(strip_utf8_bom("\u{feff}hello"), "hello");
        assert_eq!(strip_utf8_bom("hello"), "hello");

        let dir = unique_temp_dir("bom");
        let file = dir.join("bom.txt");
        std::fs::write(&file, "\u{feff}inhalt").unwrap();
        assert_eq!(read_to_string_clean(&file).unwrap(), "inhalt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_atomic_and_backup_file() {
        let dir = unique_temp_dir("atomic");
        let file = dir.join("nested").join("data.txt");

        write_atomic(&file, b"v1").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"v1");

        // Backup auf nicht-existierende Datei -> None
        let missing_bak = backup_file(&dir.join("missing.txt")).unwrap();
        assert_eq!(missing_bak, None);

        // Write with backup auf existierende Datei -> .bak mit "v1", Datei mit "v2"
        write_atomic_with_backup(&file, b"v2", true).unwrap();
        let bak_path = dir.join("nested").join("data.txt.bak");
        assert!(bak_path.exists());
        assert_eq!(std::fs::read(&bak_path).unwrap(), b"v1");
        assert_eq!(std::fs::read(&file).unwrap(), b"v2");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_json_helpers() {
        #[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
        struct Sample {
            name: String,
            count: u32,
        }

        let dir = unique_temp_dir("json");
        let file = dir.join("cfg.json");

        // Fehlende Datei -> Default
        let loaded: Sample = load_json_or_default(&file);
        assert_eq!(loaded, Sample::default());

        let data = Sample {
            name: "test".into(),
            count: 42,
        };
        save_json_atomic(&file, &data, true).unwrap();
        let reloaded: Sample = load_json_or_default(&file);
        assert_eq!(reloaded, data);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_format_bytes_and_dir_size() {
        assert_eq!(format_bytes(0), "–");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");

        let dir = unique_temp_dir("dirsize");
        std::fs::write(dir.join("a.bin"), vec![1u8; 100]).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("b.bin"), vec![2u8; 50]).unwrap();
        assert_eq!(dir_size(&dir), 150);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_format_relative_time() {
        assert_eq!(format_relative_time(None, 1000), "nie");
        assert_eq!(format_relative_time(Some(970), 1000), "gerade eben");
        assert_eq!(format_relative_time(Some(700), 1000), "vor 5 Min");
        assert_eq!(format_relative_time(Some(1000 - 7200), 1000), "vor 2 Std");
        assert_eq!(format_relative_time(Some(1000 - 3 * 86400), 1000), "vor 3 Tagen");
    }
}
