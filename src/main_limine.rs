#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;
use x86_64::VirtAddr;

// ---------------------------------------------------------------------------
// Limine protocol structures (inline — no external crate needed)
// ---------------------------------------------------------------------------

#[repr(C)]
struct LimineFramebufferRequest {
    id: [u64; 4],
    revision: u64,
    response: *const LimineFramebufferResponse,
}
unsafe impl Send for LimineFramebufferRequest {}
unsafe impl Sync for LimineFramebufferRequest {}

#[repr(C)]
struct LimineFramebufferResponse {
    revision: u64,
    framebuffer_count: u64,
    framebuffers: *const *const LimineFramebuffer,
}

#[repr(C)]
struct LimineFramebuffer {
    address: *mut u8,
    width: u64,
    height: u64,
    pitch: u64,
    bpp: u16,
}

#[repr(C)]
struct LimineMemmapRequest {
    id: [u64; 4],
    revision: u64,
    response: *const LimineMemmapResponse,
}
unsafe impl Send for LimineMemmapRequest {}
unsafe impl Sync for LimineMemmapRequest {}

#[repr(C)]
struct LimineMemmapResponse {
    revision: u64,
    entry_count: u64,
    entries: *const *const LimineMemmapEntry,
}

#[repr(C)]
struct LimineMemmapEntry {
    base: u64,
    length: u64,
    entry_type: u64,
}

const LIMINE_MEMMAP_USABLE: u64 = 0;

#[repr(C)]
struct LimineHhdmRequest {
    id: [u64; 4],
    revision: u64,
    response: *const LimineHhdmResponse,
}
unsafe impl Send for LimineHhdmRequest {}
unsafe impl Sync for LimineHhdmRequest {}

#[repr(C)]
struct LimineHhdmResponse {
    revision: u64,
    offset: u64,
}

// Limine requests start marker (MUST be first in .limine_requests section)
// NOTE: mut statics because Limine writes response pointers into these
#[used]
#[link_section = ".limine_requests_start"]
static mut REQUESTS_START_MARKER: [u64; 4] = [
    0xf6b8f4b39de7d1ae, 0xfab91a6940fcb9cf,
    0x785c6ed015d3e316, 0x181e920a7852b9d9,
];

// Limine base revision marker
#[used]
#[link_section = ".limine_requests"]
static mut BASE_REVISION: [u64; 3] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc, 2];

#[used]
#[link_section = ".limine_requests"]
static mut FRAMEBUFFER_REQUEST: LimineFramebufferRequest = LimineFramebufferRequest {
    id: [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x9d5827dcd881dd75, 0xa3148604f6fab11b],
    revision: 0,
    response: core::ptr::null(),
};

#[used]
#[link_section = ".limine_requests"]
static mut MEMMAP_REQUEST: LimineMemmapRequest = LimineMemmapRequest {
    id: [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x67cf3d9d378a806f, 0xe304acdfc50c3c62],
    revision: 0,
    response: core::ptr::null(),
};

#[used]
#[link_section = ".limine_requests"]
static mut HHDM_REQUEST: LimineHhdmRequest = LimineHhdmRequest {
    id: [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x48dcf1cb8ad2b852, 0x63984e959a98244b],
    revision: 0,
    response: core::ptr::null(),
};

// ---------------------------------------------------------------------------
// Simple bump frame allocator using Limine memory map
// ---------------------------------------------------------------------------

use x86_64::structures::paging::{FrameAllocator, PageTable, PhysFrame, Size4KiB};
use x86_64::PhysAddr;
use core::sync::atomic::{AtomicU64, Ordering};

static USABLE_START: AtomicU64 = AtomicU64::new(0);
static USABLE_END: AtomicU64 = AtomicU64::new(0);
static NEXT_FRAME: AtomicU64 = AtomicU64::new(0);
static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

struct LimineBumpAllocator;

unsafe impl FrameAllocator<Size4KiB> for LimineBumpAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let end = USABLE_END.load(Ordering::SeqCst);
        loop {
            let addr = NEXT_FRAME.load(Ordering::SeqCst);
            if addr >= end { return None; }
            let next = addr + 4096;
            if NEXT_FRAME.compare_exchange(addr, next, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                return Some(PhysFrame::containing_address(PhysAddr::new(addr)));
            }
        }
    }
}

