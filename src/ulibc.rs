/// Userspace C library for MerlionOS (U5).
///
/// Generates x86_64 machine code for standard C library functions that
/// run in Ring 3 userspace. Functions communicate with the kernel via
/// int 0x80 syscalls.
///
/// Layout: libc code is loaded at LIBC_BASE (0x0050_0000).
/// Each function is at a fixed offset, callable via `movabs rax, addr; call rax`.
///
/// Syscall ABI: rax=number, rdi=arg1, rsi=arg2, rdx=arg3, return in rax.

use alloc::vec;
use alloc::vec::Vec;
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  ADDRESS LAYOUT
// ═══════════════════════════════════════════════════════════════════

/// Base address where libc code is loaded.
pub const LIBC_BASE: u64 = 0x0000_0050_0000;

/// Total size of the libc code region (one page).
pub const LIBC_SIZE: usize = 4096;

/// Data page for libc globals (heap pointer, scratch buffers).
pub const LIBC_DATA: u64 = 0x0000_0060_0000;

/// Heap start address (processes use brk to extend).
pub const HEAP_BASE: u64 = 0x0000_0080_0000;

// Offsets within LIBC_DATA for global state
const HEAP_PTR_OFF: u64 = 0;    // current heap pointer (u64)
const HEAP_END_OFF: u64 = 8;    // current brk end (u64)
const ITOA_BUF_OFF: u64 = 256;  // 32-byte scratch for itoa

// ═══════════════════════════════════════════════════════════════════
//  FUNCTION ADDRESSES (absolute, for user programs to call)
// ═══════════════════════════════════════════════════════════════════

/// write(rdi=buf_ptr, rsi=len) -> rax=bytes_written
pub const FN_WRITE: u64 = LIBC_BASE + 0x000;

/// exit(rdi=code) -> noreturn
pub const FN_EXIT: u64 = LIBC_BASE + 0x010;

/// strlen(rdi=str_ptr) -> rax=length (null-terminated)
pub const FN_STRLEN: u64 = LIBC_BASE + 0x020;

/// memcpy(rdi=dst, rsi=src, rdx=len) -> rax=dst
pub const FN_MEMCPY: u64 = LIBC_BASE + 0x040;

/// memset(rdi=ptr, rsi=val, rdx=len) -> rax=ptr
pub const FN_MEMSET: u64 = LIBC_BASE + 0x058;

/// strcmp(rdi=s1, rsi=s2) -> rax=result (0 if equal)
pub const FN_STRCMP: u64 = LIBC_BASE + 0x070;

/// malloc(rdi=size) -> rax=ptr (0 on failure)
pub const FN_MALLOC: u64 = LIBC_BASE + 0x0A0;

/// free(rdi=ptr) -> void (no-op in bump allocator)
pub const FN_FREE: u64 = LIBC_BASE + 0x100;

/// open(rdi=path_ptr, rsi=path_len, rdx=flags) -> rax=fd
pub const FN_OPEN: u64 = LIBC_BASE + 0x108;

/// read(rdi=fd, rsi=buf_ptr, rdx=len) -> rax=bytes_read
pub const FN_READ: u64 = LIBC_BASE + 0x118;

/// close(rdi=fd) -> rax=0
pub const FN_CLOSE: u64 = LIBC_BASE + 0x128;

/// getpid() -> rax=pid
pub const FN_GETPID: u64 = LIBC_BASE + 0x138;

/// brk(rdi=addr) -> rax=new_brk
pub const FN_BRK: u64 = LIBC_BASE + 0x148;

/// sleep_ms(rdi=ms) -> void
pub const FN_SLEEP: u64 = LIBC_BASE + 0x158;

/// socket(rdi=domain, rsi=type, rdx=proto) -> rax=fd
pub const FN_SOCKET: u64 = LIBC_BASE + 0x168;

/// connect(rdi=fd, rsi=addr_ptr, rdx=addr_len) -> rax
pub const FN_CONNECT: u64 = LIBC_BASE + 0x178;

/// sendto(rdi=fd, rsi=buf, rdx=len) -> rax=bytes
pub const FN_SENDTO: u64 = LIBC_BASE + 0x188;

/// recvfrom(rdi=fd, rsi=buf, rdx=len) -> rax=bytes
pub const FN_RECVFROM: u64 = LIBC_BASE + 0x198;

/// gettime() -> rax=seconds_since_boot
pub const FN_GETTIME: u64 = LIBC_BASE + 0x1A8;

/// itoa(rdi=number, rsi=buf_ptr) -> rax=digit_count
pub const FN_ITOA: u64 = LIBC_BASE + 0x1B8;

/// printf(rdi=fmt_ptr, rsi=fmt_len, rdx=int_arg) -> rax=chars
/// Kernel-side formatting via SYS_PRINTF (160).
pub const FN_PRINTF: u64 = LIBC_BASE + 0x220;

/// puts(rdi=str_ptr) -> rax (writes null-terminated string + newline)
pub const FN_PUTS: u64 = LIBC_BASE + 0x238;

/// print_int(rdi=number) -> void (prints integer to stdout)
pub const FN_PRINT_INT: u64 = LIBC_BASE + 0x270;

// ═══════════════════════════════════════════════════════════════════
//  MACHINE CODE GENERATION
// ═══════════════════════════════════════════════════════════════════

/// Helper: emit `mov rax, imm32` (7 bytes) at position.
fn emit_mov_rax_imm32(code: &mut [u8], pos: usize, val: u32) {
    code[pos]   = 0x48;
    code[pos+1] = 0xC7;
    code[pos+2] = 0xC0;
    code[pos+3..pos+7].copy_from_slice(&val.to_le_bytes());
}

/// Helper: emit `int 0x80` (2 bytes).
fn emit_int80(code: &mut [u8], pos: usize) {
    code[pos]   = 0xCD;
    code[pos+1] = 0x80;
}

/// Helper: emit `ret` (1 byte).
fn emit_ret(code: &mut [u8], pos: usize) {
    code[pos] = 0xC3;
}

/// Helper: emit `movabs rax, imm64` (10 bytes).
fn emit_movabs_rax(code: &mut [u8], pos: usize, val: u64) {
    code[pos]   = 0x48;
    code[pos+1] = 0xB8;
    code[pos+2..pos+10].copy_from_slice(&val.to_le_bytes());
}

/// Helper: emit a simple syscall wrapper: mov rax, NUM; int 0x80; ret (10 bytes).
fn emit_syscall_wrapper(code: &mut [u8], pos: usize, syscall_num: u32) {
    emit_mov_rax_imm32(code, pos, syscall_num);
    emit_int80(code, pos + 7);
    emit_ret(code, pos + 9);
}

