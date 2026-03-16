/// Enhanced pipes for MerlionOS.
/// Provides named pipes (FIFOs), bidirectional pipes, pipe buffering,
/// and pipe status monitoring.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Default pipe buffer size (4 KiB).
const DEFAULT_BUF_SIZE: usize = 4096;

/// Maximum number of pipes.
const MAX_PIPES: usize = 64;

/// Maximum number of named pipes (FIFOs).
const MAX_FIFOS: usize = 32;

/// Pipe type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeType {
    Anonymous,
    Named,
    Bidirectional,
}

/// Result of polling a pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollResult {
    /// Data available to read (number of bytes).
    Readable(usize),
    /// Space available to write (number of bytes).
    Writable(usize),
    /// Pipe is closed.
    Closed,
    /// No data and no space (would block).
    WouldBlock,
}

/// A circular buffer backing a pipe.
struct CircularBuffer {
    data: Vec<u8>,
    capacity: usize,
    read_pos: usize,
    write_pos: usize,
    count: usize,
}

impl CircularBuffer {
    fn new(capacity: usize) -> Self {
        let mut data = Vec::new();
        data.resize(capacity, 0);
        Self { data, capacity, read_pos: 0, write_pos: 0, count: 0 }
    }

    fn available_read(&self) -> usize {
        self.count
    }

    fn available_write(&self) -> usize {
        self.capacity - self.count
    }

    fn write(&mut self, src: &[u8]) -> usize {
        let to_write = src.len().min(self.available_write());
        for i in 0..to_write {
            self.data[self.write_pos] = src[i];
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }
        self.count += to_write;
        to_write
    }

    fn read(&mut self, max: usize) -> Vec<u8> {
        let to_read = max.min(self.count);
        let mut result = Vec::with_capacity(to_read);
        for _ in 0..to_read {
            result.push(self.data[self.read_pos]);
            self.read_pos = (self.read_pos + 1) % self.capacity;
        }
        self.count -= to_read;
        result
    }
}

/// A pipe instance.
struct Pipe {
    id: u32,
    pipe_type: PipeType,
    /// Forward buffer (write end -> read end).
    fwd: CircularBuffer,
    /// Reverse buffer (only used for Bidirectional).
    rev: Option<CircularBuffer>,
    open: bool,
    bytes_written: u64,
    bytes_read: u64,
}

impl Pipe {
    fn new(id: u32, pipe_type: PipeType, buf_size: usize) -> Self {
        let rev = if pipe_type == PipeType::Bidirectional {
            Some(CircularBuffer::new(buf_size))
        } else {
            None
        };
        Self {
            id,
            pipe_type,
            fwd: CircularBuffer::new(buf_size),
            rev,
            open: true,
            bytes_written: 0,
            bytes_read: 0,
        }
    }
}

/// A named FIFO entry.
struct FifoEntry {
    path: String,
    pipe_id: u32,
}

/// Global pipe state.
struct PipeState {
    pipes: Vec<Pipe>,
    fifos: Vec<FifoEntry>,
    next_id: u32,
}

static STATE: Mutex<PipeState> = Mutex::new(PipeState {
    pipes: Vec::new(),
    fifos: Vec::new(),
    next_id: 1,
});

static TOTAL_BYTES_TX: AtomicU64 = AtomicU64::new(0);
static TOTAL_BYTES_RX: AtomicU64 = AtomicU64::new(0);

fn alloc_id(state: &mut PipeState) -> u32 {
    let id = state.next_id;
    state.next_id += 1;
    id
}

fn find_pipe(state: &PipeState, id: u32) -> Option<usize> {
    state.pipes.iter().position(|p| p.id == id)
}

// ── Named pipe (FIFO) API ──

/// Create a named pipe (FIFO) at the given path.
pub fn create_fifo(path: &str) -> Result<u32, &'static str> {
    let mut state = STATE.lock();
    if state.fifos.len() >= MAX_FIFOS {
        return Err("max FIFO count reached");
    }
    if state.fifos.iter().any(|f| f.path == path) {
        return Err("FIFO already exists at path");
    }
    if state.pipes.len() >= MAX_PIPES {
        return Err("max pipe count reached");
    }
    let id = alloc_id(&mut state);
    state.pipes.push(Pipe::new(id, PipeType::Named, DEFAULT_BUF_SIZE));
    state.fifos.push(FifoEntry { path: String::from(path), pipe_id: id });
    Ok(id)
}

/// Open an existing named pipe by path, returning its pipe ID.
pub fn open_fifo(path: &str) -> Result<u32, &'static str> {
    let state = STATE.lock();
    let entry = state.fifos.iter().find(|f| f.path == path)
        .ok_or("FIFO not found")?;
    Ok(entry.pipe_id)
}

/// Remove a named pipe.
pub fn remove_fifo(path: &str) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let fi = state.fifos.iter().position(|f| f.path == path)
        .ok_or("FIFO not found")?;
    let pipe_id = state.fifos[fi].pipe_id;
    state.fifos.remove(fi);
    // Also close the underlying pipe.
    if let Some(pi) = find_pipe(&state, pipe_id) {
        state.pipes[pi].open = false;
    }
    Ok(())
}

// ── Pipe operations ──

/// Write data to a pipe. Returns number of bytes written.
pub fn write(pipe_id: u32, data: &[u8]) -> Result<usize, &'static str> {
    let mut state = STATE.lock();
    let pi = find_pipe(&state, pipe_id).ok_or("pipe not found")?;
    if !state.pipes[pi].open {
        return Err("pipe closed");
    }
    let n = state.pipes[pi].fwd.write(data);
    state.pipes[pi].bytes_written += n as u64;
    TOTAL_BYTES_TX.fetch_add(n as u64, Ordering::Relaxed);
    Ok(n)
}

