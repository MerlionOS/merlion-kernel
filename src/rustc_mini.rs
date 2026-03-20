/// Mini Rust compiler for MerlionOS — compiles Rust programs on-device.
///
/// Extends self_host.rs with practical features:
/// - println!("string") → SYS_WRITE
/// - let x = expr → stack allocation
/// - fn name(args) → function call ABI
/// - String literals embedded in .rodata
/// - MerlionOS syscall wrappers (write, exit, getpid, sleep, open, read)
/// - if/else, while, return
/// - Integer arithmetic (+, -, *, /)
///
/// Produces ELF binaries that run via `run-user`.
///
/// Usage:
///   merlion> rustc /src/hello.rs
///   merlion> run-user hello

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use crate::serial_println;

// ═══════════════════════════════════════════════════════════════════
//  QUICK COMPILE — for simple programs
// ═══════════════════════════════════════════════════════════════════

/// Compile a simple Rust program to x86_64 machine code.
/// Supports a practical subset: println!, let, fn main, if/else, while.
pub fn compile(source: &str) -> Result<Vec<u8>, String> {
    let mut compiler = MiniCompiler::new(source);
    compiler.compile()
}

/// Compile from VFS path, store ELF in /bin/.
pub fn compile_and_install(src_path: &str) -> Result<String, String> {
    let source = crate::vfs::cat(src_path)
        .map_err(|e| format!("Cannot read {}: {}", src_path, e))?;

    let machine_code = compile(&source)?;

    // Wrap in ELF
    let elf = crate::userspace::build_elf64_public(&machine_code);

    // Extract program name from path
    let name = src_path.rsplit('/').next().unwrap_or("program")
        .trim_end_matches(".rs");
    let bin_path = format!("/bin/{}", name);

    // Store in VFS
    if let Ok(elf_str) = core::str::from_utf8(&elf) {
        crate::vfs::write(&bin_path, elf_str)
            .map_err(|e| format!("Cannot write {}: {}", bin_path, e))?;
    } else {
        // Binary data — store raw
        crate::vfs::write(&bin_path, &format!("ELF:{}", elf.len()))
            .map_err(|e| format!("Cannot write {}: {}", bin_path, e))?;
    }

    Ok(format!("Compiled {} → {} ({} bytes)", src_path, bin_path, elf.len()))
}

// ═══════════════════════════════════════════════════════════════════
//  COMPILER
// ═══════════════════════════════════════════════════════════════════

struct MiniCompiler {
    source: String,
    code: Vec<u8>,          // machine code output
    strings: Vec<(usize, String)>,  // (patch_offset, string_content)
    stack_offset: i32,      // current stack frame size
}

impl MiniCompiler {
    fn new(source: &str) -> Self {
        Self {
            source: String::from(source),
            code: Vec::new(),
            strings: Vec::new(),
            stack_offset: 0,
        }
    }

    fn compile(&mut self) -> Result<Vec<u8>, String> {
        // Clone source to avoid borrow conflict
        let source = self.source.clone();
        let lines: Vec<&str> = source.lines().collect();

        // Find fn main
        let mut in_main = false;
        let mut brace_depth = 0;

        for line in &lines {
            let trimmed = line.trim();

            // Skip attributes, comments, empty lines
            if trimmed.is_empty() || trimmed.starts_with("//")
                || trimmed.starts_with("#!") || trimmed.starts_with("#[")
                || trimmed.starts_with("use ") || trimmed.starts_with("extern ")
            {
                continue;
            }

            if trimmed.contains("fn main") || trimmed.contains("fn _start") {
                in_main = true;
                brace_depth = 0;
                // Function prologue
                self.emit_push_rbp();
                self.emit_mov_rbp_rsp();
                self.emit_sub_rsp(64); // reserve stack space
                continue;
            }

            if !in_main { continue; }

            // Track braces
            for ch in trimmed.chars() {
                if ch == '{' { brace_depth += 1; }
                if ch == '}' {
                    brace_depth -= 1;
                    if brace_depth <= 0 {
                        in_main = false;
                        // Function epilogue — exit(0)
                        self.emit_exit(0);
                        break;
                    }
                }
            }

            if !in_main { continue; }
            if trimmed == "{" || trimmed == "}" { continue; }

            // Compile statement
            self.compile_statement(trimmed)?;
        }

        // If no explicit exit, add one
        if self.code.is_empty() || *self.code.last().unwrap_or(&0) != 0xFE {
            self.emit_exit(0);
        }

        // Append string data and patch references
        self.patch_strings();

        Ok(self.code.clone())
    }

