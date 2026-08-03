//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Build Output Logger
//!
//! Provides a clean abstraction for writing build output to log files.
//! Supports combined logging (default) and split stdout/stderr logging.
//!
//! Command output is **streamed line-by-line** into the log file as it
//! arrives, via a [`LogStream`] handle the command runner (`cmd.rs`) writes
//! to during execution. Each line is ANSI-stripped and flushed immediately,
//! so `tail -f` works live and a crash mid-command loses nothing. The
//! `BuildLogger` itself writes the surrounding `[farm]` headers/footer (the
//! `[farm]` prefix marks lines emitted by farm rather than the task, matching
//! the inline `[farm]` notices the command runner writes); both share the same
//! underlying writer so ordering stays correct.
//!
//! ## Combined log (default, merged streams)
//!
//! stdout + stderr are merged in arrival order by the command runner; the
//! log mirrors that — an unframed run of lines between the headers and footer:
//!
//! ```text
//! [farm] stage=name variant=default timestamp_start=2026-03-11T21:51:51.730Z
//! [farm] cmd=/bin/sh -c echo "hello"
//!
//! hello
//!
//! [farm] exit_code=0 success=true duration_ms=4 timestamp_end=2026-03-11T21:51:51.734Z
//! ```
//!
//! ## Split file mode (`--log-output file-split`)
//!
//! stdout and stderr stream into their own files (`*_stdout.log` /
//! `*_stderr.log`); each file is single-stream, so no per-line framing is
//! used — the same header/footer wrap the streamed lines.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use chrono::Utc;

/// A log file writer shared between the `BuildLogger` (headers/footers) and a
/// `LogStream` handle (streamed command output). Sharing the *same* writer —
/// rather than opening a second handle to the file — keeps the buffered file
/// position consistent across both, so streamed lines and the surrounding
/// `[farm]` framing never clobber each other.
type SharedWriter = Arc<Mutex<BufWriter<File>>>;

/// Strip ANSI escape sequences (SGR colors, OSC hyperlinks, and
/// generic two-byte escapes) from `input`. Log files are read with
/// plain editors / grep, so leaving raw `\x1b[…m` and `\x1b]8;;…\x1b\\`
/// in them just produces noise.
///
/// Recognises:
/// - CSI sequences `ESC [ … <final-byte>` (covers SGR / cursor / mode).
/// - OSC sequences `ESC ] … BEL` or `ESC ] … ESC \` (covers `ESC ]8;;`
///   hyperlinks emitted by cargo / clippy).
/// - Any other `ESC <byte>` two-byte form (best-effort fallback).
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'[' => {
                    // CSI: ESC [ params... <final byte 0x40-0x7E>
                    i += 2;
                    while i < bytes.len() {
                        let b = bytes[i];
                        i += 1;
                        if (0x40..=0x7e).contains(&b) {
                            break;
                        }
                    }
                }
                b']' => {
                    // OSC: ESC ] ... terminator
                    // Terminator is BEL (0x07) or ST (ESC \).
                    i += 2;
                    while i < bytes.len() {
                        let b = bytes[i];
                        if b == 0x07 {
                            i += 1;
                            break;
                        }
                        if b == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Plain ESC <byte>: skip both.
                    i += 2;
                }
            }
        } else {
            // UTF-8 safe: re-decode the next char from the original
            // string slice rather than the byte.
            let rest = &input[i..];
            if let Some(c) = rest.chars().next() {
                out.push(c);
                i += c.len_utf8();
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Log output mode
#[derive(Debug, Clone, PartialEq)]
pub enum LogMode {
    /// No file logging
    None,
    /// Output to console stdout/stderr
    Stdout,
    /// Combined log file: {target}_{variant}.log
    Combined,
    /// Split log files: {target}_{variant}_stdout.log and {target}_{variant}_stderr.log
    Split,
}

impl LogMode {
    /// Parse from CLI argument string. (Inherent `from_str` is an ergonomic
    /// constructor here, not the `FromStr` trait.)
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "none" => Ok(LogMode::None),
            "stdout" => Ok(LogMode::Stdout),
            "file" => Ok(LogMode::Combined),
            "file-split" => Ok(LogMode::Split),
            _ => Err(format!(
                "Invalid log mode: '{}'. Must be 'file', 'file-split', 'stdout', or 'none'",
                s
            )),
        }
    }

    /// Check if this mode writes to files
    pub fn writes_to_file(&self) -> bool {
        matches!(self, LogMode::Combined | LogMode::Split)
    }
}

/// Build output logger that handles combined and split log modes
pub struct BuildLogger {
    mode: LogMode,
    combined_log: Option<SharedWriter>,
    stdout_log: Option<SharedWriter>,
    stderr_log: Option<SharedWriter>,
    log_dir: PathBuf,
    target: String,
    variant: String,
}

