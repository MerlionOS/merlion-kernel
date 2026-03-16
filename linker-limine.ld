/* Limine linker script for MerlionOS kernel.
 * Limine expects the kernel loaded at the higher half.
 * Request markers must be contiguous: start_marker, requests, end_marker.
 */
ENTRY(_start)

PHDRS {
    text    PT_LOAD FLAGS(5);   /* r-x */
    rodata  PT_LOAD FLAGS(4);   /* r-- */
    data    PT_LOAD FLAGS(6);   /* rw- */
}

SECTIONS {
    . = 0xffffffff80000000;

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(4096);
    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    /* Limine requests section — in data (writable) so Limine can fill response ptrs */
    . = ALIGN(4096);
    .limine_requests : {
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end))
    } :data

    . = ALIGN(4096);
    .data : {
        *(.data .data.*)
    } :data

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment*)
    }
}
