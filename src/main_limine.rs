#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;

/// Limine base revision -- we support revision 2+.
#[used]
#[link_section = ".limine_requests"]
static BASE_REVISION: [u64; 3] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc, 2];

// ---------------------------------------------------------------------------
// Limine protocol structures (defined inline to avoid external crate deps)
// ---------------------------------------------------------------------------

/// Framebuffer request.
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
    memory_model: u8,
    red_mask_size: u8,
    red_mask_shift: u8,
    green_mask_size: u8,
    green_mask_shift: u8,
    blue_mask_size: u8,
    blue_mask_shift: u8,
}

/// Memory map request.
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

#[allow(dead_code)]
const LIMINE_MEMMAP_USABLE: u64 = 0;

/// HHDM (Higher Half Direct Map) request.
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

/// RSDP request.
#[repr(C)]
struct LimineRsdpRequest {
    id: [u64; 4],
    revision: u64,
    response: *const LimineRsdpResponse,
}
unsafe impl Send for LimineRsdpRequest {}
unsafe impl Sync for LimineRsdpRequest {}

#[repr(C)]
struct LimineRsdpResponse {
    revision: u64,
    address: *const u8,
}

// ---------------------------------------------------------------------------
// Limine request statics -- the bootloader scans the ELF for these
// ---------------------------------------------------------------------------

// Request IDs from Limine spec (common prefix + per-feature suffix)
#[used]
#[link_section = ".limine_requests"]
static FRAMEBUFFER_REQUEST: LimineFramebufferRequest = LimineFramebufferRequest {
    id: [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x9d5827dcd881dd75, 0xa3148604f6fab11b],
    revision: 0,
    response: core::ptr::null(),
};

#[used]
#[link_section = ".limine_requests"]
static MEMMAP_REQUEST: LimineMemmapRequest = LimineMemmapRequest {
    id: [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x67cf3d9d378a806f, 0xe304acdfc50c3c62],
    revision: 0,
    response: core::ptr::null(),
};

#[used]
#[link_section = ".limine_requests"]
static HHDM_REQUEST: LimineHhdmRequest = LimineHhdmRequest {
    id: [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0x48dcf1cb8ad2b852, 0x63984e959a98244b],
    revision: 0,
    response: core::ptr::null(),
};

#[used]
#[link_section = ".limine_requests"]
static RSDP_REQUEST: LimineRsdpRequest = LimineRsdpRequest {
    id: [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b, 0xc5e77b6b397e7b43, 0x27637845accdcf3c],
    revision: 0,
    response: core::ptr::null(),
};

// ---------------------------------------------------------------------------
// Limine entry point
// ---------------------------------------------------------------------------

/// Limine entry point -- called by the bootloader after setting up
/// the higher-half direct map and filling in our request responses.
#[no_mangle]
extern "C" fn _start() -> ! {
    // Delegate to the shared Limine init sequence in the library
    unsafe { merlion_kernel::limine_init::limine_kernel_init(); }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();
    merlion_kernel::serial_println!("\n\x1b[31m══ KERNEL PANIC ══\x1b[0m");
    merlion_kernel::serial_println!("{}", info);
    loop { x86_64::instructions::hlt(); }
}
