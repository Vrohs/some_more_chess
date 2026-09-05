//! Recording what went wrong, so it can be read later instead of guessed at.
//!
//! This deliberately does not use the database. A panic may well be the
//! database failing, a migration going wrong, or the process dying with a lock
//! held — and a fault log that needs the broken component to record the fault
//! is no log at all. So it is an append-only file of one JSON object per line,
//! written with a single `write` call and flushed, which survives a crash mid
//! way through and can be read by anything.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

/// What kind of fault a record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The process panicked. The worst kind: it took the window with it.
    Panic,
    /// An operation failed but the application carried on.
    Error,
    /// The toolkit complained. Often harmless, occasionally the first sign of
    /// a widget being used after it was dropped.
    Toolkit,
}

impl Fault {
    pub fn label(self) -> &'static str {
        match self {
            Fault::Panic => "panic",
            Fault::Error => "error",
            Fault::Toolkit => "toolkit",
        }
    }

    fn from_label(text: &str) -> Option<Self> {
        match text {
            "panic" => Some(Fault::Panic),
            "error" => Some(Fault::Error),
            "toolkit" => Some(Fault::Toolkit),
            _ => None,
        }
    }
}

/// One recorded fault.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub at: DateTime<Utc>,
    pub fault: Fault,
    /// Where in the application it happened — a file and line for a panic, or
    /// the operation that failed.
    pub site: String,
    pub message: String,
    /// The build it happened on, so a report from an old binary is obvious.
    pub version: String,
}

/// Escape the few characters JSON forbids in a string. Small enough to do by
/// hand, and worth avoiding a dependency in a path that runs during a panic.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(c);
                }
            }
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

impl Record {
    fn to_line(&self) -> String {
        format!(
            r#"{{"at":"{}","fault":"{}","site":"{}","message":"{}","version":"{}"}}"#,
            self.at.to_rfc3339(),
            self.fault.label(),
            escape(&self.site),
            escape(&self.message),
            escape(&self.version),
        )
    }

    /// Parse one line back. Faults are read far less often than they are
    /// written, and a line that cannot be read is skipped rather than fatal —
    /// a truncated last line is exactly what a crash leaves behind.
    fn from_line(line: &str) -> Option<Self> {
        let field = |name: &str| -> Option<String> {
            let key = format!(r#""{name}":""#);
            let start = line.find(&key)? + key.len();
            let rest = &line[start..];
            // Find the closing quote that is not escaped.
            let mut end = 0;
            let bytes = rest.as_bytes();
            while end < bytes.len() {
                if bytes[end] == b'"' && (end == 0 || bytes[end - 1] != b'\\') {
                    break;
                }
                end += 1;
            }
            Some(unescape(&rest[..end]))
        };
        Some(Record {
            at: DateTime::parse_from_rfc3339(&field("at")?)
                .ok()?
                .with_timezone(&Utc),
            fault: Fault::from_label(&field("fault")?)?,
            site: field("site")?,
            message: field("message")?,
            version: field("version").unwrap_or_default(),
        })
    }
}

/// Append a fault to the log at `path`, creating it if needed.
///
/// Failure here is swallowed: the application is already having a bad time and
/// a logger that panics while recording a panic helps nobody.
pub fn append(path: &Path, record: &Record) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let mut line = record.to_line();
    line.push('\n');
    let _ = file.write_all(line.as_bytes());
    let _ = file.flush();
}

/// Every fault recorded, oldest first. Unreadable lines are skipped.
pub fn read(path: &Path) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(Record::from_line).collect()
}

/// How many of each kind, and when the most recent was — the summary worth
/// seeing before any detail.
pub fn summarise(records: &[Record]) -> Vec<(Fault, usize, DateTime<Utc>)> {
    let mut out: Vec<(Fault, usize, DateTime<Utc>)> = Vec::new();
    for record in records {
        match out.iter_mut().find(|(fault, _, _)| *fault == record.fault) {
            Some((_, count, latest)) => {
                *count += 1;
                if record.at > *latest {
                    *latest = record.at;
                }
            }
            None => out.push((record.fault, 1, record.at)),
        }
    }
    out.sort_by_key(|(fault, _, _)| match fault {
        Fault::Panic => 0,
        Fault::Error => 1,
        Fault::Toolkit => 2,
    });
    out
}

