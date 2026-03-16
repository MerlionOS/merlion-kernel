/// Pipe filesystem for MerlionOS inter-process communication.
/// Provides Unix-like anonymous pipes and named FIFOs. Each pipe is a 4096-byte
/// ring buffer with separate read and write file descriptors.

use alloc::vec::Vec;
use spin::Mutex;

/// Size of the internal ring buffer for each pipe.
const PIPE_BUF_SIZE: usize = 4096;

/// Maximum number of pipes that can exist simultaneously.
const MAX_PIPES: usize = 16;

/// A unidirectional byte stream backed by a fixed-size ring buffer.
/// Data flows from writers into `read_buf` and out to readers in FIFO order.
/// Tracks open reader/writer counts; delivers EOF once all writers close.
pub struct Pipe {
    /// Ring buffer holding in-flight bytes.
    read_buf: [u8; PIPE_BUF_SIZE],
    /// Next position to write into `read_buf`.
    write_pos: usize,
    /// Next position to read from `read_buf`.
    read_pos: usize,
    /// Number of bytes currently in the buffer.
    count: usize,
    /// Number of open read-end file descriptors.
    readers: u8,
    /// Number of open write-end file descriptors.
    writers: u8,
    /// True once both ends have been closed and the pipe is defunct.
    closed: bool,
}

impl Pipe {
    /// Create a new empty pipe with one reader and one writer.
    const fn new() -> Self {
        Self {
            read_buf: [0u8; PIPE_BUF_SIZE],
            write_pos: 0,
            read_pos: 0,
            count: 0,
            readers: 1,
            writers: 1,
            closed: false,
        }
    }

    /// Returns the number of bytes available to read.
    fn available(&self) -> usize { self.count }

    /// Returns the number of bytes of free space in the buffer.
    fn free_space(&self) -> usize { PIPE_BUF_SIZE - self.count }

    /// Write bytes into the ring buffer. Returns bytes written (short write if
    /// buffer is partially full, 0 if closed or no readers).
    fn write(&mut self, data: &[u8]) -> usize {
        if self.closed || self.readers == 0 { return 0; }
        let to_write = data.len().min(self.free_space());
        for &byte in &data[..to_write] {
            self.read_buf[self.write_pos] = byte;
            self.write_pos = (self.write_pos + 1) % PIPE_BUF_SIZE;
            self.count += 1;
        }
        to_write
    }

    /// Read bytes from the ring buffer into `buf`. Returns bytes read.
    /// Returns 0 when empty and all writers have closed (EOF).
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.available());
        for slot in buf[..to_read].iter_mut() {
            *slot = self.read_buf[self.read_pos];
            self.read_pos = (self.read_pos + 1) % PIPE_BUF_SIZE;
            self.count -= 1;
        }
        to_read
    }

    /// Close one end. Marks the pipe fully closed when both ends reach zero.
    fn close_end(&mut self, is_write_end: bool) {
        if is_write_end {
            self.writers = self.writers.saturating_sub(1);
        } else {
            self.readers = self.readers.saturating_sub(1);
        }
        if self.readers == 0 && self.writers == 0 {
            self.closed = true;
        }
    }
}

/// Slot in the global pipe table -- either free or holding an active pipe.
enum PipeSlot {
    Free,
    Active(Pipe),
}

/// Global pipe table protected by a spinlock.
/// Every pipe is identified by its index (`pipe_id`).
pub struct PipeTable {
    slots: Vec<PipeSlot>,
}

impl PipeTable {
    /// Create an empty pipe table with `MAX_PIPES` free slots.
    fn new() -> Self {
        let mut slots = Vec::with_capacity(MAX_PIPES);
        for _ in 0..MAX_PIPES { slots.push(PipeSlot::Free); }
        Self { slots }
    }

    /// Allocate a new pipe, returning its `pipe_id`.
    fn alloc(&mut self) -> Option<usize> {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if matches!(slot, PipeSlot::Free) {
                *slot = PipeSlot::Active(Pipe::new());
                return Some(i);
            }
        }
        None
    }

    /// Get a mutable reference to an active pipe by id.
    fn get_mut(&mut self, id: usize) -> Option<&mut Pipe> {
        if id >= self.slots.len() { return None; }
        match &mut self.slots[id] {
            PipeSlot::Active(p) => Some(p),
            PipeSlot::Free => None,
        }
    }

    /// Reclaim a pipe slot if it has been fully closed.
    fn try_reclaim(&mut self, id: usize) {
        if id >= self.slots.len() { return; }
        if let PipeSlot::Active(ref p) = self.slots[id] {
            if p.closed { self.slots[id] = PipeSlot::Free; }
        }
    }
}

