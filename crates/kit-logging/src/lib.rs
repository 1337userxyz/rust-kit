//! `kit-logging`: In-Memory Ringpuffer (`VecDeque`) + gepufferter File-Logger (`BufWriter`)
//! für Desktop-Anwendungen, integriert mit der offiziellen Rust `log`-Facade.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use chrono::Local;
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};

/// Log-Stufen kompatibel mit Rust `log::Level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

impl From<Level> for LogLevel {
    fn from(l: Level) -> Self {
        match l {
            Level::Error => LogLevel::Error,
            Level::Warn => LogLevel::Warn,
            Level::Info => LogLevel::Info,
            Level::Debug => LogLevel::Debug,
            Level::Trace => LogLevel::Trace,
        }
    }
}

/// Ein einzelner Log-Eintrag für UI-Anzeige oder Formatierung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

/// Konfigurationsoptionen für die Logger-Initialisierung.
#[derive(Debug, Clone)]
pub struct LoggerOptions {
    pub log_file: Option<PathBuf>,
    pub max_entries: usize,
    pub max_level: LevelFilter,
    pub truncate_on_init: bool,
}

impl Default for LoggerOptions {
    fn default() -> Self {
        Self {
            log_file: None,
            max_entries: 1000,
            max_level: LevelFilter::Info,
            truncate_on_init: false,
        }
    }
}

/// Der zentrale Logger mit `VecDeque`-Ringpuffer und gepuffertem Datei-Writer.
pub struct AppLogger {
    entries: Arc<Mutex<VecDeque<LogEntry>>>,
    max_entries: usize,
    log_writer: Mutex<Option<BufWriter<File>>>,
}

impl AppLogger {
    pub fn new(log_file: Option<PathBuf>, max_entries: usize) -> Self {
        Self::with_options(log_file, max_entries, false)
    }

    pub fn with_options(
        log_file: Option<PathBuf>,
        max_entries: usize,
        truncate_on_init: bool,
    ) -> Self {
        let writer = log_file.and_then(|path| {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
            let mut opts = OpenOptions::new();
            opts.create(true).write(true);
            if truncate_on_init {
                opts.truncate(true);
            } else {
                opts.append(true);
            }
            opts.open(&path).ok().map(|file| {
                let mut bw = BufWriter::new(file);
                let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
                if truncate_on_init {
                    let _ = writeln!(bw, "=== Session gestartet: {} ===", now);
                } else {
                    let _ = writeln!(bw, "\n=== Session gestartet: {} ===", now);
                }
                let _ = bw.flush();
                bw
            })
        });

        Self {
            entries: Arc::new(Mutex::new(VecDeque::with_capacity(max_entries))),
            max_entries,
            log_writer: Mutex::new(writer),
        }
    }

    /// Fügt einen Log-Eintrag direkt hinzu (thread-safe).
    pub fn add(&self, level: LogLevel, target: &str, message: &str) {
        let now = Local::now();
        let entry = LogEntry {
            timestamp: now.format("%H:%M:%S%.3f").to_string(),
            level,
            target: target.to_string(),
            message: message.to_string(),
        };

        if let Ok(mut writer_guard) = self.log_writer.lock() {
            if let Some(ref mut writer) = *writer_guard {
                let full_ts = now.format("%Y-%m-%d %H:%M:%S%.3f");
                let _ = writeln!(
                    writer,
                    "[{}] [{}] {}: {}",
                    full_ts,
                    level.as_str(),
                    target,
                    message
                );
                let _ = writer.flush();
            }
        }

        if self.max_entries > 0 {
            if let Ok(mut lock) = self.entries.lock() {
                while lock.len() >= self.max_entries {
                    lock.pop_front();
                }
                lock.push_back(entry);
            }
        }
    }

    /// Holt alle Einträge, optional gefiltert nach Mindest-Stufe.
    pub fn get_entries(&self, min_level: Option<LogLevel>) -> Vec<LogEntry> {
        if let Ok(lock) = self.entries.lock() {
            match min_level {
                None => lock.iter().cloned().collect(),
                Some(min) => lock.iter().filter(|e| e.level >= min).cloned().collect(),
            }
        } else {
            Vec::new()
        }
    }

