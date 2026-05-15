//! PTY Runner - Spawns terminal applications in a pseudo-terminal
//!
//! Provides deterministic terminal execution with configurable:
//! - Terminal size (rows x cols)
//! - Environment variables
//! - Locale settings
//! - Working directory

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;
use thiserror::Error;

/// Errors that can occur during PTY operations
#[derive(Debug, Error)]
pub enum PtyRunnerError {
    #[error("Failed to create PTY: {0}")]
    PtyCreation(String),

    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),

    #[error("Failed to write input: {0}")]
    WriteError(#[from] std::io::Error),

    #[error("Process not running")]
    NotRunning,

    #[error("Read timeout after {0:?}")]
    ReadTimeout(Duration),
}

/// Configuration for the PTY runner
#[derive(Debug, Clone)]
pub struct PtyRunnerConfig {
    /// Terminal width in columns (default: 80)
    pub cols: u16,
    /// Terminal height in rows (default: 24)
    pub rows: u16,
    /// Environment variables to set
    pub env: HashMap<String, String>,
    /// Environment variables to remove (applied after inherit, before set)
    pub env_remove: Vec<String>,
    /// Working directory for the spawned process
    pub cwd: Option<PathBuf>,
    /// Whether to clear the environment before setting custom vars
    pub clear_env: bool,
    /// Maximum output buffer size in bytes (oldest bytes are dropped)
    pub max_output_bytes: usize,
    /// Headful mirror configuration (optional)
    pub headful: Option<HeadfulConfig>,
}

impl Default for PtyRunnerConfig {
    fn default() -> Self {
        let mut env = HashMap::new();
        // Set deterministic locale for reproducible output
        env.insert("LANG".to_string(), "C.UTF-8".to_string());
        env.insert("LC_ALL".to_string(), "C.UTF-8".to_string());
        // Disable color codes that might vary
        env.insert("NO_COLOR".to_string(), "1".to_string());
        // Set TERM for basic terminal support
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        // Preserve PATH so spawned processes can find executables
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_string(), path);
        }
        // Preserve HOME for programs that need it
        if let Ok(home) = std::env::var("HOME") {
            env.insert("HOME".to_string(), home);
        }

        Self {
            cols: 80,
            rows: 24,
            env,
            env_remove: Vec::new(),
            cwd: None,
            clear_env: true,
            max_output_bytes: 2 * 1024 * 1024, // 2MB
            headful: HeadfulConfig::from_env(),
        }
    }
}

impl PtyRunnerConfig {
    /// Create a new config with custom terminal size
    pub fn with_size(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            ..Default::default()
        }
    }

    /// Set an environment variable
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set the working directory
    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    /// Don't clear the inherited environment
    pub fn inherit_env(mut self) -> Self {
        self.clear_env = false;
        self
    }

    /// Remove an environment variable (prevents inheritance from parent)
    pub fn env_remove(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
        self
    }

    /// Set maximum output buffer size (bytes)
    pub fn max_output_bytes(mut self, max_bytes: usize) -> Self {
        self.max_output_bytes = max_bytes;
        self
    }

    /// Enable headful mirror with custom configuration
    pub fn headful(mut self, config: HeadfulConfig) -> Self {
        self.headful = Some(config);
        self
    }
}

/// Headful mirror configuration
#[derive(Clone, Debug)]
pub struct HeadfulConfig {
    /// Enable headful mirror
    pub enabled: bool,
    /// Launch command (optional). Use "{fifo}" to inject FIFO path.
    pub command: Option<String>,
    /// Directory for FIFO files (optional)
    pub fifo_dir: Option<PathBuf>,
    /// Reuse a single terminal window/FIFO across runs
    pub reuse_window: bool,
    /// Optional name for shared FIFO when reuse is enabled
    pub name: Option<String>,
}

