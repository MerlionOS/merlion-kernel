/* RISC-V kernel linker script — QEMU virt loads at 0x80200000 */
/* OpenSBI occupies 0x80000000–0x801FFFFF, kernel starts after */
ENTRY(_start)
SECTIONS {
    . = 0x80200000;
    .text : {
        KEEP(*(.text._start))
        *(.text .text.*)
    }
    . = ALIGN(4096);
    .rodata : {
        *(.rodata .rodata.*)
    }
    . = ALIGN(4096);
    .data : {
        *(.data .data.*)
    }
    . = ALIGN(4096);
    __bss_start = .;
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }
    __bss_end = .;
    . = ALIGN(4096);
    __heap_start = .;
    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment*)
    }
}