    /// Formatiert die Log-Einträge als mehrzeiligen String.
    pub fn format_text(&self, min_level: Option<LogLevel>) -> String {
        self.get_entries(min_level)
            .iter()
            .map(|e| format!("[{}] [{}] {}: {}", e.timestamp, e.level.as_str(), e.target, e.message))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Leert den In-Memory Puffer.
    pub fn clear(&self) {
        if let Ok(mut lock) = self.entries.lock() {
            lock.clear();
        }
    }
}

impl Log for AppLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let level = LogLevel::from(record.level());
            let target = record.target();
            let msg = format!("{}", record.args());
            self.add(level, target, &msg);
        }
    }

    fn flush(&self) {
        if let Ok(mut writer_guard) = self.log_writer.lock() {
            if let Some(ref mut writer) = *writer_guard {
                let _ = writer.flush();
            }
        }
    }
}

static GLOBAL_LOGGER: OnceLock<AppLogger> = OnceLock::new();

/// Initialisiert den globalen Logger und registriert ihn bei der `log`-Facade.
pub fn init_logger(
    log_file: Option<PathBuf>,
    max_entries: usize,
    max_level: LevelFilter,
) -> Result<&'static AppLogger, SetLoggerError> {
    init_logger_with_options(LoggerOptions {
        log_file,
        max_entries,
        max_level,
        truncate_on_init: false,
    })
}

/// Initialisiert den globalen Logger mit erweiterten Optionen (z. B. `truncate_on_init`).
pub fn init_logger_with_options(
    opts: LoggerOptions,
) -> Result<&'static AppLogger, SetLoggerError> {
    let max_level = opts.max_level;
    let logger = GLOBAL_LOGGER.get_or_init(|| {
        AppLogger::with_options(opts.log_file, opts.max_entries, opts.truncate_on_init)
    });
    log::set_logger(logger)?;
    log::set_max_level(max_level);
    Ok(logger)
}

/// Holt den globalen Logger (erzeugt einen Standard-Logger, falls noch nicht initiiert).
pub fn get_logger() -> &'static AppLogger {
    GLOBAL_LOGGER.get_or_init(|| AppLogger::new(None, 1000))
}

pub fn log_info(target: &str, msg: &str) {
    get_logger().add(LogLevel::Info, target, msg);
}

pub fn log_warn(target: &str, msg: &str) {
    get_logger().add(LogLevel::Warn, target, msg);
}

pub fn log_error(target: &str, msg: &str) {
    get_logger().add(LogLevel::Error, target, msg);
}

pub fn log_debug(target: &str, msg: &str) {
    get_logger().add(LogLevel::Debug, target, msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_log(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("kit_logging_test_{name}_{nanos}.log"))
    }

    #[test]
    fn test_ring_buffer_and_level_filter() {
        let logger = AppLogger::new(None, 3);
        logger.add(LogLevel::Debug, "T0", "msg 0");
        logger.add(LogLevel::Info, "T1", "msg 1");
        logger.add(LogLevel::Warn, "T2", "msg 2");
        assert_eq!(logger.get_entries(None).len(), 3);

        // 4. Eintrag schiebt den ersten Eintrag in O(1) heraus
        logger.add(LogLevel::Error, "T3", "msg 3");
        let entries = logger.get_entries(None);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].message, "msg 1");
        assert_eq!(entries[2].message, "msg 3");

        // Filter nach Warn oder höher
        let warn_or_above = logger.get_entries(Some(LogLevel::Warn));
        assert_eq!(warn_or_above.len(), 2);
        assert_eq!(warn_or_above[0].level, LogLevel::Warn);
        assert_eq!(warn_or_above[1].level, LogLevel::Error);

        let text = logger.format_text(Some(LogLevel::Error));
        assert!(text.contains("[ERROR] T3: msg 3"));

        logger.clear();
        assert!(logger.get_entries(None).is_empty());
    }

    #[test]
    fn test_truncate_on_init() {
        let log_path = unique_temp_log("truncate");
        std::fs::write(&log_path, "ALTER_INHALT_DER_WEG_MUSS\n").unwrap();

        let logger = AppLogger::with_options(Some(log_path.clone()), 10, true);
        logger.add(LogLevel::Info, "App", "Neuer Eintrag");
        logger.flush();

        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(!content.contains("ALTER_INHALT_DER_WEG_MUSS"));
        assert!(content.contains("Neuer Eintrag"));

        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn test_append_without_truncate() {
        let log_path = unique_temp_log("append");
        std::fs::write(&log_path, "BESTEHENDER_INHALT\n").unwrap();

        let logger = AppLogger::with_options(Some(log_path.clone()), 10, false);
        logger.add(LogLevel::Info, "App", "Angehaengt");
        logger.flush();

        let content = std::fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("BESTEHENDER_INHALT"));
        assert!(content.contains("Angehaengt"));

        let _ = std::fs::remove_file(&log_path);
    }
}