impl HeadfulConfig {
    /// Load configuration from environment variables
    ///
    /// - TUI_TEST_HEADFUL=1 enables mirror
    /// - TUI_TEST_HEADFUL_CMD overrides launch command
    /// - TUI_TEST_HEADFUL_DIR overrides FIFO directory
    pub fn from_env() -> Option<Self> {
        let enabled = std::env::var("TUI_TEST_HEADFUL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !enabled {
            return None;
        }

        let command = std::env::var("TUI_TEST_HEADFUL_CMD").ok();
        let fifo_dir = std::env::var("TUI_TEST_HEADFUL_DIR")
            .ok()
            .map(PathBuf::from);
        let reuse_window = std::env::var("TUI_TEST_HEADFUL_REUSE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let name = std::env::var("TUI_TEST_HEADFUL_NAME").ok();

        Some(Self {
            enabled,
            command,
            fifo_dir,
            reuse_window,
            name,
        })
    }

    fn fifo_dir(&self) -> PathBuf {
        self.fifo_dir.clone().unwrap_or_else(std::env::temp_dir)
    }

    fn fifo_path(&self) -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        self.fifo_dir()
            .join(format!("cas-tui-headful-{pid}-{nanos}.fifo"))
    }

    fn fifo_path_reuse(&self) -> PathBuf {
        let name = self
            .name
            .clone()
            .unwrap_or_else(|| "cas-tui-test".to_string());
        self.fifo_dir().join(format!("cas-tui-headful-{name}.fifo"))
    }

    #[cfg(target_os = "macos")]
    fn window_id_path(&self) -> PathBuf {
        let name = self
            .name
            .clone()
            .unwrap_or_else(|| "cas-tui-test".to_string());
        self.fifo_dir()
            .join(format!("cas-tui-headful-{name}.windowid"))
    }

    #[allow(clippy::needless_return)]
    fn launch_terminal(&self, fifo_path: &std::path::Path) {
        if let Some(command) = &self.command {
            let mut cmd = command.clone();
            if cmd.contains("{fifo}") {
                cmd = cmd.replace("{fifo}", &fifo_path.display().to_string());
            } else {
                cmd.push(' ');
                cmd.push_str(&fifo_path.display().to_string());
            }
            let _ = std::process::Command::new("sh").arg("-lc").arg(cmd).spawn();
            return;
        }

        #[cfg(target_os = "macos")]
        {
            let path = escape_applescript_string(&fifo_path.display().to_string());
            let title = escape_applescript_string(
                &self
                    .name
                    .clone()
                    .unwrap_or_else(|| "cas-tui-test".to_string()),
            );
            let window_id_path =
                escape_applescript_string(&self.window_id_path().display().to_string());

            let script = if self.reuse_window {
                format!(
                    "tell application \"Terminal\"\n\
                        activate\n\
                        set targetCmd to \"tail -f {path}\"\n\
                        set targetTitle to \"cas-tui-headful:{title}\"\n\
                        set idFile to \"{window_id_path}\"\n\
                        set targetWindow to missing value\n\
                        set storedId to \"\"\n\
                        try\n\
                            set storedId to (do shell script \"cat \" & quoted form of idFile)\n\
                        end try\n\
                        if storedId is not \"\" then\n\
                            try\n\
                                set targetWindow to window id storedId\n\
                            end try\n\
                        end if\n\
                        if targetWindow is missing value then\n\
                            repeat with w in windows\n\
                                repeat with t in tabs of w\n\
                                    set tabContents to \"\"\n\
                                    try\n\
                                        set tabContents to (contents of t) as text\n\
                                    on error\n\
                                        set tabContents to \"\"\n\
                                    end try\n\
                                    if tabContents contains \"{path}\" then\n\
                                        set targetWindow to w\n\
                                        exit repeat\n\
                                    end if\n\
                                    try\n\
                                        if custom title of t is targetTitle then\n\
                                            set targetWindow to w\n\
                                            exit repeat\n\
                                        end if\n\
                                    end try\n\
                                end repeat\n\
                                if targetWindow is not missing value then exit repeat\n\
                            end repeat\n\
                        end if\n\
                        if targetWindow is missing value then\n\
                            if (count of windows) = 0 then\n\
                                do script targetCmd\n\
                                try\n\
                                    set targetWindow to front window\n\
                                end try\n\
                            else\n\
                                try\n\
                                    tell front window\n\
                                        activate\n\
                                        do script targetCmd in selected tab\n\
                                    end tell\n\
                                    set targetWindow to front window\n\
                                on error\n\
                                    do script targetCmd\n\
                                    try\n\
                                        set targetWindow to front window\n\
                                    end try\n\
                                end try\n\
                            end if\n\
                        else\n\
                            try\n\
                                tell targetWindow\n\
                                    activate\n\
                                    do script targetCmd in selected tab\n\
                                end tell\n\
                            on error\n\
                                try\n\
                                    do script targetCmd in targetWindow\n\
                                end try\n\
                            end try\n\
                        end if\n\
                        try\n\
                            if targetWindow is not missing value then\n\
                                tell targetWindow\n\
                                    try\n\
                                        set custom title of selected tab to targetTitle\n\
                                    end try\n\
                                end tell\n\
                            end if\n\
                        end try\n\
                        try\n\
                            if targetWindow is not missing value then\n\
                                set winId to id of targetWindow\n\
                                do shell script \"echo \" & winId & \" > \" & quoted form of idFile\n\
                            end if\n\
                        end try\n\
                    end tell"
                )
            } else {
                format!("tell application \"Terminal\" to do script \"tail -f {path}\"")
            };

            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(script)
                .spawn();
            return;
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = std::process::Command::new("xterm")
                .arg("-e")
                .arg(format!("tail -f {}", fifo_path.display()))
                .spawn();
        }
    }

