/// Kernel initialization sequence for Limine boot path.
///
/// When booting via Limine instead of `bootloader` 0.9, the kernel entry
/// point eventually calls [`limine_kernel_init`] to bring up all subsystems
/// in the correct order.
///
/// **Current status**: preparation module.  The actual Limine integration
/// requires adding the `limine` crate dependency, parsing real memory-map
/// responses, and restructuring `main.rs` with conditional compilation.
/// For now this module uses hardcoded physical memory parameters as a
/// fallback so the init sequence can be validated.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::paging::{
    FrameAllocator, OffsetPageTable, PageTable, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Limine maps all physical memory at this higher-half offset by default.
const LIMINE_HHDM_OFFSET: u64 = 0xffff_8000_0000_0000;

/// Fallback: assume usable physical RAM starts at 1 MiB.
const FALLBACK_PHYS_START: u64 = 0x10_0000;

/// Fallback: assume 128 MiB of usable physical RAM.
const FALLBACK_PHYS_SIZE: u64 = 128 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Boot method detection
// ---------------------------------------------------------------------------

/// Describes how the machine was booted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMethod {
    /// Traditional BIOS boot (legacy / CSM).
    Bios,
    /// UEFI boot (native or via Limine).
    Uefi,
}

/// Detect whether we are running under UEFI or legacy BIOS.
///
/// The heuristic probes the BIOS Data Area (BDA) at physical address
/// 0x400 through the HHDM.  In a BIOS boot the word at that address
/// contains the COM1 I/O port (typically 0x03F8).  Under pure UEFI
/// this region is often zeroed or repurposed by firmware.
///
/// **Note**: this is a best-effort check.  A production kernel should
/// rely on an explicit flag from the bootloader rather than heuristics.
pub fn detect_boot_method() -> BootMethod {
    let bda_com1: u16 = unsafe {
        let ptr = (LIMINE_HHDM_OFFSET + 0x400) as *const u16;
        core::ptr::read_volatile(ptr)
    };

    if bda_com1 == 0x03F8 || bda_com1 == 0x02F8 {
        BootMethod::Bios
    } else {
        BootMethod::Uefi
    }
}

// ---------------------------------------------------------------------------
// Simple bump frame allocator (fallback)
// ---------------------------------------------------------------------------

/// Atomic bump pointer tracking the next free physical address.
static NEXT_FREE_FRAME: AtomicU64 = AtomicU64::new(FALLBACK_PHYS_START);

/// A trivial bump allocator over a contiguous physical memory region.
///
/// Used as a stand-in until real Limine memory-map parsing is wired in.
/// Frames are never freed — this is acceptable during early boot.
pub struct FallbackFrameAllocator;