// Limine requests end marker (MUST be last in .limine_requests section)
#[used]
#[link_section = ".limine_requests_end"]
static mut REQUESTS_END_MARKER: [u64; 2] = [0xadc0e0531bb10d03, 0x9572709f31764c62];

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[no_mangle]
extern "C" fn _start() -> ! {
    // Ultra-early serial: write directly to COM1 port 0x3F8
    // This works before ANY initialization, proving _start was called
    unsafe {
        // Init COM1: disable interrupts, set baud rate 115200, 8N1
        x86_64::instructions::port::Port::<u8>::new(0x3F8 + 1).write(0x00); // disable interrupts
        x86_64::instructions::port::Port::<u8>::new(0x3F8 + 3).write(0x80); // DLAB on
        x86_64::instructions::port::Port::<u8>::new(0x3F8 + 0).write(0x01); // baud 115200
        x86_64::instructions::port::Port::<u8>::new(0x3F8 + 1).write(0x00);
        x86_64::instructions::port::Port::<u8>::new(0x3F8 + 3).write(0x03); // 8N1
        x86_64::instructions::port::Port::<u8>::new(0x3F8 + 2).write(0xC7); // FIFO
        // Write "OK\n" directly
        for &b in b"[limine] _start reached!\r\n" {
            while x86_64::instructions::port::Port::<u8>::new(0x3F8 + 5).read() & 0x20 == 0 {}
            x86_64::instructions::port::Port::<u8>::new(0x3F8).write(b);
        }
    }

    // Phase 1: Serial output (works without memory setup)
    merlion_kernel::serial::SERIAL1.lock().init();
    merlion_kernel::serial_println!("{}", merlion_kernel::version::banner());
    merlion_kernel::serial_println!("[limine] Booting via Limine UEFI...");

    // Disable VGA text mode — UEFI doesn't provide VGA text buffer at 0xB8000.
    // All console output goes to serial. The framebuffer console (fbconsole)
    // handles pixel-based display if a GOP framebuffer is available.
    merlion_kernel::vga::disable_vga();

    // Phase 2: Read HHDM offset from Limine response
    let hhdm_offset = unsafe {
        let resp = (*(&raw const HHDM_REQUEST)).response;
        if resp.is_null() {
            merlion_kernel::serial_println!("[limine] ERROR: no HHDM response!");
            halt();
        }
        let offset = (*resp).offset;
        merlion_kernel::serial_println!("[limine] HHDM offset: {:#x}", offset);
        HHDM_OFFSET.store(offset, Ordering::SeqCst);
        offset
    };

    // Phase 3: Read memory map, find largest usable region
    unsafe {
        let resp = (*(&raw const MEMMAP_REQUEST)).response;
        if resp.is_null() {
            merlion_kernel::serial_println!("[limine] ERROR: no memory map response!");
            halt();
        }
        let count = (*resp).entry_count as usize;
        let entries = (*resp).entries;
        merlion_kernel::serial_println!("[limine] Memory map: {} entries", count);

        let mut best_base: u64 = 0;
        let mut best_len: u64 = 0;
        let mut total_usable: u64 = 0;

        for i in 0..count {
            let entry = *entries.add(i);
            let base = (*entry).base;
            let len = (*entry).length;
            let etype = (*entry).entry_type;

            if etype == LIMINE_MEMMAP_USABLE {
                total_usable += len;
                // Pick the largest usable region (skip first 1MB to be safe)
                if len > best_len && base >= 0x10_0000 {
                    best_base = base;
                    best_len = len;
                }
            }
        }

        merlion_kernel::serial_println!("[limine] Total usable: {} MiB", total_usable / (1024*1024));
        merlion_kernel::serial_println!("[limine] Best region: {:#x}..{:#x} ({} MiB)",
            best_base, best_base + best_len, best_len / (1024*1024));

        USABLE_START.store(best_base, Ordering::SeqCst);
        USABLE_END.store(best_base + best_len, Ordering::SeqCst);
        NEXT_FRAME.store(best_base, Ordering::SeqCst);
    }

    // Set global PHYS_MEM_OFFSET so phys_to_virt() works everywhere
    unsafe { merlion_kernel::memory::set_phys_mem_offset(hhdm_offset); }

    // Phase 4: CPU tables
    merlion_kernel::gdt::init();
    merlion_kernel::serial_println!("[ok] GDT loaded");

    merlion_kernel::timer::init();
    merlion_kernel::serial_println!("[ok] PIT configured");

    merlion_kernel::interrupts::init();
    merlion_kernel::serial_println!("[ok] IDT + interrupts enabled");

    // Phase 5: Page table + heap
    let phys_mem_virt = VirtAddr::new(hhdm_offset);
    unsafe {
        use x86_64::registers::control::Cr3;
        let (frame, _) = Cr3::read();
        let phys = frame.start_address();
        let virt = phys_mem_virt + phys.as_u64();
        let table: &mut PageTable = &mut *virt.as_mut_ptr();
        let mut mapper = x86_64::structures::paging::OffsetPageTable::new(table, phys_mem_virt);

        merlion_kernel::serial_println!("[ok] Page table mapped via HHDM");

        let mut fa = LimineBumpAllocator;
        merlion_kernel::allocator::init(&mut mapper, &mut fa)
            .expect("heap init failed");
        merlion_kernel::serial_println!("[ok] Heap ready ({}K)", merlion_kernel::allocator::HEAP_SIZE / 1024);
    }

    // Phase 6: Framebuffer
    unsafe {
        let resp = (*(&raw const FRAMEBUFFER_REQUEST)).response;
        if !resp.is_null() && (*resp).framebuffer_count > 0 {
            let fb = *(*resp).framebuffers;
            merlion_kernel::serial_println!("[limine] Framebuffer: {}x{} bpp={} at {:p}",
                (*fb).width, (*fb).height, (*fb).bpp, (*fb).address);
        } else {
            merlion_kernel::serial_println!("[limine] No framebuffer available");
        }
    }

    // Phase 7: Core subsystems (lazy init for 120+ modules)
    merlion_kernel::task::init();
    merlion_kernel::serial_println!("[ok] Task system");

    merlion_kernel::vfs::init();
    merlion_kernel::serial_println!("[ok] VFS");

    merlion_kernel::driver::init();
    merlion_kernel::module::init();
    merlion_kernel::ksyms::init();
    merlion_kernel::slab::init();
    merlion_kernel::blkdev::init();
    merlion_kernel::fd::init();
    merlion_kernel::env::init();
    merlion_kernel::serial_println!("[ok] Core subsystems");

    merlion_kernel::smp::init();
    merlion_kernel::apic_timer::init();
    merlion_kernel::e1000e::init();
    merlion_kernel::netstack::init();
    merlion_kernel::usb_hid::init();
    merlion_kernel::serial_println!("[ok] Hardware drivers");

    merlion_kernel::security::init();
    merlion_kernel::capability::init();
    merlion_kernel::kconfig::load();
    merlion_kernel::script::create_default_init();
    merlion_kernel::serial_println!("[ok] All subsystems initialized (lazy init for 120+ modules)");

    // Phase 8: RTC + login
    let dt = merlion_kernel::rtc::read();
    merlion_kernel::serial_println!("[ok] RTC: {}", dt);
    merlion_kernel::serial_println!("Kernel initialization complete (Limine UEFI).");

    merlion_kernel::login::show();
    while merlion_kernel::login::is_logging_in() {
        x86_64::instructions::hlt();
    }

    merlion_kernel::serial_println!("Shell active.");
    merlion_kernel::shell::prompt();

    loop { x86_64::instructions::hlt(); }
}

fn halt() -> ! {
    loop { x86_64::instructions::hlt(); }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();
    merlion_kernel::serial_println!("\n══ KERNEL PANIC ══");
    merlion_kernel::serial_println!("{}", info);
    halt()
}