    pub(crate) fn open_sink(&self) -> Option<(Arc<Mutex<std::fs::File>>, PathBuf, bool)> {
        if !self.enabled {
            return None;
        }

        if self.reuse_window {
            let shared = SHARED_HEADFUL.get_or_init(|| {
                let fifo_path = self.fifo_path_reuse();
                if !fifo_path.exists() {
                    let _ = std::process::Command::new("mkfifo")
                        .arg(&fifo_path)
                        .status();
                }
                self.launch_terminal(&fifo_path);
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&fifo_path)
                    .expect("headful fifo open failed");
                set_nonblocking(&file);
                SharedHeadful {
                    file: Arc::new(Mutex::new(file)),
                    fifo: fifo_path,
                }
            });
            return Some((shared.file.clone(), shared.fifo.clone(), false));
        }

        let fifo_path = self.fifo_path();
        let _ = std::process::Command::new("mkfifo")
            .arg(&fifo_path)
            .status();
        self.launch_terminal(&fifo_path);

        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo_path)
            .ok()
            .map(|file| {
                set_nonblocking(&file);
                (Arc::new(Mutex::new(file)), fifo_path, true)
            })
    }
}

/// Output buffer for captured PTY output
#[derive(Debug, Clone, Default)]
pub struct OutputBuffer {
    data: Vec<u8>,
    total_bytes: usize,
}

impl OutputBuffer {
    /// Get the raw bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Get the output as a string (lossy UTF-8 conversion)
    pub fn as_str(&self) -> String {
        String::from_utf8_lossy(&self.data).to_string()
    }

    /// Get the output as a lossy UTF-8 view
    pub fn as_str_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.data)
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.data.clear();
        self.total_bytes = 0;
    }

    /// Append data to the buffer
    pub fn append(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data);
        self.total_bytes = self.total_bytes.saturating_add(data.len());
    }

    /// Append data with a max buffer size, dropping oldest bytes if needed
    pub fn append_bounded(&mut self, data: &[u8], max_bytes: usize) {
        if max_bytes == 0 {
            return;
        }

        self.data.extend_from_slice(data);
        self.total_bytes = self.total_bytes.saturating_add(data.len());

        if self.data.len() > max_bytes {
            let drop = self.data.len() - max_bytes;
            self.data.drain(0..drop);
        }
    }

    /// Check if buffer contains a string
    pub fn contains(&self, needle: &str) -> bool {
        self.as_str().contains(needle)
    }

    /// Get the length of the buffer
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Total bytes ever appended (monotonic)
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

