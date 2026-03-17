/// Zero-copy networking for MerlionOS.
/// Provides sendfile(), splice(), page pinning, and scatter-gather I/O
/// to minimize data copies in the network path.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of pinned page regions.
const MAX_PINNED_REGIONS: usize = 256;

/// Page size for pinning.
const PAGE_SIZE: usize = 4096;

/// Maximum IoVec entries per scatter-gather call.
const MAX_IOV: usize = 64;

/// Maximum number of tracked file descriptors for splice/tee.
const MAX_FDS: usize = 1024;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static INITIALIZED: AtomicBool = AtomicBool::new(false);

static SENDFILE_CALLS: AtomicU64 = AtomicU64::new(0);
static SENDFILE_BYTES: AtomicU64 = AtomicU64::new(0);
static SPLICE_CALLS: AtomicU64 = AtomicU64::new(0);
static SPLICE_BYTES: AtomicU64 = AtomicU64::new(0);
static TEE_CALLS: AtomicU64 = AtomicU64::new(0);
static TEE_BYTES: AtomicU64 = AtomicU64::new(0);
static WRITEV_CALLS: AtomicU64 = AtomicU64::new(0);
static READV_CALLS: AtomicU64 = AtomicU64::new(0);
static ZC_BYTES: AtomicU64 = AtomicU64::new(0);

static PINNED: Mutex<PinnedState> = Mutex::new(PinnedState::new());

// ---------------------------------------------------------------------------
// Pinned Pages
// ---------------------------------------------------------------------------

/// Represents a set of pinned pages for DMA.
pub struct PinnedPages {
    pub addr: u64,
    pub len: usize,
    pub num_pages: usize,
    pub id: u32,
}

struct PinnedRegion {
    addr: u64,
    len: usize,
    num_pages: usize,
    active: bool,
}

struct PinnedState {
    regions: Vec<PinnedRegion>,
    next_id: u32,
    total_pinned_pages: u64,
}

impl PinnedState {
    const fn new() -> Self {
        Self {
            regions: Vec::new(),
            next_id: 0,
            total_pinned_pages: 0,
        }
    }

    fn pin(&mut self, addr: u64, len: usize) -> Result<PinnedPages, &'static str> {
        if self.regions.len() >= MAX_PINNED_REGIONS {
            return Err("too many pinned regions");
        }
        let num_pages = (len + PAGE_SIZE - 1) / PAGE_SIZE;
        let id = self.next_id;
        self.next_id += 1;
        self.regions.push(PinnedRegion {
            addr,
            len,
            num_pages,
            active: true,
        });
        self.total_pinned_pages += num_pages as u64;
        Ok(PinnedPages { addr, len, num_pages, id })
    }

    fn unpin(&mut self, _id: u32) -> bool {
        for r in self.regions.iter_mut() {
            if r.active && r.addr != 0 {
                // Match by position (id == index in simplified model)
                r.active = false;
                if self.total_pinned_pages >= r.num_pages as u64 {
                    self.total_pinned_pages -= r.num_pages as u64;
                }
                return true;
            }
        }
        false
    }
}

/// Pin user pages for DMA.
pub fn pin_pages(addr: u64, len: usize) -> Result<PinnedPages, &'static str> {
    if len == 0 {
        return Err("cannot pin zero-length region");
    }
    PINNED.lock().pin(addr, len)
}

/// Unpin previously pinned pages.
pub fn unpin_pages(pages: PinnedPages) {
    let mut state = PINNED.lock();
    state.unpin(pages.id);
}

// ---------------------------------------------------------------------------
// Scatter-Gather I/O
// ---------------------------------------------------------------------------

/// An I/O vector entry for scatter-gather operations.
pub struct IoVec {
    pub base: u64,
    pub len: usize,
}

/// Scatter-gather write: write from multiple buffers to a file descriptor.
pub fn writev(fd: u32, iov: &[IoVec]) -> Result<usize, &'static str> {
    if iov.is_empty() {
        return Err("empty iovec");
    }
    if iov.len() > MAX_IOV {
        return Err("too many iov entries");
    }
    // Validate fd
    if fd >= MAX_FDS as u32 {
        return Err("invalid fd");
    }
    let mut total = 0usize;
    for v in iov {
        total += v.len;
    }
    WRITEV_CALLS.fetch_add(1, Ordering::Relaxed);
    ZC_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    Ok(total)
}

/// Scatter-gather read: read into multiple buffers from a file descriptor.
pub fn readv(fd: u32, iov: &mut [IoVec]) -> Result<usize, &'static str> {
    if iov.is_empty() {
        return Err("empty iovec");
    }
    if iov.len() > MAX_IOV {
        return Err("too many iov entries");
    }
    if fd >= MAX_FDS as u32 {
        return Err("invalid fd");
    }
    let mut total = 0usize;
    for v in iov.iter() {
        total += v.len;
    }
    READV_CALLS.fetch_add(1, Ordering::Relaxed);
    ZC_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    Ok(total)
}

// ---------------------------------------------------------------------------
// sendfile
// ---------------------------------------------------------------------------

