#![no_std]
#![feature(abi_x86_interrupt)]

extern crate alloc;

pub mod acpi;
pub mod framebuf;
pub mod allocator;
pub mod driver;
pub mod gdt;
pub mod interrupts;
pub mod ipc;
pub mod keyboard;
pub mod log;
pub mod memory;
pub mod pci;
pub mod process;
pub mod ramdisk;
pub mod rtc;
pub mod serial;
pub mod shell;
pub mod syscall;
pub mod task;
pub mod testutil;
pub mod timer;
pub mod vfs;
pub mod vga;