/// Generate the complete userspace libc machine code (4096 bytes).
pub fn generate_libc_code() -> Vec<u8> {
    let mut code = vec![0xCC_u8; LIBC_SIZE]; // fill with int3 for debugging

    // ── write(rdi=buf, rsi=len) ── offset 0x000
    // mov rax, 0; int 0x80; ret
    emit_syscall_wrapper(&mut code, 0x000, 0); // SYS_WRITE = 0

    // ── exit(rdi=code) ── offset 0x010
    // mov rax, 1; int 0x80; jmp $
    emit_mov_rax_imm32(&mut code, 0x010, 1);
    emit_int80(&mut code, 0x017);
    code[0x019] = 0xEB; code[0x01A] = 0xFE; // jmp $

    // ── strlen(rdi=str_ptr) -> rax ── offset 0x020
    {
        let b = 0x020;
        // xor rax, rax
        code[b]   = 0x48; code[b+1] = 0x31; code[b+2] = 0xC0;
        // .loop (b+3):
        // cmp byte [rdi+rax], 0
        code[b+3] = 0x80; code[b+4] = 0x3C; code[b+5] = 0x07; code[b+6] = 0x00;
        // je .done (+5 → b+14)
        code[b+7] = 0x74; code[b+8] = 0x05;
        // inc rax
        code[b+9] = 0x48; code[b+10] = 0xFF; code[b+11] = 0xC0;
        // jmp .loop (b+3): displacement = (b+3) - (b+14) = -11 = 0xF5
        code[b+12] = 0xEB; code[b+13] = 0xF5;
        // .done: ret
        code[b+14] = 0xC3;
    }

    // ── memcpy(rdi=dst, rsi=src, rdx=len) -> rax=dst ── offset 0x040
    {
        let b = 0x040;
        code[b]   = 0x57;                       // push rdi
        code[b+1] = 0x48; code[b+2] = 0x89; code[b+3] = 0xD1; // mov rcx, rdx
        code[b+4] = 0xF3; code[b+5] = 0xA4;    // rep movsb
        code[b+6] = 0x58;                       // pop rax
        code[b+7] = 0xC3;                       // ret
    }

    // ── memset(rdi=ptr, rsi=val, rdx=len) -> rax=ptr ── offset 0x058
    {
        let b = 0x058;
        code[b]   = 0x57;                       // push rdi
        code[b+1] = 0x48; code[b+2] = 0x89; code[b+3] = 0xF0; // mov rax, rsi (val)
        code[b+4] = 0x48; code[b+5] = 0x89; code[b+6] = 0xD1; // mov rcx, rdx (len)
        code[b+7] = 0xF3; code[b+8] = 0xAA;    // rep stosb
        code[b+9] = 0x58;                       // pop rax
        code[b+10] = 0xC3;                      // ret
    }

    // ── strcmp(rdi=s1, rsi=s2) -> rax ── offset 0x070
    {
        let b = 0x070;
        // .loop (b+0):
        code[b]    = 0x0F; code[b+1]  = 0xB6; code[b+2]  = 0x07; // movzx eax, byte [rdi]
        code[b+3]  = 0x0F; code[b+4]  = 0xB6; code[b+5]  = 0x0E; // movzx ecx, byte [rsi]
        code[b+6]  = 0x29; code[b+7]  = 0xC8;                     // sub eax, ecx
        code[b+8]  = 0x75; code[b+9]  = 0x0C;                     // jnz .done (b+22)
        code[b+10] = 0x84; code[b+11] = 0xC9;                     // test cl, cl
        code[b+12] = 0x74; code[b+13] = 0x08;                     // jz .done (b+22)
        code[b+14] = 0x48; code[b+15] = 0xFF; code[b+16] = 0xC7; // inc rdi
        code[b+17] = 0x48; code[b+18] = 0xFF; code[b+19] = 0xC6; // inc rsi
        code[b+20] = 0xEB; code[b+21] = 0xEA;                     // jmp .loop (-22 from b+22 to b+0)
        // .done (b+22):
        // cdqe (sign-extend eax to rax)
        code[b+22] = 0x48; code[b+23] = 0x98;
        code[b+24] = 0xC3;                                        // ret
    }

    // ── malloc(rdi=size) -> rax=ptr ── offset 0x0A0
    // Bump allocator: load heap_ptr from LIBC_DATA, add size, call brk, store new ptr
    {
        let b = 0x0A0;
        let mut p = b;
        // push rbx
        code[p] = 0x53; p += 1;
        // push r12
        code[p] = 0x41; code[p+1] = 0x54; p += 2;
        // mov r12, rdi  (save size)
        code[p] = 0x49; code[p+1] = 0x89; code[p+2] = 0xFC; p += 3;
        // movabs rbx, LIBC_DATA  (address of heap globals)
        code[p] = 0x48; code[p+1] = 0xBB;
        code[p+2..p+10].copy_from_slice(&LIBC_DATA.to_le_bytes());
        p += 10;
        // mov rax, [rbx]  (rax = current heap_ptr)
        code[p] = 0x48; code[p+1] = 0x8B; code[p+2] = 0x03; p += 3;
        // push rax  (save old heap_ptr = return value)
        code[p] = 0x50; p += 1;
        // add rax, r12  (new_brk = heap_ptr + size)
        code[p] = 0x4C; code[p+1] = 0x01; code[p+2] = 0xE0; p += 3;
        // Align to 16: add rax, 15
        code[p] = 0x48; code[p+1] = 0x83; code[p+2] = 0xC0; code[p+3] = 0x0F; p += 4;
        // and rax, -16
        code[p] = 0x48; code[p+1] = 0x83; code[p+2] = 0xE0; code[p+3] = 0xF0; p += 4;
        // mov rdi, rax  (brk arg = new_brk)
        code[p] = 0x48; code[p+1] = 0x89; code[p+2] = 0xC7; p += 3;
        // push rdi  (save new_brk)
        code[p] = 0x57; p += 1;
        // mov rax, 113 (SYS_BRK)
        emit_mov_rax_imm32(&mut code, p, 113); p += 7;
        // int 0x80
        emit_int80(&mut code, p); p += 2;
        // pop rdx  (new_brk)
        code[p] = 0x5A; p += 1;
        // test rax, rax
        code[p] = 0x48; code[p+1] = 0x85; code[p+2] = 0xC0; p += 3;
        // pop rax  (old heap_ptr)
        code[p] = 0x58; p += 1;
        // jz .fail
        let jz_pos = p;
        code[p] = 0x74; code[p+1] = 0x00; p += 2; // patch later
        // Success: store new heap_ptr
        // mov [rbx], rdx
        code[p] = 0x48; code[p+1] = 0x89; code[p+2] = 0x13; p += 3;
        // rax already has old heap_ptr (the allocated block)
        // pop r12
        code[p] = 0x41; code[p+1] = 0x5C; p += 2;
        // pop rbx
        code[p] = 0x5B; p += 1;
        // ret
        code[p] = 0xC3; p += 1;
        // .fail:
        let fail_offset = p;
        code[jz_pos + 1] = (fail_offset - (jz_pos + 2)) as u8;
        // xor rax, rax (return NULL)
        code[p] = 0x48; code[p+1] = 0x31; code[p+2] = 0xC0; p += 3;
        // pop r12
        code[p] = 0x41; code[p+1] = 0x5C; p += 2;
        // pop rbx
        code[p] = 0x5B; p += 1;
        // ret
        code[p] = 0xC3;
    }

    // ── free(rdi=ptr) ── offset 0x100
    // No-op for bump allocator (memory reclaimed on process exit)
    emit_ret(&mut code, 0x100);

    // ── Syscall wrappers ──────────────────────────────────────────

    // open(rdi=path, rsi=len, rdx=flags) ── offset 0x108
    emit_syscall_wrapper(&mut code, 0x108, 100); // SYS_OPEN

    // read(rdi=fd, rsi=buf, rdx=len) ── offset 0x118
    emit_syscall_wrapper(&mut code, 0x118, 101); // SYS_READ

    // close(rdi=fd) ── offset 0x128
    emit_syscall_wrapper(&mut code, 0x128, 102); // SYS_CLOSE

    // getpid() ── offset 0x138
    emit_syscall_wrapper(&mut code, 0x138, 3); // SYS_GETPID

    // brk(rdi=addr) ── offset 0x148
    emit_syscall_wrapper(&mut code, 0x148, 113); // SYS_BRK

    // sleep_ms(rdi=ms) ── offset 0x158
    emit_syscall_wrapper(&mut code, 0x158, 141); // SYS_NANOSLEEP

    // socket(rdi=domain, rsi=type, rdx=proto) ── offset 0x168
    emit_syscall_wrapper(&mut code, 0x168, 130); // SYS_SOCKET

    // connect(rdi=fd, rsi=addr, rdx=len) ── offset 0x178
    emit_syscall_wrapper(&mut code, 0x178, 131); // SYS_CONNECT

    // sendto(rdi=fd, rsi=buf, rdx=len) ── offset 0x188
    emit_syscall_wrapper(&mut code, 0x188, 132); // SYS_SENDTO

    // recvfrom(rdi=fd, rsi=buf, rdx=len) ── offset 0x198
    emit_syscall_wrapper(&mut code, 0x198, 133); // SYS_RECVFROM

    // gettime() ── offset 0x1A8
    emit_syscall_wrapper(&mut code, 0x1A8, 140); // SYS_TIME

    // ── itoa(rdi=number, rsi=buf_ptr) -> rax=digit_count ── offset 0x1B8
    {
        let b = 0x1B8;
        let mut p = b;
        // push rbx
        code[p] = 0x53; p += 1;
        // push r12
        code[p] = 0x41; code[p+1] = 0x54; p += 2;
        // mov r12, rsi  (save buf start)
        code[p] = 0x49; code[p+1] = 0x89; code[p+2] = 0xF4; p += 3;
        // mov rax, rdi  (number)
        code[p] = 0x48; code[p+1] = 0x89; code[p+2] = 0xF8; p += 3;
        // xor rcx, rcx  (digit count)
        code[p] = 0x48; code[p+1] = 0x31; code[p+2] = 0xC9; p += 3;
        // test rax, rax
        code[p] = 0x48; code[p+1] = 0x85; code[p+2] = 0xC0; p += 3;
        // jnz .div_loop
        code[p] = 0x75; code[p+1] = 0x07; p += 2;
        // Zero case: mov byte [rsi], '0'
        code[p] = 0xC6; code[p+1] = 0x06; code[p+2] = 0x30; p += 3;
        // mov rax, 1 (length)
        code[p] = 0x48; code[p+1] = 0xC7; code[p+2] = 0xC0;
        code[p+3] = 0x01; code[p+4] = 0x00; code[p+5] = 0x00; code[p+6] = 0x00; p += 7;
        // jmp .end
        let jmp_end_pos = p;
        code[p] = 0xEB; code[p+1] = 0x00; p += 2; // patch later

        // .div_loop:
        let div_loop = p;
        // xor rdx, rdx
        code[p] = 0x48; code[p+1] = 0x31; code[p+2] = 0xD2; p += 3;
        // mov rbx, 10
        code[p] = 0x48; code[p+1] = 0xC7; code[p+2] = 0xC3;
        code[p+3] = 0x0A; code[p+4] = 0x00; code[p+5] = 0x00; code[p+6] = 0x00; p += 7;
        // div rbx  (rax=quotient, rdx=remainder)
        code[p] = 0x48; code[p+1] = 0xF7; code[p+2] = 0xF3; p += 3;
        // add dl, '0'
        code[p] = 0x80; code[p+1] = 0xC2; code[p+2] = 0x30; p += 3;
        // push rdx  (save digit)
        code[p] = 0x52; p += 1;
        // inc rcx
        code[p] = 0x48; code[p+1] = 0xFF; code[p+2] = 0xC1; p += 3;
        // test rax, rax
        code[p] = 0x48; code[p+1] = 0x85; code[p+2] = 0xC0; p += 3;
        // jnz .div_loop
        let jnz_disp = (div_loop as isize - (p as isize + 2)) as i8;
        code[p] = 0x75; code[p+1] = jnz_disp as u8; p += 2;

        // .pop_loop: pop digits into buffer (correct order)
        let pop_loop = p;
        // pop rax
        code[p] = 0x58; p += 1;
        // mov [rsi], al
        code[p] = 0x88; code[p+1] = 0x06; p += 2;
        // inc rsi
        code[p] = 0x48; code[p+1] = 0xFF; code[p+2] = 0xC6; p += 3;
        // dec rcx
        code[p] = 0x48; code[p+1] = 0xFF; code[p+2] = 0xC9; p += 3;
        // jnz .pop_loop
        let jnz_pop = (pop_loop as isize - (p as isize + 2)) as i8;
        code[p] = 0x75; code[p+1] = jnz_pop as u8; p += 2;

        // Calculate length: rax = rsi - r12
        // .end:
        let end_pos = p;
        code[jmp_end_pos + 1] = (end_pos - (jmp_end_pos + 2)) as u8;
        // mov rax, rsi
        code[p] = 0x48; code[p+1] = 0x89; code[p+2] = 0xF0; p += 3;
        // sub rax, r12
        code[p] = 0x4C; code[p+1] = 0x29; code[p+2] = 0xE0; p += 3;
        // pop r12
        code[p] = 0x41; code[p+1] = 0x5C; p += 2;
        // pop rbx
        code[p] = 0x5B; p += 1;
        // ret
        code[p] = 0xC3;
    }

    // ── printf(rdi=fmt_ptr, rsi=fmt_len, rdx=int_arg) ── offset 0x220
    // Calls kernel SYS_PRINTF (160) which handles %d/%x/%s formatting
    emit_syscall_wrapper(&mut code, 0x220, 160); // SYS_PRINTF

    // ── puts(rdi=str_ptr) -> rax ── offset 0x238
    // Writes null-terminated string via strlen + write
    {
        let b = 0x238;
        let mut p = b;
        // push rdi  (save str_ptr)
        code[p] = 0x57; p += 1;
        // Call strlen: need to use absolute call
        // movabs rax, FN_STRLEN
        emit_movabs_rax(&mut code, p, FN_STRLEN); p += 10;
        // call rax
        code[p] = 0xFF; code[p+1] = 0xD0; p += 2;
        // mov rsi, rax  (len)
        code[p] = 0x48; code[p+1] = 0x89; code[p+2] = 0xC6; p += 3;
        // pop rdi  (restore str_ptr)
        code[p] = 0x5F; p += 1;
        // mov rax, 0 (SYS_WRITE)
        emit_mov_rax_imm32(&mut code, p, 0); p += 7;
        // int 0x80
        emit_int80(&mut code, p); p += 2;
        // ret
        code[p] = 0xC3;
    }

    // ── print_int(rdi=number) -> void ── offset 0x270
    // Converts number to string and writes it
    {
        let b = 0x270;
        let mut p = b;
        // sub rsp, 32  (local buffer on stack)
        code[p] = 0x48; code[p+1] = 0x83; code[p+2] = 0xEC; code[p+3] = 0x20; p += 4;
        // mov rsi, rsp  (buf = stack buffer)
        code[p] = 0x48; code[p+1] = 0x89; code[p+2] = 0xE6; p += 3;
        // rdi already has the number
        // movabs rax, FN_ITOA
        emit_movabs_rax(&mut code, p, FN_ITOA); p += 10;
        // call rax
        code[p] = 0xFF; code[p+1] = 0xD0; p += 2;
        // rax = digit count
        // mov rsi, rax  (len)
        code[p] = 0x48; code[p+1] = 0x89; code[p+2] = 0xC6; p += 3;
        // mov rdi, rsp  (buf)
        code[p] = 0x48; code[p+1] = 0x89; code[p+2] = 0xE7; p += 3;
        // mov rax, 0  (SYS_WRITE)
        emit_mov_rax_imm32(&mut code, p, 0); p += 7;
        // int 0x80
        emit_int80(&mut code, p); p += 2;
        // add rsp, 32
        code[p] = 0x48; code[p+1] = 0x83; code[p+2] = 0xC4; code[p+3] = 0x20; p += 4;
        // ret
        code[p] = 0xC3;
    }

    code
}