/// A cloneable, thread-safe handle for streaming command output line-by-line
/// into the active log file(s). Handed out by [`BuildLogger::stream`] and
/// carried through `task::Context` so the command runner (`cmd.rs`) can write
/// each line as it arrives. ANSI escapes are stripped and the writer is
/// flushed per line (so `tail -f` works and a crash loses nothing).
#[derive(Clone)]
pub struct LogStream {
    mode: LogMode,
    combined_log: Option<SharedWriter>,
    stdout_log: Option<SharedWriter>,
    stderr_log: Option<SharedWriter>,
}

impl LogStream {
    /// Pick the writer a stdout line should land in for the current mode.
    fn stdout_target(&self) -> Option<&SharedWriter> {
        match self.mode {
            LogMode::Combined => self.combined_log.as_ref(),
            LogMode::Split => self.stdout_log.as_ref(),
            LogMode::None | LogMode::Stdout => None,
        }
    }

    /// Pick the writer a stderr line should land in for the current mode.
    /// In combined mode stderr is merged into the single combined file.
    fn stderr_target(&self) -> Option<&SharedWriter> {
        match self.mode {
            LogMode::Combined => self.combined_log.as_ref(),
            LogMode::Split => self.stderr_log.as_ref(),
            LogMode::None | LogMode::Stdout => None,
        }
    }

    fn write_line(target: Option<&SharedWriter>, line: &str) {
        if let Some(writer) = target {
            let clean = strip_ansi(line);
            if let Ok(mut guard) = writer.lock() {
                // Best-effort: a logging I/O error must not abort the build.
                let _ = writeln!(guard, "{}", clean);
                let _ = guard.flush();
            }
        }
    }

    /// Stream a single stdout line to the log (ANSI stripped, flushed).
    pub fn stdout_line(&self, line: &str) {
        Self::write_line(self.stdout_target(), line);
    }

    /// Stream a single stderr line to the log (ANSI stripped, flushed).
    pub fn stderr_line(&self, line: &str) {
        Self::write_line(self.stderr_target(), line);
    }
}

impl BuildLogger {
    /// Create a new BuildLogger
    pub fn new(
        log_dir: &Path,
        target: &str,
        variant: &str,
        mode: LogMode,
    ) -> Result<Self, String> {
        let mut logger = BuildLogger {
            mode: mode.clone(),
            combined_log: None,
            stdout_log: None,
            stderr_log: None,
            log_dir: log_dir.to_path_buf(),
            target: target.to_string(),
            variant: variant.to_string(),
        };

        // Create log files based on mode
        match mode {
            LogMode::Combined => {
                let path = log_dir.join(format!("{}_{}.log", target, variant));
                logger.combined_log = Some(Self::create_log_file(&path)?);
            }
            LogMode::Split => {
                let stdout_path = log_dir.join(format!("{}_{}_stdout.log", target, variant));
                let stderr_path = log_dir.join(format!("{}_{}_stderr.log", target, variant));
                logger.stdout_log = Some(Self::create_log_file(&stdout_path)?);
                logger.stderr_log = Some(Self::create_log_file(&stderr_path)?);
            }
            LogMode::None | LogMode::Stdout => {
                // No files to create
            }
        }

        Ok(logger)
    }

