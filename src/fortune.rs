/// fortune — random tips, facts, and quotes about MerlionOS.

use crate::timer;

static FORTUNES: &[&str] = &[
    "The Merlion has a lion's head and a fish's body — just like this OS has a kernel's brain and a shell's personality.",
    "MerlionOS was written entirely by AI in a single conversation. Born for AI, Built by AI.",
    "Fun fact: The VGA text mode buffer is at physical address 0xB8000. That's been the same since the IBM PC in 1981.",
    "Tip: Use 'cat /proc/cpuinfo' to see your (virtual) CPU details.",
    "Did you know? The PIT (Programmable Interval Timer) chip was designed by Intel in 1981. We use it at 100 Hz.",
    "Tip: Try 'cat /proc/tasks | grep running' — yes, pipes work!",
    "The x86_64 architecture has 4 privilege rings, but most OSes only use Ring 0 (kernel) and Ring 3 (user).",
    "MerlionOS has over 100 shell commands. Type 'help' to see them all.",
    "Singapore's Merlion statue was designed by Alec Fraser-Brunner and unveiled in 1972.",
    "Tip: 'edit /tmp/notes.txt' opens a real text editor inside the OS.",
    "The virtio standard was created by Rusty Russell at IBM. It's the standard for VM device emulation.",
    "A context switch saves rbx, rbp, r12-r15, and rsp — that's 7 registers, 56 bytes.",
    "Tip: Run 'demo' for an automated tour of all MerlionOS features.",
    "The ELF format (Executable and Linkable Format) was introduced by Unix System V in 1983.",
    "Tip: 'snake' launches a Snake game. Arrow keys to move, eat * to grow!",
    "In MerlionOS, syscalls use int 0x80 — same as early Linux (before syscall/sysenter).",
    "Tip: 'top' shows a live system monitor. Press 'q' to exit.",
    "The QEMU name stands for 'Quick Emulator'. It was created by Fabrice Bellard in 2003.",
    "Tip: Try 'ai 你好' — the AI shell understands Chinese!",
    "Rust's ownership system prevents data races at compile time. That's why we use it for OS development.",
    "Tip: 'calc (2 + 3) * 4' evaluates arithmetic with proper operator precedence.",
    "The bootloader maps all physical memory at a high virtual address. That's how we access any physical page.",
    "Marina Bay Sands was designed by Moshe Safdie and opened in 2010. It cost US$5.7 billion.",
    "Tip: 'neofetch' shows a system summary with the MerlionOS logo.",
    "A slab allocator pre-divides pages into fixed-size slots. Faster than the general heap for common objects.",
    "Tip: 'chat' enters an interactive AI conversation mode.",
    "The PS/2 keyboard protocol uses scancodes. Set 1 dates back to the IBM PC XT from 1983.",
    "Tip: Use up/down arrow keys to browse command history in the shell.",
];

/// Get a random fortune.
pub fn random() -> &'static str {
    let ticks = timer::ticks() as usize;
    FORTUNES[ticks % FORTUNES.len()]
}