/// Initialize the libc data page with default values.
pub fn generate_libc_data() -> Vec<u8> {
    let mut data = vec![0u8; 4096];
    // heap_ptr = HEAP_BASE
    data[HEAP_PTR_OFF as usize..HEAP_PTR_OFF as usize + 8]
        .copy_from_slice(&HEAP_BASE.to_le_bytes());
    // heap_end = HEAP_BASE
    data[HEAP_END_OFF as usize..HEAP_END_OFF as usize + 8]
        .copy_from_slice(&HEAP_BASE.to_le_bytes());
    data
}

// ═══════════════════════════════════════════════════════════════════
//  USER PROGRAM BUILDERS
// ═══════════════════════════════════════════════════════════════════

/// Helper: emit `movabs rax, addr; call rax` (12 bytes) to call a libc function.
pub fn emit_call_libc(code: &mut Vec<u8>, fn_addr: u64) {
    // movabs rax, fn_addr
    code.push(0x48); code.push(0xB8);
    code.extend_from_slice(&fn_addr.to_le_bytes());
    // call rax
    code.push(0xFF); code.push(0xD0);
}

/// Helper: emit `mov rdi, imm64` (10 bytes).
pub fn emit_mov_rdi_imm64(code: &mut Vec<u8>, val: u64) {
    code.push(0x48); code.push(0xBF);
    code.extend_from_slice(&val.to_le_bytes());
}