/// Read up to `max` bytes from a pipe.
pub fn read(pipe_id: u32, max: usize) -> Result<Vec<u8>, &'static str> {
    let mut state = STATE.lock();
    let pi = find_pipe(&state, pipe_id).ok_or("pipe not found")?;
    if !state.pipes[pi].open && state.pipes[pi].fwd.available_read() == 0 {
        return Err("pipe closed");
    }
    let data = state.pipes[pi].fwd.read(max);
    let n = data.len() as u64;
    state.pipes[pi].bytes_read += n;
    TOTAL_BYTES_RX.fetch_add(n, Ordering::Relaxed);
    Ok(data)
}

/// Close a pipe.
pub fn close(pipe_id: u32) -> Result<(), &'static str> {
    let mut state = STATE.lock();
    let pi = find_pipe(&state, pipe_id).ok_or("pipe not found")?;
    state.pipes[pi].open = false;
    Ok(())
}

// ── Non-blocking I/O ──

/// Try to read without blocking. Returns empty vec if no data.
pub fn try_read(pipe_id: u32, max: usize) -> Vec<u8> {
    let mut state = STATE.lock();
    if let Some(pi) = find_pipe(&state, pipe_id) {
        if state.pipes[pi].fwd.available_read() > 0 {
            let data = state.pipes[pi].fwd.read(max);
            let n = data.len() as u64;
            state.pipes[pi].bytes_read += n;
            TOTAL_BYTES_RX.fetch_add(n, Ordering::Relaxed);
            return data;
        }
    }
    Vec::new()
}

/// Try to write without blocking. Returns 0 if buffer full.
pub fn try_write(pipe_id: u32, data: &[u8]) -> usize {
    let mut state = STATE.lock();
    if let Some(pi) = find_pipe(&state, pipe_id) {
        if state.pipes[pi].open && state.pipes[pi].fwd.available_write() > 0 {
            let n = state.pipes[pi].fwd.write(data);
            state.pipes[pi].bytes_written += n as u64;
            TOTAL_BYTES_TX.fetch_add(n as u64, Ordering::Relaxed);
            return n;
        }
    }
    0
}

/// Poll a pipe for readiness.
pub fn poll(pipe_id: u32) -> PollResult {
    let state = STATE.lock();
    if let Some(pi) = find_pipe(&state, pipe_id) {
        let p = &state.pipes[pi];
        if !p.open && p.fwd.available_read() == 0 {
            return PollResult::Closed;
        }
        if p.fwd.available_read() > 0 {
            return PollResult::Readable(p.fwd.available_read());
        }
        if p.fwd.available_write() > 0 {
            return PollResult::Writable(p.fwd.available_write());
        }
        PollResult::WouldBlock
    } else {
        PollResult::Closed
    }
}

// ── Pipe pair ──

/// Create an anonymous pipe pair, returning (read_end_id, write_end_id).
/// Both IDs refer to the same underlying pipe.
pub fn create_pipe_pair() -> Result<(u32, u32), &'static str> {
    let mut state = STATE.lock();
    if state.pipes.len() >= MAX_PIPES {
        return Err("max pipe count reached");
    }
    let id = alloc_id(&mut state);
    state.pipes.push(Pipe::new(id, PipeType::Anonymous, DEFAULT_BUF_SIZE));
    // Return the same id for both ends — callers use write() and read().
    Ok((id, id))
}

/// Create a bidirectional pipe pair.
pub fn create_bidi_pair() -> Result<u32, &'static str> {
    let mut state = STATE.lock();
    if state.pipes.len() >= MAX_PIPES {
        return Err("max pipe count reached");
    }
    let id = alloc_id(&mut state);
    state.pipes.push(Pipe::new(id, PipeType::Bidirectional, DEFAULT_BUF_SIZE));
    Ok(id)
}

// ── Stats ──

/// Summary of active pipes.
pub fn pipe_info() -> String {
    let state = STATE.lock();
    let active = state.pipes.iter().filter(|p| p.open).count();
    let total = state.pipes.len();
    let fifos = state.fifos.len();
    let tx = TOTAL_BYTES_TX.load(Ordering::Relaxed);
    let rx = TOTAL_BYTES_RX.load(Ordering::Relaxed);

    let mut out = format!("Pipes: {} active / {} total, {} FIFOs\n", active, total, fifos);
    out.push_str(&format!("Bytes transferred: {} TX, {} RX\n", tx, rx));

    if !state.fifos.is_empty() {
        out.push_str("Named pipes:\n");
        for f in &state.fifos {
            out.push_str(&format!("  {} (id: {})\n", f.path, f.pipe_id));
        }
    }

    for p in &state.pipes {
        let typ = match p.pipe_type {
            PipeType::Anonymous => "anon",
            PipeType::Named => "fifo",
            PipeType::Bidirectional => "bidi",
        };
        let status = if p.open { "open" } else { "closed" };
        out.push_str(&format!("  pipe {} [{}] {} — buf {}/{}, w={} r={}\n",
            p.id, typ, status,
            p.fwd.count, p.fwd.capacity,
            p.bytes_written, p.bytes_read));
    }
    out
}

/// Initialize pipe2 subsystem.
pub fn init() {
    crate::serial_println!("[pipe2] enhanced pipe subsystem initialized");
}