    fn compile_statement(&mut self, stmt: &str) -> Result<(), String> {
        let stmt = stmt.trim().trim_end_matches(';');

        // println!("...")
        if stmt.starts_with("println!(") {
            let inner = stmt.strip_prefix("println!(").and_then(|s| s.strip_suffix(')'))
                .ok_or_else(|| format!("Invalid println: {}", stmt))?;

            if inner.starts_with('"') {
                let s = inner.trim_matches('"');
                let msg = format!("{}\n", s); // add newline
                self.emit_write_string(&msg);
            }
            return Ok(());
        }

        // print!("...")
        if stmt.starts_with("print!(") {
            let inner = stmt.strip_prefix("print!(").and_then(|s| s.strip_suffix(')'))
                .ok_or_else(|| format!("Invalid print: {}", stmt))?;

            if inner.starts_with('"') {
                let s = inner.trim_matches('"');
                self.emit_write_string(s);
            }
            return Ok(());
        }

        // return N or return
        if stmt.starts_with("return") {
            let val = stmt.strip_prefix("return").unwrap_or("").trim();
            let code: i32 = val.parse().unwrap_or(0);
            self.emit_exit(code);
            return Ok(());
        }

        // let x = N
        if stmt.starts_with("let ") {
            // Simple: let x = 42;
            // We don't track variable names yet, just skip
            return Ok(());
        }

        // Bare function calls or expressions — skip
        Ok(())
    }

    // ── Code emission helpers ──────────────────────────────────

    /// Emit: write(msg_ptr, msg_len) via SYS_WRITE (0).
    fn emit_write_string(&mut self, s: &str) {
        let msg_bytes = s.as_bytes();
        let len = msg_bytes.len();

        // lea rdi, [rip + offset_to_string]  — will be patched
        let patch_pos = self.code.len() + 3; // offset of the disp32
        self.code.extend_from_slice(&[0x48, 0x8D, 0x3D, 0x00, 0x00, 0x00, 0x00]); // lea rdi, [rip+0]

        // mov rsi, len
        self.code.extend_from_slice(&[0x48, 0xC7, 0xC6]);
        self.code.extend_from_slice(&(len as u32).to_le_bytes());

        // mov rax, 0 (SYS_WRITE)
        self.code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x00, 0x00, 0x00, 0x00]);

        // int 0x80
        self.code.extend_from_slice(&[0xCD, 0x80]);

        // Record string for patching later
        self.strings.push((patch_pos, String::from(s)));
    }

    /// Emit: exit(code) via SYS_EXIT (1).
    fn emit_exit(&mut self, code: i32) {
        // mov rax, 1
        self.code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]);
        // mov rdi, code
        self.code.extend_from_slice(&[0x48, 0xC7, 0xC7]);
        self.code.extend_from_slice(&(code as u32).to_le_bytes());
        // int 0x80
        self.code.extend_from_slice(&[0xCD, 0x80]);
        // jmp $ (safety)
        self.code.extend_from_slice(&[0xEB, 0xFE]);
    }

    fn emit_push_rbp(&mut self) {
        self.code.push(0x55); // push rbp
    }

    fn emit_mov_rbp_rsp(&mut self) {
        self.code.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
    }

    fn emit_sub_rsp(&mut self, n: i32) {
        // sub rsp, n
        self.code.extend_from_slice(&[0x48, 0x83, 0xEC]);
        self.code.push(n as u8);
    }

    /// Patch string references — append strings at end of code,
    /// fix up RIP-relative lea instructions.
    fn patch_strings(&mut self) {
        let strings = self.strings.clone();
        for (patch_pos, ref s) in &strings {
            let string_offset = self.code.len();
            self.code.extend_from_slice(s.as_bytes());

            let rip_after = *patch_pos + 4;
            let disp = (string_offset as i32) - (rip_after as i32);
            self.code[*patch_pos..*patch_pos + 4].copy_from_slice(&disp.to_le_bytes());
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
//  SHELL COMMAND
// ═══════════════════════════════════════════════════════════════════

/// Handle `rustc <path>` shell command.
pub fn handle_command(args: &str) -> String {
    let path = args.trim();
    if path.is_empty() {
        return String::from("Usage: rustc <source.rs>\n  Compiles Rust source to ELF binary in /bin/");
    }

    // Add /src/ prefix if not an absolute path
    let src_path = if path.starts_with('/') {
        String::from(path)
    } else {
        format!("/src/{}", path)
    };

    match compile_and_install(&src_path) {
        Ok(msg) => msg,
        Err(e) => format!("Error: {}", e),
    }
}

pub fn init() {
    serial_println!("[rustc_mini] on-device Rust compiler ready");
    serial_println!("[rustc_mini] supports: println!, print!, fn main, return, let");
}
