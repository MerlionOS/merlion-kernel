/// VideoCore mailbox interface for Raspberry Pi.
/// Communicates with the GPU to query hardware info,
/// request framebuffer, and configure clocks.

use alloc::string::String;
use alloc::format;

// ---------------------------------------------------------------------------
// Mailbox registers (Pi 3 / QEMU raspi3b)
// ---------------------------------------------------------------------------

const MAILBOX_BASE: u64   = 0x3F00_B880;
const MAILBOX_READ: u64   = MAILBOX_BASE + 0x00;
const MAILBOX_STATUS: u64 = MAILBOX_BASE + 0x18;
const MAILBOX_WRITE: u64  = MAILBOX_BASE + 0x20;

/// Status register flags.
const MAILBOX_FULL: u32  = 1 << 31;
const MAILBOX_EMPTY: u32 = 1 << 30;

/// Property tags channel.
const CHANNEL_PROP: u32 = 8;

// ---------------------------------------------------------------------------
// Property tags
// ---------------------------------------------------------------------------

const TAG_GET_BOARD_REVISION: u32 = 0x0001_0002;
const TAG_GET_MAC_ADDRESS: u32    = 0x0001_0003;
const TAG_GET_SERIAL: u32        = 0x0001_0004;
const TAG_GET_ARM_MEMORY: u32    = 0x0001_0005;
const TAG_GET_VC_MEMORY: u32     = 0x0001_0006;
const TAG_SET_CLOCK_RATE: u32    = 0x0003_8002;

/// Request code sent to GPU.
const REQUEST_CODE: u32   = 0x0000_0000;
/// Successful response code from GPU.
const RESPONSE_OK: u32    = 0x8000_0000;
/// End tag sentinel.
const TAG_END: u32         = 0x0000_0000;

// ---------------------------------------------------------------------------
// Mailbox buffer — must be 16-byte aligned
// ---------------------------------------------------------------------------

/// A 16-byte aligned buffer for mailbox property requests.
#[repr(C, align(16))]
struct MailboxBuffer {
    data: [u32; 36],
}

// ---------------------------------------------------------------------------
// Low-level mailbox read/write
// ---------------------------------------------------------------------------

/// Write a message (buffer address | channel) to the mailbox.
#[allow(unused_variables)]
fn mailbox_write_raw(channel: u32, data: u32) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        // Wait until mailbox is not full
        while mmio_read(MAILBOX_STATUS) & MAILBOX_FULL != 0 {
            core::hint::spin_loop();
        }
        mmio_write(MAILBOX_WRITE, (data & !0xF) | (channel & 0xF));
    }
}

/// Read a response from the mailbox on the given channel.
#[allow(unused_variables)]
fn mailbox_read_raw(channel: u32) -> u32 {
    #[cfg(target_arch = "aarch64")]
    {
        loop {
            unsafe {
                // Wait until mailbox is not empty
                while mmio_read(MAILBOX_STATUS) & MAILBOX_EMPTY != 0 {
                    core::hint::spin_loop();
                }
                let val = mmio_read(MAILBOX_READ);
                if (val & 0xF) == channel {
                    return val & !0xF;
                }
            }
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    { 0 }
}

/// Send a property tag request and wait for the response.
/// Returns true if the GPU responded successfully.
fn mailbox_call(buf: &mut MailboxBuffer) -> bool {
    let addr = buf.data.as_ptr() as u32;
    mailbox_write_raw(CHANNEL_PROP, addr);
    let _ = mailbox_read_raw(CHANNEL_PROP);
    buf.data[1] == RESPONSE_OK
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the mailbox interface (currently a no-op).
pub fn init() {
    // Nothing needed — the mailbox hardware is always available.
}

/// Query the ARM memory region: returns (base_address, size_in_bytes).
pub fn get_arm_memory() -> (u64, u64) {
    #[cfg(target_arch = "aarch64")]
    {
        let mut buf = MailboxBuffer { data: [0; 36] };
        buf.data[0] = 8 * 4;             // buffer size in bytes
        buf.data[1] = REQUEST_CODE;
        buf.data[2] = TAG_GET_ARM_MEMORY;
        buf.data[3] = 8;                 // value buffer size
        buf.data[4] = 0;                 // request code
        buf.data[5] = 0;                 // base (filled by GPU)
        buf.data[6] = 0;                 // size (filled by GPU)
        buf.data[7] = TAG_END;

        if mailbox_call(&mut buf) {
            (buf.data[5] as u64, buf.data[6] as u64)
        } else {
            (0, 0)
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    { (0, 256 * 1024 * 1024) } // fake 256 MiB
}

/// Query the board revision number.
pub fn get_board_revision() -> u32 {
    #[cfg(target_arch = "aarch64")]
    {
        let mut buf = MailboxBuffer { data: [0; 36] };
        buf.data[0] = 7 * 4;
        buf.data[1] = REQUEST_CODE;
        buf.data[2] = TAG_GET_BOARD_REVISION;
        buf.data[3] = 4;
        buf.data[4] = 0;
        buf.data[5] = 0;
        buf.data[6] = TAG_END;

        if mailbox_call(&mut buf) {
            buf.data[5]
        } else {
            0
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    { 0x00A02082 } // fake Pi 3 B rev 1.2
}

/// Query the board serial number (64-bit).
pub fn get_serial() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        let mut buf = MailboxBuffer { data: [0; 36] };
        buf.data[0] = 8 * 4;
        buf.data[1] = REQUEST_CODE;
        buf.data[2] = TAG_GET_SERIAL;
        buf.data[3] = 8;
        buf.data[4] = 0;
        buf.data[5] = 0;
        buf.data[6] = 0;
        buf.data[7] = TAG_END;

        if mailbox_call(&mut buf) {
            (buf.data[5] as u64) | ((buf.data[6] as u64) << 32)
        } else {
            0
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    { 0x0000_0000_DEAD_BEEF }
}

/// Query the Ethernet MAC address.
pub fn get_mac_address() -> [u8; 6] {
    #[cfg(target_arch = "aarch64")]
    {
        let mut buf = MailboxBuffer { data: [0; 36] };
        buf.data[0] = 8 * 4;
        buf.data[1] = REQUEST_CODE;
        buf.data[2] = TAG_GET_MAC_ADDRESS;
        buf.data[3] = 6;
        buf.data[4] = 0;
        buf.data[5] = 0;
        buf.data[6] = 0;
        buf.data[7] = TAG_END;

        if mailbox_call(&mut buf) {
            let lo = buf.data[5].to_le_bytes();
            let hi = buf.data[6].to_le_bytes();
            [lo[0], lo[1], lo[2], lo[3], hi[0], hi[1]]
        } else {
            [0; 6]
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    { [0xB8, 0x27, 0xEB, 0x00, 0x00, 0x01] } // fake Pi MAC
}

/// Return a formatted string with hardware info from the mailbox.
pub fn mailbox_info() -> String {
    let (mem_base, mem_size) = get_arm_memory();
    let rev = get_board_revision();
    let serial = get_serial();
    let mac = get_mac_address();
    format!(
        "Board revision: 0x{:08X}\n\
         Serial: 0x{:016X}\n\
         ARM memory: 0x{:X} ({} MiB)\n\
         MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        rev,
        serial,
        mem_base,
        mem_size / (1024 * 1024),
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_write(reg: u64, val: u32) {
    core::ptr::write_volatile(reg as *mut u32, val);
}

#[cfg(target_arch = "aarch64")]
unsafe fn mmio_read(reg: u64) -> u32 {
    core::ptr::read_volatile(reg as *const u32)
}