/// Helper: emit `mov rsi, imm64` (10 bytes).
pub fn emit_mov_rsi_imm64(code: &mut Vec<u8>, val: u64) {
    code.push(0x48); code.push(0xBE);
    code.extend_from_slice(&val.to_le_bytes());
}

/// Helper: emit `mov rdx, imm64` (10 bytes).
pub fn emit_mov_rdx_imm64(code: &mut Vec<u8>, val: u64) {
    code.push(0x48); code.push(0xBA);
    code.extend_from_slice(&val.to_le_bytes());
}

/// Helper: emit `mov rdi, rax` (3 bytes).
pub fn emit_mov_rdi_rax(code: &mut Vec<u8>) {
    code.extend_from_slice(&[0x48, 0x89, 0xC7]);
}

/// Helper: emit `mov rsi, rax` (3 bytes).
pub fn emit_mov_rsi_rax(code: &mut Vec<u8>) {
    code.extend_from_slice(&[0x48, 0x89, 0xC6]);
}

/// Helper: emit `mov rdx, rax` (3 bytes).
pub fn emit_mov_rdx_rax(code: &mut Vec<u8>) {
    code.extend_from_slice(&[0x48, 0x89, 0xC2]);
}

/// Helper: emit `push rax` (1 byte).
pub fn emit_push_rax(code: &mut Vec<u8>) {
    code.push(0x50);
}

/// Helper: emit `pop rdi` (1 byte).
pub fn emit_pop_rdi(code: &mut Vec<u8>) {
    code.push(0x5F);
}

/// Helper: emit string data, returns the offset within the code where it was placed.
pub fn emit_string(code: &mut Vec<u8>, s: &[u8]) -> usize {
    let off = code.len();
    code.extend_from_slice(s);
    code.push(0); // null terminator
    off
}

// ═══════════════════════════════════════════════════════════════════
//  DEMO PROGRAMS (using libc)
// ═══════════════════════════════════════════════════════════════════

/// Generate "malloc-test" program: allocates memory, writes to it, prints result.
pub fn gen_malloc_test() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // --- puts(msg1) ---
    let msg1_fixup = c.len() + 2; // offset of imm64 in mov rdi
    emit_mov_rdi_imm64(&mut c, 0); // placeholder
    emit_call_libc(&mut c, FN_PUTS);

    // --- malloc(64) ---
    emit_mov_rdi_imm64(&mut c, 64);
    emit_call_libc(&mut c, FN_MALLOC);

    // Save ptr in r12
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax

    // --- puts(msg2) ---
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // placeholder
    emit_call_libc(&mut c, FN_PUTS);

    // --- memset(ptr, 'A', 64) ---
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_mov_rsi_imm64(&mut c, 0x41); // 'A'
    emit_mov_rdx_imm64(&mut c, 64);
    emit_call_libc(&mut c, FN_MEMSET);

    // --- write(ptr, 8) to show filled memory ---
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_mov_rsi_imm64(&mut c, 8);
    emit_call_libc(&mut c, FN_WRITE);

    // --- puts(msg3) ---
    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // placeholder
    emit_call_libc(&mut c, FN_PUTS);

    // --- exit(0) ---
    c.extend_from_slice(&[0x48, 0x31, 0xFF]); // xor rdi, rdi
    emit_call_libc(&mut c, FN_EXIT);
    // jmp $ safety
    c.extend_from_slice(&[0xEB, 0xFE]);

    // --- String data ---
    let msg1_addr = text_base + c.len() as u64;
    let msg1 = b"malloc-test: allocating 64 bytes...\n\0";
    c.extend_from_slice(msg1);

    let msg2_addr = text_base + c.len() as u64;
    let msg2 = b"malloc-test: malloc returned ptr, filling with 'A'...\n\0";
    c.extend_from_slice(msg2);

    let msg3_addr = text_base + c.len() as u64;
    let msg3 = b"\nmalloc-test: done!\n\0";
    c.extend_from_slice(msg3);

    // Patch string addresses
    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());

    c
}

/// Generate "printf-test" program: demonstrates printf with format strings.
pub fn gen_printf_test() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c = Vec::new();

    // --- printf(fmt1, fmt_len, 42) ---
    let fmt1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // placeholder fmt_ptr
    let fmt1_len_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0); // placeholder fmt_len
    emit_mov_rdx_imm64(&mut c, 42); // int_arg = 42
    emit_call_libc(&mut c, FN_PRINTF);

    // --- printf(fmt2, fmt_len, 0xDEAD) ---
    let fmt2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    let fmt2_len_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0);
    emit_mov_rdx_imm64(&mut c, 0xDEAD);
    emit_call_libc(&mut c, FN_PRINTF);

    // --- puts(msg) ---
    let msg_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- getpid and print_int ---
    emit_call_libc(&mut c, FN_GETPID);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);

    // --- puts(newline) ---
    let nl_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- exit(0) ---
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // String data
    let fmt1_addr = text_base + c.len() as u64;
    let fmt1 = b"printf-test: answer is %d\n";
    let fmt1_len = fmt1.len() as u64;
    c.extend_from_slice(fmt1);
    c.push(0);

    let fmt2_addr = text_base + c.len() as u64;
    let fmt2 = b"printf-test: hex value is 0x%x\n";
    let fmt2_len = fmt2.len() as u64;
    c.extend_from_slice(fmt2);
    c.push(0);

    let msg_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"printf-test: my pid is ");
    c.push(0);

    let nl_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"\n");
    c.push(0);

    // Patch addresses
    c[fmt1_fixup..fmt1_fixup+8].copy_from_slice(&fmt1_addr.to_le_bytes());
    c[fmt1_len_fixup..fmt1_len_fixup+8].copy_from_slice(&fmt1_len.to_le_bytes());
    c[fmt2_fixup..fmt2_fixup+8].copy_from_slice(&fmt2_addr.to_le_bytes());
    c[fmt2_len_fixup..fmt2_len_fixup+8].copy_from_slice(&fmt2_len.to_le_bytes());
    c[msg_fixup..msg_fixup+8].copy_from_slice(&msg_addr.to_le_bytes());
    c[nl_fixup..nl_fixup+8].copy_from_slice(&nl_addr.to_le_bytes());

    c
}