unsafe impl FrameAllocator<Size4KiB> for FallbackFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let end = FALLBACK_PHYS_START + FALLBACK_PHYS_SIZE;
        loop {
            let addr = NEXT_FREE_FRAME.load(Ordering::SeqCst);
            if addr >= end {
                return None;
            }
            let next = addr + 4096;
            if NEXT_FREE_FRAME
                .compare_exchange(addr, next, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Some(PhysFrame::containing_address(PhysAddr::new(addr)));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Main init sequence
// ---------------------------------------------------------------------------

/// Full kernel initialization when booted via the Limine protocol.
///
/// This mirrors the sequence in `main.rs::kernel_main` but does not depend
/// on `bootloader::BootInfo`.  Instead it uses hardcoded physical-memory
/// parameters as a temporary stand-in until the real Limine memory-map
/// response is wired in.
///
/// # Safety
/// Must be called exactly once, from the Limine entry point, with
/// interrupts disabled and a valid stack already set up by Limine.
pub unsafe fn limine_kernel_init() -> ! {
    // ---- Phase 1: early serial output ------------------------------------
    crate::serial::SERIAL1.lock().init();
    crate::serial_println!("[limine] Serial (COM1) initialised");
    crate::serial_println!("{}", crate::version::banner());
    crate::serial_println!("[limine] Booting via Limine path...");

    let boot = detect_boot_method();
    crate::serial_println!("[limine] Boot method: {:?}", boot);

    // ---- Phase 2: CPU tables & interrupts --------------------------------
    crate::gdt::init();
    crate::serial_println!("[ok] GDT loaded");

    crate::timer::init();
    crate::serial_println!("[ok] PIT configured");

    crate::interrupts::init();
    crate::serial_println!("[ok] IDT loaded, interrupts enabled");

    // ---- Phase 3: memory (fallback) --------------------------------------
    //
    // TODO(limine): replace with real memory-map parsing once the `limine`
    //       crate is added.  The proper flow is:
    //       1. Read MEMMAP_RESPONSE from limine_entry.rs
    //       2. Walk usable regions to seed the frame allocator
    //       3. Read HHDM_RESPONSE for the actual offset
    let phys_mem_offset = VirtAddr::new(LIMINE_HHDM_OFFSET);

    crate::serial_println!(
        "[limine] Fallback memory: phys {:#x}..{:#x}, HHDM {:#x}",
        FALLBACK_PHYS_START,
        FALLBACK_PHYS_START + FALLBACK_PHYS_SIZE,
        LIMINE_HHDM_OFFSET,
    );

    let mut mapper = unsafe { build_offset_page_table(phys_mem_offset) };
    crate::serial_println!("[ok] Page table mapped (fallback frame allocator)");

    // ---- Phase 4: heap ---------------------------------------------------
    let mut fa = FallbackFrameAllocator;
    crate::allocator::init(&mut mapper, &mut fa)
        .expect("heap initialisation failed");
    crate::serial_println!(
        "[ok] Heap ready ({}K)",
        crate::allocator::HEAP_SIZE / 1024,
    );

    // ---- Phase 5: subsystems ---------------------------------------------
    crate::task::init();
    crate::serial_println!("[ok] Task system");

    crate::vfs::init();
    crate::serial_println!("[ok] VFS");

    crate::driver::init();
    crate::serial_println!("[ok] Drivers");

    crate::module::init();
    crate::serial_println!("[ok] Modules");

    crate::ksyms::init();
    crate::slab::init();
    crate::blkdev::init();
    crate::fd::init();
    crate::serial_println!("[ok] Slab + block devices + fd table");

    crate::env::init();
    crate::smp::init();
    crate::apic_timer::init();
    crate::virtio_blk::init();
    crate::virtio_net::init();
    crate::ahci::init();
    crate::nvme::init();
    crate::xhci::init();
    crate::e1000e::init();
    crate::netstack::init();
    crate::usb_hid::init();
    crate::semfs::init();
    crate::security::init();
    crate::capability::init();
    crate::structured_log::init();
    crate::log_rotate::init();
    crate::remote_log::init();
    crate::panic_recover::init();
    crate::http_middleware::init();
    crate::scp::init();
    crate::dns_zone::init();
    crate::mqtt_broker::init();
    crate::ws_server::init();
    crate::nn_inference::init();
    crate::vector_store::init();
    crate::ai_workflow::init();
    crate::self_evolve::init();
    crate::gpu::init();
    crate::bluetooth::init();
    crate::dfs::init();
    crate::rt_sched::init();
    crate::microkernel::init();
    crate::audio_engine::init();
    crate::midi::init();
    crate::userland::init();
    crate::libc::init();
    crate::widget::init();
    crate::dialog::init();
    crate::ipv6::init();
    crate::https_server::init();
    crate::pkg_registry::init();
    crate::build_system::init();
    crate::ext4::init();
    crate::tcp_congestion::init();
    crate::wasi::init();
    crate::veth::init();
    crate::bridge::init();
    crate::elf_runtime::init();
    crate::debuginfo::init();
    crate::crypto_ext::init();
    crate::procfs::init();
    crate::sysfs::init();
    crate::tmpfs::init();
    crate::pipe2::init();
    crate::ai_proxy::init();
    crate::agent::init();
    crate::acl::init();
    crate::power_mgmt::init();
    crate::hda::init();
    crate::wifi::init();
    crate::mmap::init();
    crate::proc_mgr::init();
    crate::elf_exec::init();
    crate::bash::init();
    crate::autocomplete::init();
    crate::installer::init();
    crate::multi_user::init();
    crate::service_mgr::init();
    crate::virtio_gpu_ext::init();
    crate::kconfig::load();
    crate::kconfig_ext::init();
    crate::netdiag::init();
    crate::vmm::init();
    crate::ipc_ext::init();
    crate::perf_events::init();
    crate::cgroup::init();
    crate::vim::init();
    crate::script::create_default_init();
    crate::serial_println!("[ok] All subsystems initialised");

    // ---- Phase 6: user-facing startup ------------------------------------
    let dt = crate::rtc::read();
    crate::serial_println!("[ok] RTC: {}", dt);
    crate::serial_println!("Kernel initialisation complete (Limine path).");

    crate::login::show();
    while crate::login::is_logging_in() {
        x86_64::instructions::hlt();
    }

    crate::println!("Type 'help' for available commands.");
    crate::serial_println!("Shell active.");
    crate::shell::prompt();

    halt_loop();
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build an [`OffsetPageTable`] from the currently active CR3.
///
/// # Safety
/// `phys_mem_offset` must equal the HHDM base that Limine established.
unsafe fn build_offset_page_table(phys_mem_offset: VirtAddr) -> OffsetPageTable<'static> {
    use x86_64::registers::control::Cr3;

    let (frame, _) = Cr3::read();
    let phys = frame.start_address();
    let virt = phys_mem_offset + phys.as_u64();
    let table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };
    unsafe { OffsetPageTable::new(table, phys_mem_offset) }
}

/// Halt the CPU in an infinite loop — final idle state.
fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