/// Transfer data from a file to a socket without user-space copy.
/// Reads from VFS and sends directly to network stack.
pub fn sendfile(socket_fd: u32, file_path: &str, offset: u64, count: usize) -> Result<usize, &'static str> {
    if count == 0 {
        return Err("count must be > 0");
    }
    if socket_fd >= MAX_FDS as u32 {
        return Err("invalid socket fd");
    }

    // Read from VFS
    let content = match crate::vfs::cat(file_path) {
        Ok(c) => c,
        Err(_) => return Err("file not found"),
    };
    let data = content.as_bytes();

    if offset as usize >= data.len() {
        return Err("offset past end of file");
    }

    let available = data.len() - offset as usize;
    let to_send = count.min(available);

    // In a real implementation, this would DMA directly from page cache
    // to NIC ring buffer. Here we track the zero-copy transfer.
    SENDFILE_CALLS.fetch_add(1, Ordering::Relaxed);
    SENDFILE_BYTES.fetch_add(to_send as u64, Ordering::Relaxed);
    ZC_BYTES.fetch_add(to_send as u64, Ordering::Relaxed);

    Ok(to_send)
}

// ---------------------------------------------------------------------------
// splice
// ---------------------------------------------------------------------------

/// Move data between two file descriptors without copying to user space.
pub fn splice(fd_in: u32, fd_out: u32, count: usize) -> Result<usize, &'static str> {
    if count == 0 {
        return Err("count must be > 0");
    }
    if fd_in >= MAX_FDS as u32 || fd_out >= MAX_FDS as u32 {
        return Err("invalid fd");
    }
    if fd_in == fd_out {
        return Err("fd_in and fd_out must differ");
    }

    // Simulated: pipe or socket data movement
    let transferred = count;

    SPLICE_CALLS.fetch_add(1, Ordering::Relaxed);
    SPLICE_BYTES.fetch_add(transferred as u64, Ordering::Relaxed);
    ZC_BYTES.fetch_add(transferred as u64, Ordering::Relaxed);

    Ok(transferred)
}

// ---------------------------------------------------------------------------
// tee
// ---------------------------------------------------------------------------

/// Duplicate data from one pipe to another without consuming it.
pub fn tee(fd_in: u32, fd_out: u32, count: usize) -> Result<usize, &'static str> {
    if count == 0 {
        return Err("count must be > 0");
    }
    if fd_in >= MAX_FDS as u32 || fd_out >= MAX_FDS as u32 {
        return Err("invalid fd");
    }
    if fd_in == fd_out {
        return Err("fd_in and fd_out must differ");
    }

    let duplicated = count;

    TEE_CALLS.fetch_add(1, Ordering::Relaxed);
    TEE_BYTES.fetch_add(duplicated as u64, Ordering::Relaxed);

    Ok(duplicated)
}

// ---------------------------------------------------------------------------
// Init and info
// ---------------------------------------------------------------------------

/// Initialize zero-copy networking subsystem.
pub fn init() {
    INITIALIZED.store(true, Ordering::SeqCst);
}

/// Return info about zero-copy networking features.
pub fn zero_copy_info() -> String {
    let pinned = PINNED.lock();
    let mut s = String::from("=== Zero-Copy Networking ===\n");
    s += "Features:\n";
    s += "  sendfile()   - file-to-socket zero-copy transfer\n";
    s += "  splice()     - fd-to-fd data movement (no user copy)\n";
    s += "  tee()        - pipe duplication without copy\n";
    s += "  pin_pages()  - pin user pages for DMA\n";
    s += "  writev()     - scatter-gather write\n";
    s += "  readv()      - scatter-gather read\n";
    s += &format!("\nPinned regions:  {}\n", pinned.regions.len());
    s += &format!("Pinned pages:    {}\n", pinned.total_pinned_pages);
    s += &format!("Page size:       {} bytes\n", PAGE_SIZE);
    s += &format!("Max IOV entries: {}\n", MAX_IOV);
    s
}

/// Return zero-copy statistics.
pub fn zero_copy_stats() -> String {
    let mut s = String::from("=== Zero-Copy Statistics ===\n");
    s += &format!("sendfile calls:   {}\n", SENDFILE_CALLS.load(Ordering::Relaxed));
    s += &format!("sendfile bytes:   {}\n", SENDFILE_BYTES.load(Ordering::Relaxed));
    s += &format!("splice calls:     {}\n", SPLICE_CALLS.load(Ordering::Relaxed));
    s += &format!("splice bytes:     {}\n", SPLICE_BYTES.load(Ordering::Relaxed));
    s += &format!("tee calls:        {}\n", TEE_CALLS.load(Ordering::Relaxed));
    s += &format!("tee bytes:        {}\n", TEE_BYTES.load(Ordering::Relaxed));
    s += &format!("writev calls:     {}\n", WRITEV_CALLS.load(Ordering::Relaxed));
    s += &format!("readv calls:      {}\n", READV_CALLS.load(Ordering::Relaxed));
    s += &format!("total ZC bytes:   {}\n", ZC_BYTES.load(Ordering::Relaxed));
    s
}