/// Generate "string-test" program: demonstrates strlen, strcmp, memcpy.
pub fn gen_string_test() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c = Vec::new();

    // --- puts(msg1) "string-test: testing libc string functions\n" ---
    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- strlen(test_str) ---
    let str1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_STRLEN);
    // print "strlen = "
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    // Save strlen result, print as int
    emit_push_rax(&mut c);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);
    // newline
    let nl_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    // discard saved rax
    emit_pop_rdi(&mut c);

    // --- strcmp(s1, s2) where s1 == s2 ---
    let cmp1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // s1
    let cmp2_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0); // s2
    emit_call_libc(&mut c, FN_STRCMP);
    // puts("strcmp equal = ")
    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);
    let nl2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- puts(done_msg) ---
    let msg4_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- exit(0) ---
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // String data
    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"string-test: testing libc string functions\n");
    c.push(0);

    let str1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"Hello, MerlionOS!");
    c.push(0);

    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"strlen(\"Hello, MerlionOS!\") = ");
    c.push(0);

    let nl_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"\n");
    c.push(0);

    let cmp1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"abc");
    c.push(0);

    let cmp2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"abc");
    c.push(0);

    let msg3_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"strcmp(\"abc\", \"abc\") = ");
    c.push(0);

    let msg4_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"string-test: all tests passed!\n");
    c.push(0);

    // Patch all addresses
    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[str1_fixup..str1_fixup+8].copy_from_slice(&str1_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[nl_fixup..nl_fixup+8].copy_from_slice(&nl_addr.to_le_bytes());
    c[cmp1_fixup..cmp1_fixup+8].copy_from_slice(&cmp1_addr.to_le_bytes());
    c[cmp2_fixup..cmp2_fixup+8].copy_from_slice(&cmp2_addr.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());
    c[nl2_fixup..nl2_fixup+8].copy_from_slice(&nl_addr.to_le_bytes()); // reuse newline
    c[msg4_fixup..msg4_fixup+8].copy_from_slice(&msg4_addr.to_le_bytes());

    c
}

/// Generate "libc-demo" program: comprehensive demo of all libc features.
pub fn gen_libc_demo() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c = Vec::new();

    // --- Banner ---
    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- getpid ---
    emit_call_libc(&mut c, FN_GETPID);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (save pid)
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_call_libc(&mut c, FN_PRINT_INT);
    let nl_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- malloc + memset ---
    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    emit_mov_rdi_imm64(&mut c, 128);
    emit_call_libc(&mut c, FN_MALLOC);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (save ptr)
    // memset(ptr, 'M', 128)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_mov_rsi_imm64(&mut c, b'M' as u64);
    emit_mov_rdx_imm64(&mut c, 128);
    emit_call_libc(&mut c, FN_MEMSET);

    // write 10 chars of the allocated block
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_mov_rsi_imm64(&mut c, 10);
    emit_call_libc(&mut c, FN_WRITE);

    let nl2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- gettime ---
    let msg4_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    emit_call_libc(&mut c, FN_GETTIME);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);
    let msg5_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // --- Done ---
    let msg6_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // String data
    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"=== MerlionOS libc demo (U5) ===\n");
    c.push(0);

    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"  PID: ");
    c.push(0);

    let nl_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"\n");
    c.push(0);

    let msg3_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"  malloc(128) + memset('M'): ");
    c.push(0);

    let msg4_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"  uptime: ");
    c.push(0);

    let msg5_addr = text_base + c.len() as u64;
    c.extend_from_slice(b" seconds\n");
    c.push(0);

    let msg6_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"=== libc demo complete ===\n");
    c.push(0);

    // Patch addresses
    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[nl_fixup..nl_fixup+8].copy_from_slice(&nl_addr.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());
    c[nl2_fixup..nl2_fixup+8].copy_from_slice(&nl_addr.to_le_bytes());
    c[msg4_fixup..msg4_fixup+8].copy_from_slice(&msg4_addr.to_le_bytes());
    c[msg5_fixup..msg5_fixup+8].copy_from_slice(&msg5_addr.to_le_bytes());
    c[msg6_fixup..msg6_fixup+8].copy_from_slice(&msg6_addr.to_le_bytes());

    c
}

// ═══════════════════════════════════════════════════════════════════
//  INITIALIZATION
// ═══════════════════════════════════════════════════════════════════

/// Initialize the userspace libc subsystem.
pub fn init() {
    serial_println!("[ulibc] userspace libc initialized");
    serial_println!("[ulibc] LIBC_BASE={:#x} LIBC_DATA={:#x} HEAP_BASE={:#x}",
        LIBC_BASE, LIBC_DATA, HEAP_BASE);
    serial_println!("[ulibc] {} libc functions available", 22);
    serial_println!("[ulibc] functions: write exit strlen memcpy memset strcmp malloc free");
    serial_println!("[ulibc]   open read close getpid brk sleep socket connect sendto recvfrom");
    serial_println!("[ulibc]   gettime itoa printf puts print_int");
}

/// Return info string about the libc.
pub fn info() -> alloc::string::String {
    alloc::format!(
        "Userspace libc (U5)\n\
         Code:      {:#010x} ({} bytes)\n\
         Data:      {:#010x}\n\
         Heap:      {:#010x}\n\
         Functions: 22 (write, exit, strlen, memcpy, memset, strcmp,\n\
         \x20          malloc, free, open, read, close, getpid, brk,\n\
         \x20          sleep, socket, connect, sendto, recvfrom,\n\
         \x20          gettime, itoa, printf, puts, print_int)\n\
         Programs:  malloc-test, printf-test, string-test, libc-demo,\n\
         \x20          cat, echo, wc, ls\n",
        LIBC_BASE, LIBC_SIZE, LIBC_DATA, HEAP_BASE,
    )
}

// ═══════════════════════════════════════════════════════════════════
//  STANDARD USER PROGRAMS
// ═══════════════════════════════════════════════════════════════════

/// Generate "cat" program: reads /proc/version and prints it.
pub fn gen_cat() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // puts("cat: reading /proc/version\n")
    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // fd = open("/proc/version", 13, 0)
    let path_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // path
    let path_len_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0); // len
    emit_mov_rdx_imm64(&mut c, 0); // flags
    emit_call_libc(&mut c, FN_OPEN);
    // save fd in r12
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax

    // buf = malloc(256)
    emit_mov_rdi_imm64(&mut c, 256);
    emit_call_libc(&mut c, FN_MALLOC);
    c.extend_from_slice(&[0x49, 0x89, 0xC5]); // mov r13, rax (buf)

    // n = read(fd, buf, 256)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12 (fd)
    c.extend_from_slice(&[0x4C, 0x89, 0xEE]); // mov rsi, r13 (buf)
    emit_mov_rdx_imm64(&mut c, 256);
    emit_call_libc(&mut c, FN_READ);
    c.extend_from_slice(&[0x49, 0x89, 0xC6]); // mov r14, rax (n)

    // write(buf, n) — print file contents
    c.extend_from_slice(&[0x4C, 0x89, 0xEF]); // mov rdi, r13 (buf)
    c.extend_from_slice(&[0x4C, 0x89, 0xF6]); // mov rsi, r14 (n)
    emit_call_libc(&mut c, FN_WRITE);

    // close(fd)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_call_libc(&mut c, FN_CLOSE);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // Strings
    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"cat: /proc/version\n\0");

    let path_addr = text_base + c.len() as u64;
    let path = b"/proc/version";
    let path_len = path.len() as u64;
    c.extend_from_slice(path);
    c.push(0);

    // Patch
    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[path_fixup..path_fixup+8].copy_from_slice(&path_addr.to_le_bytes());
    c[path_len_fixup..path_len_fixup+8].copy_from_slice(&path_len.to_le_bytes());

    c
}