/// PTY Runner - manages a pseudo-terminal session
pub struct PtyRunner {
    config: PtyRunnerConfig,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    output: Arc<Mutex<OutputBuffer>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    reader_handle: Option<JoinHandle<()>>,
    read_offset: usize,
    headful_fifo: Option<PathBuf>,
    headful_cleanup: bool,
}

impl PtyRunner {
    /// Create a new PTY runner with default configuration
    pub fn new() -> Self {
        Self::with_config(PtyRunnerConfig::default())
    }

    /// Create a new PTY runner with custom configuration
    pub fn with_config(config: PtyRunnerConfig) -> Self {
        Self {
            config,
            master: None,
            writer: None,
            output: Arc::new(Mutex::new(OutputBuffer::default())),
            child: None,
            reader_handle: None,
            read_offset: 0,
            headful_fifo: None,
            headful_cleanup: false,
        }
    }

    /// Spawn a command in the PTY
    pub fn spawn(&mut self, program: &str, args: &[&str]) -> Result<(), PtyRunnerError> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows: self.config.rows,
                cols: self.config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyRunnerError::PtyCreation(e.to_string()))?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);

        // Set working directory if specified
        if let Some(ref cwd) = self.config.cwd {
            cmd.cwd(cwd);
        }

        // Handle environment
        if self.config.clear_env {
            cmd.env_clear();
        }
        for key in &self.config.env_remove {
            cmd.env_remove(key);
        }
        for (key, value) in &self.config.env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyRunnerError::SpawnFailed(e.to_string()))?;

        // Get reader and writer from the master
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyRunnerError::PtyCreation(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyRunnerError::PtyCreation(e.to_string()))?;

        let output = Arc::clone(&self.output);
        let max_output_bytes = self.config.max_output_bytes;
        let mut headful_path = None;
        let mut headful_cleanup = false;
        let headful = self
            .config
            .headful
            .as_ref()
            .and_then(|config| config.open_sink())
            .map(|(file, path, cleanup)| {
                headful_path = Some(path);
                headful_cleanup = cleanup;
                file
            });
        let handle = std::thread::spawn(move || {
            Self::reader_loop(reader, output, max_output_bytes, headful);
        });

        self.master = Some(pair.master);
        self.writer = Some(writer);
        self.child = Some(child);
        self.reader_handle = Some(handle);
        self.read_offset = 0;
        self.headful_fifo = headful_path;
        self.headful_cleanup = headful_cleanup;

        Ok(())
    }

    fn reader_loop(
        mut reader: Box<dyn Read + Send>,
        output: Arc<Mutex<OutputBuffer>>,
        max: usize,
        headful: Option<Arc<Mutex<std::fs::File>>>,
    ) {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut internal = output.lock().unwrap();
                    internal.append_bounded(&buf[..n], max);
                    if let Some(ref file) = headful {
                        let mut file = file.lock().unwrap();
                        let _ = file.write(&buf[..n]);
                    }
                }
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::WouldBlock {
                        break;
                    }
                }
            }
        }
    }

    /// Send input to the PTY
    pub fn send_input(&mut self, input: &str) -> Result<(), PtyRunnerError> {
        let writer = self.writer.as_mut().ok_or(PtyRunnerError::NotRunning)?;
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        Ok(())
    }

    /// Send raw bytes to the PTY
    pub fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), PtyRunnerError> {
        let writer = self.writer.as_mut().ok_or(PtyRunnerError::NotRunning)?;
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    /// Send a key sequence (e.g., Enter, Ctrl+C)
    pub fn send_key(&mut self, key: Key) -> Result<(), PtyRunnerError> {
        self.send_bytes(key.as_bytes())
    }

    /// Read available output from the PTY (non-blocking)
    pub fn read_available(&mut self) -> Result<OutputBuffer, PtyRunnerError> {
        let mut output = OutputBuffer::default();
        let internal = self.output.lock().unwrap();
        let len = internal.len();
        if self.read_offset > len {
            self.read_offset = len;
        }
        if self.read_offset < len {
            output.append(&internal.as_bytes()[self.read_offset..len]);
            self.read_offset = len;
        }

        Ok(output)
    }

    /// Get all captured output so far
    pub fn get_output(&self) -> OutputBuffer {
        self.output.lock().unwrap().clone()
    }

    /// Clear the internal output buffer
    pub fn clear_output(&self) {
        self.output.lock().unwrap().clear();
    }

    pub fn with_output<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&OutputBuffer) -> R,
    {
        let output = self.output.lock().unwrap();
        f(&output)
    }

    /// Check if the process is still running
    pub fn is_running(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            child.try_wait().ok().flatten().is_none()
        } else {
            false
        }
    }

    /// Wait for the process to exit
    pub fn wait(&mut self) -> Result<Option<portable_pty::ExitStatus>, PtyRunnerError> {
        if let Some(ref mut child) = self.child {
            Ok(child.wait().ok())
        } else {
            Err(PtyRunnerError::NotRunning)
        }
    }

    /// Kill the running process
    pub fn kill(&mut self) -> Result<(), PtyRunnerError> {
        if let Some(ref mut child) = self.child {
            child.kill().map_err(PtyRunnerError::WriteError)?;
        }
        Ok(())
    }

    /// Resize the terminal
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyRunnerError> {
        if let Some(ref master) = self.master {
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| PtyRunnerError::PtyCreation(e.to_string()))?;
            self.config.cols = cols;
            self.config.rows = rows;
        }
        Ok(())
    }

    /// Get the current terminal size
    pub fn size(&self) -> (u16, u16) {
        (self.config.cols, self.config.rows)
    }
}

