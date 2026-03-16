/// TTY/PTY (pseudo-terminal) subsystem for MerlionOS.
///
/// Provides pseudo-terminal devices with configurable line discipline,
/// echo, and raw/cooked mode support.  Each TTY has independent input and
/// output buffers.  In cooked mode the line discipline buffers input until
/// a newline, handles backspace editing, and translates Ctrl+C / Ctrl+D
/// into signal / EOF semantics.  Thread-safe via `spin::Mutex`.
///
/// The default TTY (`/dev/tty0`) is registered with the VFS at init time.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;

// ── constants ────────────────────────────────────────────────────────

/// Maximum number of TTYs the subsystem will manage.
const MAX_TTYS: usize = 8;

/// Capacity of per-TTY input and output ring buffers (bytes).
const BUF_SIZE: usize = 1024;

/// ASCII code points used by the line discipline.
const CHAR_BACKSPACE: u8 = 0x08;
const CHAR_DEL: u8 = 0x7F;
const CHAR_NEWLINE: u8 = b'\n';
const CHAR_CARRIAGE_RETURN: u8 = b'\r';
const CHAR_CTRL_C: u8 = 0x03;
const CHAR_CTRL_D: u8 = 0x04;

// ── line discipline ──────────────────────────────────────────────────

/// Line discipline mode governing how input is processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDiscipline {
    /// Cooked (canonical) mode — input is buffered line-by-line; backspace
    /// editing and control-character handling are active.
    Cooked,
    /// Raw mode — every byte is delivered immediately with no
    /// interpretation or editing.
    Raw,
}

/// Result of feeding a single byte through the line discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisciplineEvent {
    /// The byte was consumed; no action required yet.
    Buffered,
    /// A complete line is ready to be read from the cooked buffer.
    LineReady,
    /// Ctrl+C was received — the foreground task should be interrupted.
    Interrupt,
    /// Ctrl+D was received on an empty line — signals end-of-input.
    Eof,
}

// ── ring buffer ──────────────────────────────────────────────────────

/// Simple bounded ring buffer used for both input and output.
#[derive(Clone)]
struct RingBuf {
    buf: [u8; BUF_SIZE],
    head: usize,
    tail: usize,
    len: usize,
}

impl RingBuf {
    /// Create an empty ring buffer.
    const fn new() -> Self {
        Self { buf: [0u8; BUF_SIZE], head: 0, tail: 0, len: 0 }
    }