    /// Create a log file with truncation
    fn create_log_file(path: &Path) -> Result<SharedWriter, String> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|e| format!("Failed to create log file {}: {}", path.display(), e))?;
        Ok(Arc::new(Mutex::new(BufWriter::new(file))))
    }

    /// Hand out a cloneable streaming handle that shares this logger's file
    /// writers. The command runner writes output lines through it while a
    /// command runs; headers/footers continue to go through the `BuildLogger`
    /// itself — both share the same underlying writer, so ordering is correct.
    pub fn stream(&self) -> LogStream {
        LogStream {
            mode: self.mode.clone(),
            combined_log: self.combined_log.clone(),
            stdout_log: self.stdout_log.clone(),
            stderr_log: self.stderr_log.clone(),
        }
    }

    /// Write a single blank separator line to the active log writer(s).
    /// The engine calls this once after the task header, before output starts
    /// streaming in — mirroring the leading blank line of the old block format.
    pub fn begin_output(&mut self) -> Result<(), String> {
        self.write_raw("")
    }

    /// Get the log mode
    pub fn mode(&self) -> &LogMode {
        &self.mode
    }

    /// Get the log directory
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Format current timestamp as ISO 8601
    fn timestamp() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    /// Write stage header: [farm] stage=name variant=default timestamp_start=...
    pub fn write_stage_header(&mut self, stage: &str, variant: &str) -> Result<(), String> {
        let line = format!("[farm] stage={} variant={} timestamp_start={}", stage, variant, Self::timestamp());
        self.write_raw(&line)
    }

    /// Write task/command header: [farm] cmd=...
    pub fn write_task_header(&mut self, cmd: &str) -> Result<(), String> {
        let line = format!("[farm] cmd={}", cmd);
        self.write_raw(&line)
    }

    /// Write stage footer: [farm] exit_code=... success=... duration_ms=... timestamp_end=...
    pub fn write_stage_footer(&mut self, exit_code: Option<i32>, success: bool, duration_ms: u64) -> Result<(), String> {
        self.write_raw("")?; // blank line before footer
        let exit_str = exit_code.map(|c| c.to_string()).unwrap_or_else(|| "none".to_string());
        let line = format!("[farm] exit_code={} success={} duration_ms={} timestamp_end={}", 
            exit_str, success, duration_ms, Self::timestamp());
        self.write_raw(&line)
    }

    /// Write a line to a single shared writer, locking it.
    fn write_shared(writer: &SharedWriter, line: &str, what: &str) -> Result<(), String> {
        let mut guard = writer
            .lock()
            .map_err(|_| format!("{} log mutex poisoned", what))?;
        writeln!(guard, "{}", line).map_err(|e| format!("Failed to write to {} log: {}", what, e))
    }

    /// Write a raw line to logs (internal helper)
    fn write_raw(&mut self, line: &str) -> Result<(), String> {
        match self.mode {
            LogMode::Combined => {
                if let Some(ref log) = self.combined_log {
                    Self::write_shared(log, line, "combined")?;
                }
            }
            LogMode::Split => {
                if let Some(ref log) = self.stdout_log {
                    Self::write_shared(log, line, "stdout")?;
                }
                if let Some(ref log) = self.stderr_log {
                    Self::write_shared(log, line, "stderr")?;
                }
            }
            LogMode::None | LogMode::Stdout => {}
        }
        Ok(())
    }

    /// Write a header line (goes to all active logs) - legacy method
    pub fn write_header(&mut self, header: &str) -> Result<(), String> {
        self.write_raw(header)
    }

    /// Write a message line (typically metadata like exit code, duration)
    pub fn write_info(&mut self, info: &str) -> Result<(), String> {
        match self.mode {
            LogMode::Combined => {
                if let Some(ref log) = self.combined_log {
                    Self::write_shared(log, info, "combined")?;
                }
            }
            LogMode::Split => {
                // Info goes to stdout log in split mode
                if let Some(ref log) = self.stdout_log {
                    Self::write_shared(log, info, "stdout")?;
                }
            }
            LogMode::None | LogMode::Stdout => {}
        }
        Ok(())
    }

    /// Write a blank line
    pub fn write_blank(&mut self) -> Result<(), String> {
        self.write_info("")
    }

    /// Flush all log buffers
    pub fn flush(&mut self) -> Result<(), String> {
        for (writer, what) in [
            (&self.combined_log, "combined"),
            (&self.stdout_log, "stdout"),
            (&self.stderr_log, "stderr"),
        ] {
            if let Some(log) = writer {
                log.lock()
                    .map_err(|_| format!("{} log mutex poisoned", what))?
                    .flush()
                    .map_err(|e| format!("Failed to flush {} log: {}", what, e))?;
            }
        }
        Ok(())
    }

    /// Read the combined log content (for proxy notification)
    pub fn read_combined_log(&self) -> Option<String> {
        match self.mode {
            LogMode::Combined => {
                let path = self.log_dir.join(format!("{}_{}.log", self.target, self.variant));
                std::fs::read_to_string(&path).ok()
            }
            LogMode::Split => {
                // Return stdout log content as the "main" log
                let path = self.log_dir.join(format!("{}_{}_stdout.log", self.target, self.variant));
                std::fs::read_to_string(&path).ok()
            }
            _ => None,
        }
    }

    /// Read the stderr log content (only available in split mode)
    pub fn read_stderr_log(&self) -> Option<String> {
        match self.mode {
            LogMode::Split => {
                let path = self.log_dir.join(format!("{}_{}_stderr.log", self.target, self.variant));
                std::fs::read_to_string(&path).ok()
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_log_mode_from_str() {
        assert_eq!(LogMode::from_str("file").unwrap(), LogMode::Combined);
        assert_eq!(LogMode::from_str("file-split").unwrap(), LogMode::Split);
        assert_eq!(LogMode::from_str("stdout").unwrap(), LogMode::Stdout);
        assert_eq!(LogMode::from_str("none").unwrap(), LogMode::None);
        assert!(LogMode::from_str("invalid").is_err());
    }

    #[test]
    fn test_combined_streaming_no_framing() {
        // Default path: lines stream into the combined file via LogStream,
        // wrapped by the engine's header/footer. No [stdout]/[stderr] framing.
        let temp_dir = TempDir::new().unwrap();
        let mut logger = BuildLogger::new(temp_dir.path(), "build", "debug", LogMode::Combined).unwrap();

        logger.write_stage_header("build", "debug").unwrap();
        logger.write_task_header("/bin/sh -c echo hello").unwrap();
        logger.begin_output().unwrap();
        let stream = logger.stream();
        stream.stdout_line("hello");
        stream.stderr_line("warning text"); // merged into the combined file
        logger.write_stage_footer(Some(0), true, 42).unwrap();
        logger.flush().unwrap();

        let content = fs::read_to_string(temp_dir.path().join("build_debug.log")).unwrap();
        assert!(content.contains("[farm] stage=build variant=debug timestamp_start="));
        assert!(content.contains("[farm] cmd=/bin/sh -c echo hello"));
        assert!(content.contains("hello"));
        assert!(content.contains("warning text"));
        assert!(!content.contains("[stdout]"), "streamed log must not emit [stdout] framing, got:\n{}", content);
        assert!(!content.contains("[stderr]"), "streamed log must not emit [stderr] framing, got:\n{}", content);
        assert!(content.contains("[farm] exit_code=0 success=true duration_ms=42 timestamp_end="));
    }

    #[test]
    fn test_streamed_lines_are_ansi_stripped() {
        // SGR colors and OSC hyperlinks must be stripped before landing on disk.
        let temp_dir = TempDir::new().unwrap();
        let logger = BuildLogger::new(temp_dir.path(), "build", "debug", LogMode::Combined).unwrap();
        let stream = logger.stream();
        stream.stdout_line("\x1b[1m\x1b[33mwarning\x1b[0m: thing");
        stream.stdout_line("\x1b]8;;https://example.com\x1b\\link text\x1b]8;;\x1b\\");
        // LogStream flushes per line, so the file is already on disk.

        let content = fs::read_to_string(temp_dir.path().join("build_debug.log")).unwrap();
        assert!(content.contains("warning: thing"));
        assert!(content.contains("link text"));
        assert!(!content.contains('\x1b'), "log must not contain raw escape bytes");
    }

    #[test]
    fn test_stream_noop_for_none_mode() {
        // With no file logging, the stream handle is a no-op and writes nothing.
        let temp_dir = TempDir::new().unwrap();
        let logger = BuildLogger::new(temp_dir.path(), "build", "debug", LogMode::None).unwrap();
        let stream = logger.stream();
        stream.stdout_line("ignored");
        stream.stderr_line("ignored");
        assert!(!temp_dir.path().join("build_debug.log").exists());
    }

    #[test]
    fn test_strip_ansi_helper() {
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\x1b[1mbold\x1b[0m"), "bold");
        assert_eq!(strip_ansi("a\x1b[33mb\x1b[0mc"), "abc");
        // OSC hyperlink wrapping `text`
        assert_eq!(
            strip_ansi("\x1b]8;;https://x\x1b\\text\x1b]8;;\x1b\\"),
            "text"
        );
        // UTF-8 survives intact
        assert_eq!(strip_ansi("héllo\x1b[31m world\x1b[0m"), "héllo world");
    }

    #[test]
    fn test_split_streaming() {
        // In split mode each stream lands in its own file (no framing markers).
        let temp_dir = TempDir::new().unwrap();
        let mut logger = BuildLogger::new(temp_dir.path(), "build", "debug", LogMode::Split).unwrap();

        logger.write_stage_header("build", "debug").unwrap();
        logger.begin_output().unwrap();
        let stream = logger.stream();
        stream.stdout_line("Hello from stdout");
        stream.stderr_line("Warning message");
        logger.flush().unwrap();

        let stdout_path = temp_dir.path().join("build_debug_stdout.log");
        let stderr_path = temp_dir.path().join("build_debug_stderr.log");
        assert!(stdout_path.exists());
        assert!(stderr_path.exists());

        let stdout_content = fs::read_to_string(&stdout_path).unwrap();
        let stderr_content = fs::read_to_string(&stderr_path).unwrap();

        // Header goes to both files; each stream's line lands only in its file.
        assert!(stdout_content.contains("[farm] stage=build"));
        assert!(stdout_content.contains("Hello from stdout"));
        assert!(!stdout_content.contains("Warning message"), "stderr line must not land in stdout file");
        assert!(stderr_content.contains("Warning message"));
        assert!(!stdout_content.contains("[stdout]"), "no framing markers in streamed split files");
        assert!(!stderr_content.contains("[stderr]"), "no framing markers in streamed split files");
    }
}