impl Default for PtyRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PtyRunner {
    fn drop(&mut self) {
        if self.headful_cleanup {
            if let Some(ref path) = self.headful_fifo {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

struct SharedHeadful {
    file: Arc<Mutex<std::fs::File>>,
    fifo: PathBuf,
}

static SHARED_HEADFUL: OnceLock<SharedHeadful> = OnceLock::new();

#[cfg(target_os = "macos")]
fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\"', "\\\"")
}

fn set_nonblocking(file: &std::fs::File) {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            let fd = file.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }
}

/// Common key sequences for terminal input
#[derive(Debug, Clone, Copy)]
pub enum Key {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    CtrlC,
    CtrlD,
    CtrlZ,
    CtrlL,
}

impl Key {
    /// Get the byte sequence for this key
    pub fn as_bytes(&self) -> &'static [u8] {
        match self {
            Key::Enter => b"\r",
            Key::Tab => b"\t",
            Key::Escape => b"\x1b",
            Key::Backspace => b"\x7f",
            Key::Delete => b"\x1b[3~",
            Key::Up => b"\x1b[A",
            Key::Down => b"\x1b[B",
            Key::Left => b"\x1b[D",
            Key::Right => b"\x1b[C",
            Key::Home => b"\x1b[H",
            Key::End => b"\x1b[F",
            Key::PageUp => b"\x1b[5~",
            Key::PageDown => b"\x1b[6~",
            Key::CtrlC => b"\x03",
            Key::CtrlD => b"\x04",
            Key::CtrlZ => b"\x1a",
            Key::CtrlL => b"\x0c",
        }
    }
}

#[cfg(test)]
#[path = "pty_runner_tests/tests.rs"]
mod tests;