/// The default location, beside the ordinary log.
pub fn default_path() -> PathBuf {
    crate::paths::cache_dir().join("diagnostics.jsonl")
}

/// Record a fault that the application recovered from.
pub fn record_error(site: &str, message: impl std::fmt::Display) {
    append(
        &default_path(),
        &Record {
            at: Utc::now(),
            fault: Fault::Error,
            site: site.to_owned(),
            message: message.to_string(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    );
}

/// Record a complaint from the toolkit.
pub fn record_toolkit(site: &str, message: impl std::fmt::Display) {
    append(
        &default_path(),
        &Record {
            at: Utc::now(),
            fault: Fault::Toolkit,
            site: site.to_owned(),
            message: message.to_string(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    );
}

/// Catch panics into the log, keeping whatever handler was already installed
/// so the message still reaches the terminal.
///
/// A panic in a GTK callback unwinds into C, which aborts — so by the time
/// anything else could look, the process is gone. This is the only chance to
/// write the reason down.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let site = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".to_owned());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic with no message".to_owned());
        append(
            &default_path(),
            &Record {
                at: Utc::now(),
                fault: Fault::Panic,
                site,
                message,
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        );
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(fault: Fault, message: &str) -> Record {
        Record {
            at: Utc::now(),
            fault,
            site: "store.rs:412".into(),
            message: message.into(),
            version: "0.1.0".into(),
        }
    }

    #[test]
    fn a_record_survives_a_round_trip() {
        let original = record(Fault::Panic, "called `Option::unwrap()` on a `None` value");
        let parsed = Record::from_line(&original.to_line()).expect("a written line reads back");
        assert_eq!(parsed.fault, original.fault);
        assert_eq!(parsed.site, original.site);
        assert_eq!(parsed.message, original.message);
        assert_eq!(parsed.version, original.version);
    }

    /// Panic messages routinely contain quotes, newlines and backslashes, and
    /// a log that mangles them loses the part worth reading.
    #[test]
    fn awkward_characters_survive() {
        let nasty = "expected \"a\\b\" got\n\ttab \u{1} end";
        let parsed = Record::from_line(&record(Fault::Error, nasty).to_line()).unwrap();
        assert_eq!(parsed.message, nasty);
    }

    /// A crash can leave a half-written last line. It must cost that line and
    /// nothing else.
    #[test]
    fn a_truncated_last_line_does_not_lose_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("diagnostics.jsonl");
        append(&path, &record(Fault::Error, "first"));
        append(&path, &record(Fault::Panic, "second"));
        // Simulate dying mid-write.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"at":"2026-09-05T10:00:00Z","fau"#)
            .unwrap();

        let read_back = read(&path);
        assert_eq!(read_back.len(), 2, "the two complete records survive");
        assert_eq!(read_back[0].message, "first");
        assert_eq!(read_back[1].message, "second");
    }

    #[test]
    fn reading_a_log_that_does_not_exist_is_empty_not_an_error() {
        assert!(read(Path::new("/nonexistent/diagnostics.jsonl")).is_empty());
    }

    #[test]
    fn the_summary_counts_each_kind_and_worst_first() {
        let records = vec![
            record(Fault::Toolkit, "a"),
            record(Fault::Error, "b"),
            record(Fault::Error, "c"),
            record(Fault::Panic, "d"),
        ];
        let summary = summarise(&records);
        assert_eq!(summary[0].0, Fault::Panic, "panics are read first");
        assert_eq!(summary[1].0, Fault::Error);
        assert_eq!(summary[1].1, 2);
        assert_eq!(summary[2].0, Fault::Toolkit);
    }
}