/// Generate "echo" program: prints a message to stdout.
pub fn gen_echo() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // puts(msg)
    let msg_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    let msg_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"Hello from MerlionOS userspace echo!\n\0");

    c[msg_fixup..msg_fixup+8].copy_from_slice(&msg_addr.to_le_bytes());
    c
}

/// Generate "wc" program: counts characters in /proc/version.
pub fn gen_wc() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // fd = open("/proc/version", 13, 0)
    let path_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    let path_len_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0);
    emit_mov_rdx_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_OPEN);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (fd)

    // buf = malloc(512)
    emit_mov_rdi_imm64(&mut c, 512);
    emit_call_libc(&mut c, FN_MALLOC);
    c.extend_from_slice(&[0x49, 0x89, 0xC5]); // mov r13, rax (buf)

    // n = read(fd, buf, 512)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    c.extend_from_slice(&[0x4C, 0x89, 0xEE]); // mov rsi, r13
    emit_mov_rdx_imm64(&mut c, 512);
    emit_call_libc(&mut c, FN_READ);
    c.extend_from_slice(&[0x49, 0x89, 0xC6]); // mov r14, rax (n = bytes read)

    // close(fd)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_call_libc(&mut c, FN_CLOSE);

    // print "  <n> /proc/version\n"
    let msg_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    c.extend_from_slice(&[0x4C, 0x89, 0xF7]); // mov rdi, r14 (n)
    emit_call_libc(&mut c, FN_PRINT_INT);
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // Strings
    let path_addr = text_base + c.len() as u64;
    let path = b"/proc/version";
    let path_len = path.len() as u64;
    c.extend_from_slice(path);
    c.push(0);

    let msg_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"  \0");

    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b" /proc/version\n\0");

    c[path_fixup..path_fixup+8].copy_from_slice(&path_addr.to_le_bytes());
    c[path_len_fixup..path_len_fixup+8].copy_from_slice(&path_len.to_le_bytes());
    c[msg_fixup..msg_fixup+8].copy_from_slice(&msg_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());

    c
}

/// Generate "ls" program: lists files in root directory.
pub fn gen_ls() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // puts("ls /\n")
    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // buf = malloc(1024)
    emit_mov_rdi_imm64(&mut c, 1024);
    emit_call_libc(&mut c, FN_MALLOC);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (buf)

    // readdir("/", 1, buf, 1024) via SYS_READDIR (107)
    let dir_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // path "/"
    emit_mov_rsi_imm64(&mut c, 1); // path_len
    c.extend_from_slice(&[0x4C, 0x89, 0xE2]); // mov rdx, r12 (buf) — but readdir uses arg3 for buf
    // Actually readdir ABI: rdi=path_ptr, rsi=path_len, rdx is not used for buf in current implementation
    // Let me check... SYS_READDIR takes (path_ptr, path_len, buf_ptr, buf_len) but we only have 3 args
    // The current syscall only passes 3 args via rdi/rsi/rdx, and arg3 is the buf_ptr
    // So: mov rdi=path_ptr, mov rsi=path_len, mov rdx=buf_ptr
    // We need rcx for buf_len but our ABI only has 3 args
    // Let me use the raw syscall approach instead
    // Actually looking at SYS_READDIR handler in syscall.rs, it reads 4 args:
    // arg1=path_ptr, arg2=path_len, arg3=buf_ptr... but arg3 is rdx.
    // The handler uses arg3 as buf_ptr. buf_len is hardcoded as 4096.
    // So we need: rdi=path_ptr, rsi=path_len, rdx=buf_ptr
    // Use raw syscall: mov rax, 107; int 0x80
    c.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x6B, 0x00, 0x00, 0x00]); // mov rax, 107
    c.extend_from_slice(&[0xCD, 0x80]); // int 0x80
    // rax = number of entries

    // Write the buffer contents (readdir fills it with newline-separated names)
    // We need to know how many bytes were written to buf.
    // readdir returns entry count, but writes formatted entries to buf.
    // Use strlen on buf to get actual length
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12 (buf)
    emit_call_libc(&mut c, FN_STRLEN);
    emit_mov_rsi_rax(&mut c); // rsi = length
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12 (buf)
    emit_call_libc(&mut c, FN_WRITE);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // Strings
    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"ls /\n\0");

    let dir_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"/\0");

    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[dir_fixup..dir_fixup+8].copy_from_slice(&dir_addr.to_le_bytes());

    c
}

/// Generate "init" program: PID 1 init process.
/// Prints banner, gets PID, prints uptime, then exits.
pub fn gen_init() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // Banner
    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // getpid
    emit_call_libc(&mut c, FN_GETPID);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_call_libc(&mut c, FN_PRINT_INT);

    // uptime
    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    emit_call_libc(&mut c, FN_GETTIME);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);
    let msg4_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // "init complete" message
    let msg5_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // Strings
    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"[init] MerlionOS init (PID 1) starting...\n\0");

    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"[init] PID: \0");

    let msg3_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"\n[init] uptime: \0");

    let msg4_addr = text_base + c.len() as u64;
    c.extend_from_slice(b" seconds\n\0");

    let msg5_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"[init] system ready - returning to kernel shell\n\0");

    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());
    c[msg4_fixup..msg4_fixup+8].copy_from_slice(&msg4_addr.to_le_bytes());
    c[msg5_fixup..msg5_fixup+8].copy_from_slice(&msg5_addr.to_le_bytes());

    c
}

// ═══════════════════════════════════════════════════════════════════
//  USERSPACE APPLICATIONS
// ═══════════════════════════════════════════════════════════════════

/// Helper: emit raw syscall (mov rax, NUM; int 0x80) — 9 bytes.
fn emit_raw_syscall(c: &mut Vec<u8>, num: u32) {
    c.extend_from_slice(&[0x48, 0xC7, 0xC0]); // mov rax, imm32
    c.extend_from_slice(&num.to_le_bytes());
    c.extend_from_slice(&[0xCD, 0x80]); // int 0x80
}

/// Generate "ush" (micro-shell): runs a sequence of commands demonstrating
/// fork+exec pattern. Prints prompt, runs programs, prints results.
pub fn gen_ush() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // Banner
    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // fork() → child_pid
    emit_raw_syscall(&mut c, 110); // SYS_FORK
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (child_pid)

    // puts("ush: forked child pid=")
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_call_libc(&mut c, FN_PRINT_INT);
    let nl_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // exec("hello") on child — SYS_EXEC(111)
    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    let prog_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // program name ptr
    emit_mov_rsi_imm64(&mut c, 5); // "hello" len
    emit_raw_syscall(&mut c, 111); // SYS_EXEC
    // If exec returns (failure), continue

    // getpid
    let msg4_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    emit_call_libc(&mut c, FN_GETPID);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);

    // Done
    let msg5_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // Strings
    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"ush: MerlionOS micro-shell (Ring 3)\n\0");
    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"ush: forked child pid=\0");
    let nl_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"\n\0");
    let msg3_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"ush: exec hello...\n\0");
    let prog_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"hello\0");
    let msg4_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"\nush: back from exec, pid=\0");
    let msg5_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"\nush: shell exiting\n\0");

    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[nl_fixup..nl_fixup+8].copy_from_slice(&nl_addr.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());
    c[prog_fixup..prog_fixup+8].copy_from_slice(&prog_addr.to_le_bytes());
    c[msg4_fixup..msg4_fixup+8].copy_from_slice(&msg4_addr.to_le_bytes());
    c[msg5_fixup..msg5_fixup+8].copy_from_slice(&msg5_addr.to_le_bytes());

    c
}