static PIPE_TABLE: Mutex<Option<PipeTable>> = Mutex::new(None);

/// Initialize the pipe filesystem. Must be called once during kernel boot.
pub fn init() {
    *PIPE_TABLE.lock() = Some(PipeTable::new());
}

/// Create an anonymous pipe. Returns `(read_fd, write_fd)` -- two file
/// descriptor numbers obtained from `crate::fd`.
pub fn create_pipe() -> Result<(usize, usize), &'static str> {
    let pipe_id = {
        let mut table = PIPE_TABLE.lock();
        let table = table.as_mut().ok_or("pipefs not initialized")?;
        table.alloc().ok_or("too many pipes")?
    };

    // Build pseudo-paths so the fd layer can track them.
    let read_path = alloc::format!("/dev/pipe/{}/r", pipe_id);
    let write_path = alloc::format!("/dev/pipe/{}/w", pipe_id);

    let read_fd = crate::fd::open(&read_path).map_err(|_| "cannot allocate read fd")?;
    let write_fd = crate::fd::open(&write_path).map_err(|e| {
        let _ = crate::fd::close(read_fd);
        e
    })?;
    Ok((read_fd, write_fd))
}

/// Write data into a pipe. Copies as many bytes as free space allows (short
/// write). Returns bytes written, or error on broken pipe / invalid id.
pub fn pipe_write(pipe_id: usize, data: &[u8]) -> Result<usize, &'static str> {
    let mut table = PIPE_TABLE.lock();
    let table = table.as_mut().ok_or("pipefs not initialized")?;
    let pipe = table.get_mut(pipe_id).ok_or("invalid pipe id")?;
    if pipe.readers == 0 { return Err("broken pipe: no readers"); }
    let written = pipe.write(data);
    if written == 0 && !data.is_empty() { return Err("pipe full"); }
    Ok(written)
}

/// Read data from a pipe. Returns bytes read, or 0 on EOF (buffer empty and
/// all writers closed). Error only on invalid pipe id.
pub fn pipe_read(pipe_id: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
    let mut table = PIPE_TABLE.lock();
    let table = table.as_mut().ok_or("pipefs not initialized")?;
    let pipe = table.get_mut(pipe_id).ok_or("invalid pipe id")?;
    let n = pipe.read(buf);
    Ok(n)
}

/// Close one end of a pipe. `is_write_end` selects which end. The pipe slot
/// is reclaimed automatically once both ends reach zero open descriptors.
pub fn close_pipe_end(pipe_id: usize, is_write_end: bool) {
    let mut table = PIPE_TABLE.lock();
    if let Some(table) = table.as_mut() {
        if let Some(pipe) = table.get_mut(pipe_id) {
            pipe.close_end(is_write_end);
        }
        table.try_reclaim(pipe_id);
    }
}

/// Create a named pipe (FIFO) visible in the VFS at `path`. Allocates a
/// backing pipe and writes a marker file so other tasks can discover it.
/// Returns the `pipe_id` for subsequent read/write calls.
pub fn named_pipe(path: &str) -> Result<usize, &'static str> {
    let pipe_id = {
        let mut table = PIPE_TABLE.lock();
        let table = table.as_mut().ok_or("pipefs not initialized")?;
        table.alloc().ok_or("too many pipes")?
    };
    let marker = alloc::format!("{{fifo:pipe_id={}}}", pipe_id);
    crate::vfs::write(path, &marker)?;
    Ok(pipe_id)
}

/// Information about a single pipe for diagnostic display.
pub struct PipeInfo {
    /// Pipe table index.
    pub id: usize,
    /// Bytes currently buffered.
    pub buffered: usize,
    /// Open reader count.
    pub readers: u8,
    /// Open writer count.
    pub writers: u8,
}

/// List all active (non-closed) pipes and their status.
pub fn list() -> Vec<PipeInfo> {
    let table = PIPE_TABLE.lock();
    let mut result = Vec::new();
    if let Some(ref table) = *table {
        for (i, slot) in table.slots.iter().enumerate() {
            if let PipeSlot::Active(ref p) = slot {
                if !p.closed {
                    result.push(PipeInfo {
                        id: i,
                        buffered: p.available(),
                        readers: p.readers,
                        writers: p.writers,
                    });
                }
            }
        }
    }
    result
}