    /// Number of bytes currently stored.
    fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` when the buffer has no data.
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Push a single byte.  Returns `false` if full.
    fn push(&mut self, byte: u8) -> bool {
        if self.len >= BUF_SIZE {
            return false;
        }
        self.buf[self.tail] = byte;
        self.tail = (self.tail + 1) % BUF_SIZE;
        self.len += 1;
        true
    }

    /// Pop a single byte from the front, or `None` if empty.
    fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.buf[self.head];
        self.head = (self.head + 1) % BUF_SIZE;
        self.len -= 1;
        Some(byte)
    }

    /// Remove the most recently pushed byte (for backspace editing).
    /// Returns `true` if a byte was removed.
    fn pop_back(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        self.tail = if self.tail == 0 { BUF_SIZE - 1 } else { self.tail - 1 };
        self.len -= 1;
        true
    }

    /// Drain up to `dst.len()` bytes into `dst`.  Returns the count read.
    fn drain_into(&mut self, dst: &mut [u8]) -> usize {
        let mut n = 0;
        while n < dst.len() {
            match self.pop() {
                Some(b) => { dst[n] = b; n += 1; }
                None => break,
            }
        }
        n
    }

    /// Discard all data.
    fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }
}

// ── TTY struct ───────────────────────────────────────────────────────

/// A single pseudo-terminal device.
pub struct Tty {
    /// Numeric identifier (0-based).
    pub id: usize,
    /// Buffer holding bytes available for userspace reads.
    input_buf: RingBuf,
    /// Buffer holding bytes written by userspace (e.g. for display).
    output_buf: RingBuf,
    /// Editing buffer used in cooked mode (accumulates the current line).
    cooked_line: RingBuf,
    /// Current line discipline.
    pub line_discipline: LineDiscipline,
    /// When `true`, input bytes are echoed back to the output buffer.
    pub echo: bool,
    /// Shorthand flag — `true` when line discipline is `Raw`.
    pub raw_mode: bool,
    /// PID of the foreground task group (for signal delivery).
    pub foreground_pid: Option<usize>,
}

impl Tty {
    /// Create a new TTY with sensible defaults (cooked mode, echo on).
    fn new(id: usize) -> Self {
        Self {
            id,
            input_buf: RingBuf::new(),
            output_buf: RingBuf::new(),
            cooked_line: RingBuf::new(),
            line_discipline: LineDiscipline::Cooked,
            echo: true,
            raw_mode: false,
            foreground_pid: None,
        }
    }

    /// Feed a single byte from the keyboard / PTY master side.
    ///
    /// In raw mode the byte is placed directly into the input buffer.
    /// In cooked mode the line discipline processes editing keys and
    /// control characters, returning an appropriate [`DisciplineEvent`].
    pub fn receive_byte(&mut self, byte: u8) -> DisciplineEvent {
        if self.raw_mode {
            self.input_buf.push(byte);
            if self.echo {
                self.output_buf.push(byte);
            }
            return DisciplineEvent::Buffered;
        }

        // ── cooked mode line discipline ──
        match byte {
            CHAR_CTRL_C => {
                // Discard the current editing line.
                self.cooked_line.clear();
                if self.echo {
                    // Echo ^C and a newline.
                    for &ch in b"^C\n" {
                        self.output_buf.push(ch);
                    }
                }
                // Deliver SIGINT to the foreground task if one exists.
                if let Some(pid) = self.foreground_pid {
                    let _ = crate::signal::send_signal(pid, crate::signal::SIGKILL);
                }
                DisciplineEvent::Interrupt
            }
            CHAR_CTRL_D => {
                if self.cooked_line.is_empty() {
                    // EOF on empty line.
                    if self.echo {
                        for &ch in b"^D\n" {
                            self.output_buf.push(ch);
                        }
                    }
                    DisciplineEvent::Eof
                } else {
                    // Flush whatever is in the editing buffer as a line.
                    self.flush_cooked_line();
                    DisciplineEvent::LineReady
                }
            }
            CHAR_BACKSPACE | CHAR_DEL => {
                if self.cooked_line.pop_back() && self.echo {
                    // Erase the character on the terminal: BS, space, BS.
                    for &ch in &[0x08, b' ', 0x08] {
                        self.output_buf.push(ch);
                    }
                }
                DisciplineEvent::Buffered
            }
            CHAR_NEWLINE | CHAR_CARRIAGE_RETURN => {
                self.cooked_line.push(CHAR_NEWLINE);
                if self.echo {
                    self.output_buf.push(CHAR_NEWLINE);
                }
                self.flush_cooked_line();
                DisciplineEvent::LineReady
            }
            _ => {
                self.cooked_line.push(byte);
                if self.echo {
                    self.output_buf.push(byte);
                }
                DisciplineEvent::Buffered
            }
        }
    }

    /// Move everything in the cooked editing buffer into the input buffer.
    fn flush_cooked_line(&mut self) {
        while let Some(b) = self.cooked_line.pop() {
            self.input_buf.push(b);
        }
    }

    /// Read up to `buf.len()` bytes from the TTY input buffer.
    /// Returns the number of bytes actually read.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        self.input_buf.drain_into(buf)
    }

    /// Write data to the TTY output buffer (from userspace).
    pub fn write(&mut self, data: &[u8]) {
        for &b in data {
            self.output_buf.push(b);
        }
    }

    /// Drain up to `buf.len()` bytes from the output buffer.
    /// Used by the display driver to fetch pending output.
    pub fn drain_output(&mut self, buf: &mut [u8]) -> usize {
        self.output_buf.drain_into(buf)
    }
}

// ── TTY manager ──────────────────────────────────────────────────────

/// Global manager holding every allocated TTY behind a spinlock.
pub struct TtyManager {
    ttys: Vec<Tty>,
}

impl TtyManager {
    /// Create an empty manager.
    fn new() -> Self {
        Self { ttys: Vec::new() }
    }

    /// Allocate a new TTY and return its id.  Returns `None` if the
    /// maximum number of TTYs has been reached.
    pub fn create_tty(&mut self) -> Option<usize> {
        if self.ttys.len() >= MAX_TTYS {
            return None;
        }
        let id = self.ttys.len();
        self.ttys.push(Tty::new(id));
        Some(id)
    }

    /// Obtain a mutable reference to the TTY with the given id.
    pub fn get_mut(&mut self, id: usize) -> Option<&mut Tty> {
        self.ttys.get_mut(id)
    }

    /// Obtain a shared reference to the TTY with the given id.
    pub fn get(&self, id: usize) -> Option<&Tty> {
        self.ttys.get(id)
    }

    /// Return the number of allocated TTYs.
    pub fn count(&self) -> usize {
        self.ttys.len()
    }
}

// ── global state ─────────────────────────────────────────────────────

/// Global TTY manager, protected by a spinlock.
pub static TTY_MANAGER: Mutex<Option<TtyManager>> = Mutex::new(None);

/// Initialise the TTY subsystem and create the default `/dev/tty0`.
///
/// Registers `/dev/tty0` with the VFS so that userspace can open the
/// primary console terminal.
pub fn init() {
    let mut mgr = TtyManager::new();
    let id = mgr.create_tty().expect("failed to create tty0");
    assert_eq!(id, 0, "first TTY must be tty0");
    *TTY_MANAGER.lock() = Some(mgr);

    // Register /dev/tty0 with the VFS.
    let _ = crate::vfs::write("/dev/tty0", "");
    crate::serial_println!("[tty] initialised /dev/tty0");
    crate::klog_println!("[tty] TTY subsystem ready");
}

/// Create a new TTY device.  Returns the numeric id, or `None` on failure.
pub fn create_tty() -> Option<usize> {
    let mut guard = TTY_MANAGER.lock();
    let mgr = guard.as_mut()?;
    let id = mgr.create_tty()?;
    // Register the device node in the VFS.
    let path = format!("/dev/tty{}", id);
    let _ = crate::vfs::write(&path, "");
    Some(id)
}

/// Read up to `buf.len()` bytes from the given TTY.
/// Returns `Ok(bytes_read)` or `Err` if the id is invalid.
pub fn tty_read(id: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut guard = TTY_MANAGER.lock();
    let mgr = guard.as_mut().ok_or("tty: not initialised")?;
    let tty = mgr.get_mut(id).ok_or("tty: invalid id")?;
    Ok(tty.read(buf))
}

/// Write `data` to the given TTY's output buffer.
pub fn tty_write(id: usize, data: &[u8]) -> Result<(), &'static str> {
    let mut guard = TTY_MANAGER.lock();
    let mgr = guard.as_mut().ok_or("tty: not initialised")?;
    let tty = mgr.get_mut(id).ok_or("tty: invalid id")?;
    tty.write(data);
    Ok(())
}

/// Enable or disable raw mode on the given TTY.
///
/// In raw mode every byte is delivered immediately with no line editing.
/// Disabling raw mode returns to cooked (canonical) mode.
pub fn set_raw_mode(id: usize, enable: bool) -> Result<(), &'static str> {
    let mut guard = TTY_MANAGER.lock();
    let mgr = guard.as_mut().ok_or("tty: not initialised")?;
    let tty = mgr.get_mut(id).ok_or("tty: invalid id")?;
    tty.raw_mode = enable;
    tty.line_discipline = if enable { LineDiscipline::Raw } else { LineDiscipline::Cooked };
    Ok(())
}

/// Enable or disable echo on the given TTY.
///
/// When echo is on, received input bytes are copied to the output buffer
/// so the user can see what they type.
pub fn set_echo(id: usize, enable: bool) -> Result<(), &'static str> {
    let mut guard = TTY_MANAGER.lock();
    let mgr = guard.as_mut().ok_or("tty: not initialised")?;
    let tty = mgr.get_mut(id).ok_or("tty: invalid id")?;
    tty.echo = enable;
    Ok(())
}

/// Feed a byte into the given TTY from the hardware side (keyboard IRQ
/// or PTY master).  Returns the resulting [`DisciplineEvent`].
pub fn receive_byte(id: usize, byte: u8) -> Result<DisciplineEvent, &'static str> {
    let mut guard = TTY_MANAGER.lock();
    let mgr = guard.as_mut().ok_or("tty: not initialised")?;
    let tty = mgr.get_mut(id).ok_or("tty: invalid id")?;
    Ok(tty.receive_byte(byte))
}

/// Drain pending output from the given TTY into `buf`.
/// Used by the display / serial driver to render terminal output.
pub fn drain_output(id: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut guard = TTY_MANAGER.lock();
    let mgr = guard.as_mut().ok_or("tty: not initialised")?;
    let tty = mgr.get_mut(id).ok_or("tty: invalid id")?;
    Ok(tty.drain_output(buf))
}

/// Set the foreground PID for signal delivery on Ctrl+C.
pub fn set_foreground(id: usize, pid: usize) -> Result<(), &'static str> {
    let mut guard = TTY_MANAGER.lock();
    let mgr = guard.as_mut().ok_or("tty: not initialised")?;
    let tty = mgr.get_mut(id).ok_or("tty: invalid id")?;
    tty.foreground_pid = Some(pid);
    Ok(())
}

/// Return a diagnostic summary of all allocated TTYs.
pub fn status() -> String {
    let guard = TTY_MANAGER.lock();
    match guard.as_ref() {
        None => "(tty subsystem not initialised)\n".into(),
        Some(mgr) => {
            let mut out = format!("TTY count: {}\n", mgr.count());
            for tty in &mgr.ttys {
                out.push_str(&format!(
                    "  tty{}: mode={:?} echo={} input={} output={} fg={:?}\n",
                    tty.id, tty.line_discipline, tty.echo,
                    tty.input_buf.len(), tty.output_buf.len(),
                    tty.foreground_pid,
                ));
            }
            out
        }
    }
}
