/// Global Descriptor Table and Task State Segment setup.
/// The GDT defines memory segments; the TSS provides a separate stack
/// for double fault handling and the kernel stack for ring 3 → ring 0 transitions.

use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use spin::Lazy;

/// IST index used for the double fault handler stack.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const STACK_SIZE: usize = 4096 * 5; // 20 KiB

static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();

    // Double fault handler stack (IST entry 0)
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
        let stack_start = VirtAddr::from_ptr(&raw const STACK);
        stack_start + STACK_SIZE as u64
    };

    // Kernel stack for ring 3 → ring 0 transitions (privilege_stack_table[0])
    tss.privilege_stack_table[0] = {
        static mut KERNEL_STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];
        let stack_start = VirtAddr::from_ptr(&raw const KERNEL_STACK);
        stack_start + STACK_SIZE as u64
    };

    tss
});

/// GDT layout:
///   0: null descriptor
///   1: kernel code (0x08)
///   2-3: TSS (occupies two entries: 0x10, 0x18)
///   4: user data (0x20, with RPL=3 → selector 0x23)
///   5: user code (0x28, with RPL=3 → selector 0x2B)
static GDT: Lazy<(GlobalDescriptorTable, Selectors)> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let kernel_code = gdt.add_entry(Descriptor::kernel_code_segment());
    let tss = gdt.add_entry(Descriptor::tss_segment(&TSS));
    let user_data = gdt.add_entry(Descriptor::user_data_segment());
    let user_code = gdt.add_entry(Descriptor::user_code_segment());
    (gdt, Selectors { kernel_code, tss, user_data, user_code })
});

#[allow(dead_code)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub tss: SegmentSelector,
    pub user_data: SegmentSelector,
    pub user_code: SegmentSelector,
}

/// Load the GDT and set CS and TSS segment registers.
pub fn init() {
    use x86_64::instructions::segmentation::{CS, Segment};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.kernel_code);
        load_tss(GDT.1.tss);
    }
}

#[allow(dead_code)]
pub fn selectors() -> &'static Selectors {
    &GDT.1
}