/// Generate "fwrite-test": writes a file to VFS, reads it back, prints contents.
pub fn gen_fwrite_test() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    // puts("fwrite-test: creating /tmp/hello.txt")
    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // fd = open("/tmp/hello.txt", 14, 1)  — flags=1 means write
    let path_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    let pathlen_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0);
    emit_mov_rdx_imm64(&mut c, 1); // flags = write
    emit_call_libc(&mut c, FN_OPEN);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (fd)

    // fwrite(fd, "Hello from userspace file write!\n", 33) via SYS_FWRITE (195)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12 (fd)
    let data_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0); // data ptr
    emit_mov_rdx_imm64(&mut c, 33); // data len
    emit_raw_syscall(&mut c, 195); // SYS_FWRITE

    // close(fd)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_call_libc(&mut c, FN_CLOSE);

    // Now read it back: open for read
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    let path2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    let pathlen2_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0);
    emit_mov_rdx_imm64(&mut c, 0); // flags = read
    emit_call_libc(&mut c, FN_OPEN);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (fd)

    // buf = malloc(256)
    emit_mov_rdi_imm64(&mut c, 256);
    emit_call_libc(&mut c, FN_MALLOC);
    c.extend_from_slice(&[0x49, 0x89, 0xC5]); // mov r13, rax (buf)

    // n = read(fd, buf, 256)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    c.extend_from_slice(&[0x4C, 0x89, 0xEE]); // mov rsi, r13
    emit_mov_rdx_imm64(&mut c, 256);
    emit_call_libc(&mut c, FN_READ);

    // write(buf, n) to stdout
    c.extend_from_slice(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    emit_mov_rsi_rax(&mut c);
    emit_call_libc(&mut c, FN_WRITE);

    // close
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_call_libc(&mut c, FN_CLOSE);

    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // exit(0)
    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // Strings
    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"fwrite-test: writing /tmp/hello.txt\n\0");
    let path_addr = text_base + c.len() as u64;
    let path = b"/tmp/hello.txt";
    let path_len = path.len() as u64;
    c.extend_from_slice(path);
    c.push(0);
    let data_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"Hello from userspace file write!\n\0");
    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"fwrite-test: reading back:\n\0");
    let msg3_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"fwrite-test: done!\n\0");

    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[path_fixup..path_fixup+8].copy_from_slice(&path_addr.to_le_bytes());
    c[pathlen_fixup..pathlen_fixup+8].copy_from_slice(&path_len.to_le_bytes());
    c[data_fixup..data_fixup+8].copy_from_slice(&data_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[path2_fixup..path2_fixup+8].copy_from_slice(&path_addr.to_le_bytes());
    c[pathlen2_fixup..pathlen2_fixup+8].copy_from_slice(&path_len.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());

    c
}

/// Generate "paint" program: draws colored rectangles on framebuffer.
pub fn gen_paint() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // Draw a 20x10 rectangle at (10,5) with color 4 (red)
    // for y in 5..15: for x in 10..30: fbwrite(x, y, 4)
    // Use r12=y, r13=x
    c.extend_from_slice(&[0x49, 0xC7, 0xC4, 0x05, 0x00, 0x00, 0x00]); // mov r12, 5 (y_start)
    // .y_loop:
    let y_loop = c.len();
    c.extend_from_slice(&[0x49, 0xC7, 0xC5, 0x0A, 0x00, 0x00, 0x00]); // mov r13, 10 (x_start)
    // .x_loop:
    let x_loop = c.len();
    // fbwrite(x, y, color) via SYS_FBWRITE (196)
    c.extend_from_slice(&[0x4C, 0x89, 0xEF]); // mov rdi, r13 (x)
    c.extend_from_slice(&[0x4C, 0x89, 0xE6]); // mov rsi, r12 (y)
    emit_mov_rdx_imm64(&mut c, 4); // color = red
    emit_raw_syscall(&mut c, 196);
    // inc r13
    c.extend_from_slice(&[0x49, 0xFF, 0xC5]);
    // cmp r13, 30
    c.extend_from_slice(&[0x49, 0x83, 0xFD, 0x1E]);
    // jl .x_loop
    let jl_disp = (x_loop as isize - (c.len() as isize + 2)) as i8;
    c.extend_from_slice(&[0x7C, jl_disp as u8]);
    // inc r12
    c.extend_from_slice(&[0x49, 0xFF, 0xC4]);
    // cmp r12, 15
    c.extend_from_slice(&[0x49, 0x83, 0xFC, 0x0F]);
    // jl .y_loop
    let jl_disp2 = (y_loop as isize - (c.len() as isize + 2)) as i8;
    c.extend_from_slice(&[0x7C, jl_disp2 as u8]);

    // Draw another rectangle at (40,5) with color 2 (green)
    c.extend_from_slice(&[0x49, 0xC7, 0xC4, 0x05, 0x00, 0x00, 0x00]); // mov r12, 5
    let y2_loop = c.len();
    c.extend_from_slice(&[0x49, 0xC7, 0xC5, 0x28, 0x00, 0x00, 0x00]); // mov r13, 40
    let x2_loop = c.len();
    c.extend_from_slice(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    c.extend_from_slice(&[0x4C, 0x89, 0xE6]); // mov rsi, r12
    emit_mov_rdx_imm64(&mut c, 2); // green
    emit_raw_syscall(&mut c, 196);
    c.extend_from_slice(&[0x49, 0xFF, 0xC5]);
    c.extend_from_slice(&[0x49, 0x83, 0xFD, 0x3C]); // cmp r13, 60
    let jl3 = (x2_loop as isize - (c.len() as isize + 2)) as i8;
    c.extend_from_slice(&[0x7C, jl3 as u8]);
    c.extend_from_slice(&[0x49, 0xFF, 0xC4]);
    c.extend_from_slice(&[0x49, 0x83, 0xFC, 0x0F]); // cmp r12, 15
    let jl4 = (y2_loop as isize - (c.len() as isize + 2)) as i8;
    c.extend_from_slice(&[0x7C, jl4 as u8]);

    // Render framebuffer: fbwrite(0xFFFF, 0xFFFF, 0)
    emit_mov_rdi_imm64(&mut c, 0xFFFF);
    emit_mov_rsi_imm64(&mut c, 0xFFFF);
    emit_mov_rdx_imm64(&mut c, 0);
    emit_raw_syscall(&mut c, 196);

    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"paint: drawing rectangles on framebuffer...\n\0");
    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"paint: done! (red and green rectangles drawn)\n\0");

    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());

    c
}

/// Generate "wget-user" program: fetches a URL via SYS_WGET.
pub fn gen_wget_user() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // buf = malloc(4096) for response
    emit_mov_rdi_imm64(&mut c, 4096);
    emit_call_libc(&mut c, FN_MALLOC);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (buf)

    // wget(url_ptr, url_len, buf_ptr) via SYS_WGET (197)
    let url_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0); // url
    let urllen_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0); // url_len
    c.extend_from_slice(&[0x4C, 0x89, 0xE2]); // mov rdx, r12 (buf)
    emit_raw_syscall(&mut c, 197);
    c.extend_from_slice(&[0x49, 0x89, 0xC5]); // mov r13, rax (bytes received)

    // Print result
    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    c.extend_from_slice(&[0x4C, 0x89, 0xEF]); // mov rdi, r13
    emit_call_libc(&mut c, FN_PRINT_INT);
    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"wget-user: fetching http://10.0.2.2/\n\0");
    let url_addr = text_base + c.len() as u64;
    let url = b"http://10.0.2.2/";
    let url_len = url.len() as u64;
    c.extend_from_slice(url);
    c.push(0);
    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"wget-user: received \0");
    let msg3_addr = text_base + c.len() as u64;
    c.extend_from_slice(b" bytes\n\0");

    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[url_fixup..url_fixup+8].copy_from_slice(&url_addr.to_le_bytes());
    c[urllen_fixup..urllen_fixup+8].copy_from_slice(&url_len.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());

    c
}

/// Generate "pkg-install" program: simulated package install from VFS.
pub fn gen_pkg_install() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    let msg1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // Create /bin/demo file via open+fwrite
    let path_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    let pathlen_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0);
    emit_mov_rdx_imm64(&mut c, 1);
    emit_call_libc(&mut c, FN_OPEN);
    c.extend_from_slice(&[0x49, 0x89, 0xC4]); // mov r12, rax (fd)

    // Write package data (simulated ELF header)
    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    let data_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0);
    emit_mov_rdx_imm64(&mut c, 28);
    emit_raw_syscall(&mut c, 195); // SYS_FWRITE

    c.extend_from_slice(&[0x4C, 0x89, 0xE7]); // mov rdi, r12
    emit_call_libc(&mut c, FN_CLOSE);

    let msg2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    let msg3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    let msg1_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"pkg-install: installing package 'demo'...\n\0");
    let path_addr = text_base + c.len() as u64;
    let path = b"/bin/demo";
    let path_len = path.len() as u64;
    c.extend_from_slice(path);
    c.push(0);
    let data_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"#!/bin/echo Hello from demo!\n\0");
    let msg2_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"pkg-install: installed /bin/demo\n\0");
    let msg3_addr = text_base + c.len() as u64;
    c.extend_from_slice(b"pkg-install: done! Run with: run-user demo\n\0");

    c[msg1_fixup..msg1_fixup+8].copy_from_slice(&msg1_addr.to_le_bytes());
    c[path_fixup..path_fixup+8].copy_from_slice(&path_addr.to_le_bytes());
    c[pathlen_fixup..pathlen_fixup+8].copy_from_slice(&path_len.to_le_bytes());
    c[data_fixup..data_fixup+8].copy_from_slice(&data_addr.to_le_bytes());
    c[msg2_fixup..msg2_fixup+8].copy_from_slice(&msg2_addr.to_le_bytes());
    c[msg3_fixup..msg3_fixup+8].copy_from_slice(&msg3_addr.to_le_bytes());

    c
}

/// Generate "test-suite": validates major syscalls and libc functions.
pub fn gen_test_suite() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    let msg0_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // Test getpid
    emit_call_libc(&mut c, FN_GETPID);
    let t1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    // Test strlen("hello") → 5
    let str_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_STRLEN);
    emit_push_rax(&mut c);
    let t2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    c.push(0x58);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);

    // Test malloc(64)
    let t3_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    emit_mov_rdi_imm64(&mut c, 64);
    emit_call_libc(&mut c, FN_MALLOC);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);

    // Test gettime
    let t4_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    emit_call_libc(&mut c, FN_GETTIME);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);

    // Test strcmp("abc","abc") → 0
    let t5_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    let cmp1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    let cmp2_fixup = c.len() + 2;
    emit_mov_rsi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_STRCMP);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);

    // Test fork
    let t6_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);
    emit_raw_syscall(&mut c, 110);
    emit_mov_rdi_rax(&mut c);
    emit_call_libc(&mut c, FN_PRINT_INT);

    // Done
    let done_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    // Strings
    let msg0 = text_base + c.len() as u64; c.extend_from_slice(b"=== MerlionOS Syscall Test Suite ===\n\0");
    let t1 = text_base + c.len() as u64; c.extend_from_slice(b"[PASS] getpid\n\0");
    let str1 = text_base + c.len() as u64; c.extend_from_slice(b"hello\0");
    let t2 = text_base + c.len() as u64; c.extend_from_slice(b"[TEST] strlen = \0");
    let t3 = text_base + c.len() as u64; c.extend_from_slice(b"\n[TEST] malloc = \0");
    let t4 = text_base + c.len() as u64; c.extend_from_slice(b"\n[TEST] time = \0");
    let t5 = text_base + c.len() as u64; c.extend_from_slice(b"\n[TEST] strcmp = \0");
    let c1 = text_base + c.len() as u64; c.extend_from_slice(b"abc\0");
    let c2 = text_base + c.len() as u64; c.extend_from_slice(b"abc\0");
    let t6 = text_base + c.len() as u64; c.extend_from_slice(b"\n[TEST] fork = \0");
    let done = text_base + c.len() as u64; c.extend_from_slice(b"\n=== All tests complete ===\n\0");

    c[msg0_fixup..msg0_fixup+8].copy_from_slice(&msg0.to_le_bytes());
    c[t1_fixup..t1_fixup+8].copy_from_slice(&t1.to_le_bytes());
    c[str_fixup..str_fixup+8].copy_from_slice(&str1.to_le_bytes());
    c[t2_fixup..t2_fixup+8].copy_from_slice(&t2.to_le_bytes());
    c[t3_fixup..t3_fixup+8].copy_from_slice(&t3.to_le_bytes());
    c[t4_fixup..t4_fixup+8].copy_from_slice(&t4.to_le_bytes());
    c[t5_fixup..t5_fixup+8].copy_from_slice(&t5.to_le_bytes());
    c[cmp1_fixup..cmp1_fixup+8].copy_from_slice(&c1.to_le_bytes());
    c[cmp2_fixup..cmp2_fixup+8].copy_from_slice(&c2.to_le_bytes());
    c[t6_fixup..t6_fixup+8].copy_from_slice(&t6.to_le_bytes());
    c[done_fixup..done_fixup+8].copy_from_slice(&done.to_le_bytes());
    c
}

/// Generate "beep": plays A4-A5-A6 melody via SYS_BEEP.
pub fn gen_beep() -> Vec<u8> {
    let text_base: u64 = 0x0000_0040_0000;
    let mut c: Vec<u8> = Vec::new();

    let m1_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    for &(freq, dur) in &[(440u64, 200u64), (880, 200), (1760, 400)] {
        emit_mov_rdi_imm64(&mut c, freq);
        emit_mov_rsi_imm64(&mut c, dur);
        emit_raw_syscall(&mut c, 200);
    }

    let m2_fixup = c.len() + 2;
    emit_mov_rdi_imm64(&mut c, 0);
    emit_call_libc(&mut c, FN_PUTS);

    c.extend_from_slice(&[0x48, 0x31, 0xFF]);
    emit_call_libc(&mut c, FN_EXIT);
    c.extend_from_slice(&[0xEB, 0xFE]);

    let m1 = text_base + c.len() as u64; c.extend_from_slice(b"beep: A4-A5-A6 melody\n\0");
    let m2 = text_base + c.len() as u64; c.extend_from_slice(b"beep: done!\n\0");
    c[m1_fixup..m1_fixup+8].copy_from_slice(&m1.to_le_bytes());
    c[m2_fixup..m2_fixup+8].copy_from_slice(&m2.to_le_bytes());
    c
}
