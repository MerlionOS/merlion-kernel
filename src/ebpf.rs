/// Extended BPF (eBPF) for MerlionOS.
/// Provides a programmable in-kernel virtual machine with maps,
/// helper functions, and XDP (eXpress Data Path) support.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ── Instruction classes (3 bits) ──
const BPF_LD: u8    = 0x00;
const BPF_LDX: u8   = 0x01;
const BPF_ST: u8     = 0x02;
const BPF_STX: u8    = 0x03;
const BPF_ALU: u8    = 0x04;
const BPF_JMP: u8    = 0x05;
const BPF_JMP32: u8  = 0x06;
const BPF_ALU64: u8  = 0x07;

// ── ALU/JMP operations (4 bits, shifted left 4) ──
const BPF_ADD: u8  = 0x00;
const BPF_SUB: u8  = 0x10;
const BPF_MUL: u8  = 0x20;
const BPF_DIV: u8  = 0x30;
const BPF_OR: u8   = 0x40;
const BPF_AND: u8  = 0x50;
const BPF_LSH: u8  = 0x60;
const BPF_RSH: u8  = 0x70;
const BPF_NEG: u8  = 0x80;
const BPF_MOD: u8  = 0x90;
const BPF_XOR: u8  = 0xa0;
const BPF_MOV: u8  = 0xb0;
const BPF_ARSH: u8 = 0xc0;

// ── JMP operations ──
const BPF_JA: u8   = 0x00;
const BPF_JEQ: u8  = 0x10;
const BPF_JGT: u8  = 0x20;
const BPF_JGE: u8  = 0x30;
const BPF_JSET: u8 = 0x40;
const BPF_JNE: u8  = 0x50;
const BPF_JSGT: u8 = 0x60;
const BPF_JSGE: u8 = 0x70;
const BPF_CALL: u8 = 0x80;
const BPF_EXIT: u8 = 0x90;
const BPF_JLT: u8  = 0xa0;
const BPF_JLE: u8  = 0xb0;

// ── Source modifier ──
const BPF_K: u8 = 0x00;
const BPF_X: u8 = 0x08;

// ── Size modifiers for LD/ST ──
const BPF_W: u8  = 0x00;
const BPF_H: u8  = 0x08;
const BPF_B: u8  = 0x10;
const BPF_DW: u8 = 0x18;

// ── Memory modes ──
const BPF_MEM: u8 = 0x60;

/// Maximum program length.
const MAX_INSNS: usize = 4096;

/// Maximum maps per program.
const MAX_MAPS: usize = 64;

/// eBPF stack size.
const STACK_SIZE: usize = 512;

/// Register count (R0-R10).
const NUM_REGS: usize = 11;

/// A single 64-bit eBPF instruction.
#[derive(Debug, Clone, Copy)]
pub struct EbpfInsn {
    pub opcode: u8,
    pub regs: u8,      // dst:4 | src:4
    pub offset: i16,
    pub imm: i32,
}

impl EbpfInsn {
    pub const fn new(opcode: u8, dst: u8, src: u8, offset: i16, imm: i32) -> Self {
        Self {
            opcode,
            regs: (dst & 0x0f) | ((src & 0x0f) << 4),
            offset,
            imm,
        }
    }

    /// Destination register (low 4 bits of regs).
    #[inline]
    pub fn dst(&self) -> usize { (self.regs & 0x0f) as usize }

    /// Source register (high 4 bits of regs).
    #[inline]
    pub fn src(&self) -> usize { ((self.regs >> 4) & 0x0f) as usize }
}

/// Map types available in the eBPF subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapType {
    HashMap,
    Array,
    PerCpuArray,
    LruHash,
    RingBuffer,
}

/// An eBPF map — a key-value data structure accessible from programs.
pub struct EbpfMap {
    pub id: u32,
    pub map_type: MapType,
    pub key_size: u32,
    pub value_size: u32,
    pub max_entries: u32,
    entries: Vec<(Vec<u8>, Vec<u8>)>,
}

impl EbpfMap {
    fn new(id: u32, map_type: MapType, key_size: u32, value_size: u32, max_entries: u32) -> Self {
        Self {
            id,
            map_type,
            key_size,
            value_size,
            max_entries,
            entries: Vec::new(),
        }
    }

    fn lookup(&self, key: &[u8]) -> Option<&Vec<u8>> {
        self.entries.iter().find(|(k, _)| k.as_slice() == key).map(|(_, v)| v)
    }

    fn update(&mut self, key: &[u8], value: &[u8]) -> Result<(), &'static str> {
        if key.len() != self.key_size as usize {
            return Err("key size mismatch");
        }
        if value.len() != self.value_size as usize {
            return Err("value size mismatch");
        }
        if let Some(entry) = self.entries.iter_mut().find(|(k, _)| k.as_slice() == key) {
            entry.1 = value.into();
        } else {
            if self.entries.len() >= self.max_entries as usize {
                // For LRU, evict first entry
                if self.map_type == MapType::LruHash && !self.entries.is_empty() {
                    self.entries.remove(0);
                } else {
                    return Err("map full");
                }
            }
            self.entries.push((key.into(), value.into()));
        }
        Ok(())
    }

    fn delete(&mut self, key: &[u8]) -> bool {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k.as_slice() == key) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }
}

/// Helper functions callable from eBPF programs via CALL instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum HelperFn {
    MapLookup = 1,
    MapUpdate = 2,
    MapDelete = 3,
    KtimeGetNs = 5,
    TracePrintk = 6,
    GetCurrentPid = 14,
    GetCurrentComm = 16,
    Redirect = 23,
    PerfEventOutput = 25,
    SkbLoadBytes = 26,
    XdpAdjustHead = 44,
}

/// XDP action returned by XDP programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum XdpAction {
    Aborted = 0,
    Drop = 1,
    Pass = 2,
    Tx = 3,
    Redirect = 4,
}

impl XdpAction {
    fn from_u64(v: u64) -> Self {
        match v {
            0 => Self::Aborted,
            1 => Self::Drop,
            2 => Self::Pass,
            3 => Self::Tx,
            4 => Self::Redirect,
            _ => Self::Aborted,
        }
    }
}

/// Program types supported by the eBPF subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramType {
    Xdp,
    TcClassifier,
    SocketFilter,
    Kprobe,
    Tracepoint,
}

/// The eBPF virtual machine.
pub struct EbpfVm {
    program: Vec<EbpfInsn>,
    regs: [u64; NUM_REGS],
    stack: [u8; STACK_SIZE],
    maps: Vec<u32>,
}

impl EbpfVm {
    /// Create a new eBPF VM with a verified program.
    pub fn new(program: Vec<EbpfInsn>) -> Result<Self, &'static str> {
        if program.is_empty() {
            return Err("empty program");
        }
        if program.len() > MAX_INSNS {
            return Err("program too long");
        }
        // Basic verification
        Self::verify(&program)?;
        Ok(Self {
            program,
            regs: [0u64; NUM_REGS],
            stack: [0u8; STACK_SIZE],
            maps: Vec::new(),
        })
    }

    /// Verify the program for safety.
    fn verify(program: &[EbpfInsn]) -> Result<(), &'static str> {
        let len = program.len();
        for (i, insn) in program.iter().enumerate() {
            let class = insn.opcode & 0x07;
            let op = insn.opcode & 0xf0;

            // Check register indices
            if insn.dst() >= NUM_REGS {
                return Err("invalid destination register");
            }
            if insn.src() >= NUM_REGS {
                return Err("invalid source register");
            }

            // Check jump targets
            if class == BPF_JMP || class == BPF_JMP32 {
                if op == BPF_CALL || op == BPF_EXIT {
                    continue;
                }
                if op == BPF_JA {
                    let target = i as i64 + 1 + insn.offset as i64;
                    if target < 0 || target as usize >= len {
                        return Err("jump target out of bounds");
                    }
                } else {
                    let target = i as i64 + 1 + insn.offset as i64;
                    if target < 0 || target as usize >= len {
                        return Err("conditional jump target out of bounds");
                    }
                }
            }

            // Check ALU division by zero with immediates
            if (class == BPF_ALU || class == BPF_ALU64) &&
               (op == BPF_DIV || op == BPF_MOD) &&
               (insn.opcode & BPF_X) == 0 && insn.imm == 0 {
                return Err("division by zero in immediate");
            }

            // Reject writes to R10 (frame pointer, read-only)
            if insn.dst() == 10 && class != BPF_STX && class != BPF_JMP && class != BPF_JMP32 {
                return Err("cannot write to R10 (frame pointer)");
            }
        }

        // Last instruction must be EXIT
        let last = &program[len - 1];
        if (last.opcode & 0x07) != BPF_JMP || (last.opcode & 0xf0) != BPF_EXIT {
            return Err("program does not end with EXIT");
        }

        Ok(())
    }

    /// Add a map ID accessible to this program.
    pub fn add_map(&mut self, map_id: u32) {
        if self.maps.len() < MAX_MAPS {
            self.maps.push(map_id);
        }
    }

    /// Execute the eBPF program with context data (e.g. packet).
    /// Returns R0 (the return value).
    pub fn run(&mut self, ctx: &[u8]) -> u64 {
        self.regs = [0u64; NUM_REGS];
        self.stack = [0u8; STACK_SIZE];
        // R1 = pointer to context (we use the length as a proxy)
        self.regs[1] = ctx.len() as u64;
        // R10 = frame pointer (top of stack)
        self.regs[10] = STACK_SIZE as u64;

        let mut pc: usize = 0;
        let mut insn_count: u64 = 0;
        let max_insns: u64 = 1_000_000; // prevent infinite loops

        while pc < self.program.len() {
            insn_count += 1;
            if insn_count > max_insns {
                return 0; // abort: too many instructions
            }

            let insn = self.program[pc];
            let class = insn.opcode & 0x07;
            let op = insn.opcode & 0xf0;
            let src_flag = insn.opcode & 0x08;
            let dst = insn.dst();
            let src = insn.src();

            match class {
                BPF_ALU => {
                    let s: u32 = if src_flag == BPF_X { self.regs[src] as u32 } else { insn.imm as u32 };
                    let d = self.regs[dst] as u32;
                    let result: u32 = match op {
                        BPF_ADD  => d.wrapping_add(s),
                        BPF_SUB  => d.wrapping_sub(s),
                        BPF_MUL  => d.wrapping_mul(s),
                        BPF_DIV  => { if s == 0 { return 0; } d / s }
                        BPF_MOD  => { if s == 0 { return 0; } d % s }
                        BPF_OR   => d | s,
                        BPF_AND  => d & s,
                        BPF_XOR  => d ^ s,
                        BPF_LSH  => d.wrapping_shl(s),
                        BPF_RSH  => d.wrapping_shr(s),
                        BPF_NEG  => (!d).wrapping_add(1),
                        BPF_MOV  => s,
                        BPF_ARSH => ((d as i32).wrapping_shr(s)) as u32,
                        _ => d,
                    };
                    self.regs[dst] = result as u64;
                }
                BPF_ALU64 => {
                    let s: u64 = if src_flag == BPF_X { self.regs[src] } else { insn.imm as i64 as u64 };
                    let d = self.regs[dst];
                    let result: u64 = match op {
                        BPF_ADD  => d.wrapping_add(s),
                        BPF_SUB  => d.wrapping_sub(s),
                        BPF_MUL  => d.wrapping_mul(s),
                        BPF_DIV  => { if s == 0 { return 0; } d / s }
                        BPF_MOD  => { if s == 0 { return 0; } d % s }
                        BPF_OR   => d | s,
                        BPF_AND  => d & s,
                        BPF_XOR  => d ^ s,
                        BPF_LSH  => d.wrapping_shl(s as u32),
                        BPF_RSH  => d.wrapping_shr(s as u32),
                        BPF_NEG  => (!d).wrapping_add(1),
                        BPF_MOV  => s,
                        BPF_ARSH => ((d as i64).wrapping_shr(s as u32)) as u64,
                        _ => d,
                    };
                    self.regs[dst] = result;
                }
                BPF_JMP | BPF_JMP32 => {
                    if op == BPF_EXIT {
                        return self.regs[0];
                    }
                    if op == BPF_CALL {
                        self.regs[0] = self.call_helper(insn.imm as u32, ctx);
                        pc += 1;
                        continue;
                    }
                    if op == BPF_JA {
                        pc = (pc as i64 + 1 + insn.offset as i64) as usize;
                        continue;
                    }
                    let (d, s) = if class == BPF_JMP32 {
                        (self.regs[dst] as u32 as u64,
                         if src_flag == BPF_X { self.regs[src] as u32 as u64 } else { insn.imm as u32 as u64 })
                    } else {
                        (self.regs[dst],
                         if src_flag == BPF_X { self.regs[src] } else { insn.imm as i64 as u64 })
                    };
                    let cond = match op {
                        BPF_JEQ  => d == s,
                        BPF_JGT  => d > s,
                        BPF_JGE  => d >= s,
                        BPF_JSET => (d & s) != 0,
                        BPF_JNE  => d != s,
                        BPF_JSGT => (d as i64) > (s as i64),
                        BPF_JSGE => (d as i64) >= (s as i64),
                        BPF_JLT  => d < s,
                        BPF_JLE  => d <= s,
                        _ => false,
                    };
                    if cond {
                        pc = (pc as i64 + 1 + insn.offset as i64) as usize;
                        continue;
                    }
                }
                BPF_LD => {
                    // 64-bit immediate load (two instructions)
                    let size = insn.opcode & 0x18;
                    if size == BPF_DW {
                        let lo = insn.imm as u32 as u64;
                        if pc + 1 < self.program.len() {
                            let hi = self.program[pc + 1].imm as u32 as u64;
                            self.regs[dst] = lo | (hi << 32);
                            pc += 2;
                            continue;
                        }
                    }
                    // Absolute packet load
                    let off = insn.imm as usize;
                    self.regs[dst] = self.load_ctx(ctx, off, insn.opcode & 0x18);
                }
                BPF_LDX => {
                    let off = (self.regs[src] as i64 + insn.offset as i64) as usize;
                    self.regs[dst] = self.load_ctx(ctx, off, insn.opcode & 0x18);
                }
                BPF_ST => {
                    let off = (self.regs[dst] as i64 + insn.offset as i64) as usize;
                    self.store_stack(off, insn.imm as u64, insn.opcode & 0x18);
                }
                BPF_STX => {
                    let off = (self.regs[dst] as i64 + insn.offset as i64) as usize;
                    self.store_stack(off, self.regs[src], insn.opcode & 0x18);
                }
                _ => { return 0; }
            }
            pc += 1;
        }
        self.regs[0]
    }

    /// Load a value from context/packet data.
    fn load_ctx(&self, ctx: &[u8], off: usize, size: u8) -> u64 {
        match size {
            BPF_B => {
                if off < ctx.len() { ctx[off] as u64 } else { 0 }
            }
            BPF_H => {
                if off + 2 <= ctx.len() {
                    u16::from_be_bytes([ctx[off], ctx[off+1]]) as u64
                } else { 0 }
            }
            BPF_W => {
                if off + 4 <= ctx.len() {
                    u32::from_be_bytes([ctx[off], ctx[off+1], ctx[off+2], ctx[off+3]]) as u64
                } else { 0 }
            }
            BPF_DW => {
                if off + 8 <= ctx.len() {
                    u64::from_be_bytes([
                        ctx[off], ctx[off+1], ctx[off+2], ctx[off+3],
                        ctx[off+4], ctx[off+5], ctx[off+6], ctx[off+7],
                    ])
                } else { 0 }
            }
            _ => 0,
        }
    }

    /// Store a value to the stack.
    fn store_stack(&mut self, off: usize, val: u64, size: u8) {
        if off >= STACK_SIZE { return; }
        match size {
            BPF_B => {
                if off < STACK_SIZE { self.stack[off] = val as u8; }
            }
            BPF_H => {
                let bytes = (val as u16).to_ne_bytes();
                if off + 2 <= STACK_SIZE {
                    self.stack[off..off+2].copy_from_slice(&bytes);
                }
            }
            BPF_W => {
                let bytes = (val as u32).to_ne_bytes();
                if off + 4 <= STACK_SIZE {
                    self.stack[off..off+4].copy_from_slice(&bytes);
                }
            }
            BPF_DW => {
                let bytes = val.to_ne_bytes();
                if off + 8 <= STACK_SIZE {
                    self.stack[off..off+8].copy_from_slice(&bytes);
                }
            }
            _ => {}
        }
    }

    /// Call a helper function by ID.
    fn call_helper(&self, id: u32, _ctx: &[u8]) -> u64 {
        match id {
            5 => {
                // ktime_get_ns: return uptime in nanoseconds (approximate)
                let ticks = crate::timer::ticks() as u64;
                ticks * 10_000_000 // 100 Hz tick -> 10ms per tick -> 10_000_000 ns
            }
            6 => {
                // trace_printk: just return 0 (logging stub)
                0
            }
            14 => {
                // get_current_pid
                crate::task::current_pid() as u64
            }
            _ => 0,
        }
    }
}

// ── Global eBPF state ──

/// A loaded eBPF program with metadata.
struct LoadedProgram {
    id: u32,
    name: String,
    prog_type: ProgramType,
    vm: EbpfVm,
    run_count: u64,
}

/// XDP attachment to an interface.
struct XdpAttachment {
    iface: String,
    program_id: u32,
    packets_processed: u64,
    packets_dropped: u64,
    packets_passed: u64,
    packets_tx: u64,
    packets_redirect: u64,
}

struct EbpfState {
    programs: Vec<LoadedProgram>,
    maps: Vec<EbpfMap>,
    xdp_attachments: Vec<XdpAttachment>,
}

impl EbpfState {
    const fn new() -> Self {
        Self {
            programs: Vec::new(),
            maps: Vec::new(),
            xdp_attachments: Vec::new(),
        }
    }
}

static STATE: Mutex<EbpfState> = Mutex::new(EbpfState::new());
static NEXT_PROG_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_MAP_ID: AtomicU32 = AtomicU32::new(1);
static TOTAL_RUNS: AtomicU64 = AtomicU64::new(0);

/// Initialise the eBPF subsystem.
pub fn init() {
    let mut st = STATE.lock();
    st.programs = Vec::new();
    st.maps = Vec::new();
    st.xdp_attachments = Vec::new();
    crate::serial_println!("[ebpf] extended BPF subsystem initialised");
}

/// Load an eBPF program. Returns the program ID.
pub fn load_program(name: &str, prog_type: ProgramType, program: Vec<EbpfInsn>) -> Result<u32, &'static str> {
    let vm = EbpfVm::new(program)?;
    let id = NEXT_PROG_ID.fetch_add(1, Ordering::Relaxed);
    let mut st = STATE.lock();
    st.programs.push(LoadedProgram {
        id,
        name: String::from(name),
        prog_type,
        vm,
        run_count: 0,
    });
    Ok(id)
}

/// Unload an eBPF program by ID.
pub fn unload_program(prog_id: u32) -> bool {
    let mut st = STATE.lock();
    // Remove any XDP attachments using this program
    st.xdp_attachments.retain(|a| a.program_id != prog_id);
    if let Some(pos) = st.programs.iter().position(|p| p.id == prog_id) {
        st.programs.remove(pos);
        true
    } else {
        false
    }
}

/// Create an eBPF map. Returns the map ID.
pub fn map_create(map_type: MapType, key_size: u32, value_size: u32, max_entries: u32) -> u32 {
    let id = NEXT_MAP_ID.fetch_add(1, Ordering::Relaxed);
    let mut st = STATE.lock();
    st.maps.push(EbpfMap::new(id, map_type, key_size, value_size, max_entries));
    id
}

/// Look up a value in an eBPF map.
pub fn map_lookup(map_id: u32, key: &[u8]) -> Option<Vec<u8>> {
    let st = STATE.lock();
    st.maps.iter().find(|m| m.id == map_id)
        .and_then(|m| m.lookup(key).cloned())
}

/// Update a key-value pair in an eBPF map.
pub fn map_update(map_id: u32, key: &[u8], value: &[u8]) -> Result<(), &'static str> {
    let mut st = STATE.lock();
    let map = st.maps.iter_mut().find(|m| m.id == map_id)
        .ok_or("map not found")?;
    map.update(key, value)
}

/// Delete a key from an eBPF map.
pub fn map_delete(map_id: u32, key: &[u8]) -> bool {
    let mut st = STATE.lock();
    if let Some(map) = st.maps.iter_mut().find(|m| m.id == map_id) {
        map.delete(key)
    } else {
        false
    }
}

/// Destroy an eBPF map.
pub fn map_destroy(map_id: u32) -> bool {
    let mut st = STATE.lock();
    if let Some(pos) = st.maps.iter().position(|m| m.id == map_id) {
        st.maps.remove(pos);
        true
    } else {
        false
    }
}

/// Attach an eBPF program to an interface as an XDP hook.
pub fn xdp_attach(iface: &str, program_id: u32) -> Result<(), &'static str> {
    let mut st = STATE.lock();
    // Verify program exists and is XDP type
    let prog = st.programs.iter().find(|p| p.id == program_id)
        .ok_or("program not found")?;
    if prog.prog_type != ProgramType::Xdp {
        return Err("program is not XDP type");
    }
    // Replace or add attachment
    if let Some(att) = st.xdp_attachments.iter_mut().find(|a| a.iface == iface) {
        att.program_id = program_id;
        att.packets_processed = 0;
        att.packets_dropped = 0;
        att.packets_passed = 0;
        att.packets_tx = 0;
        att.packets_redirect = 0;
    } else {
        st.xdp_attachments.push(XdpAttachment {
            iface: String::from(iface),
            program_id,
            packets_processed: 0,
            packets_dropped: 0,
            packets_passed: 0,
            packets_tx: 0,
            packets_redirect: 0,
        });
    }
    Ok(())
}

/// Detach the XDP program from an interface.
pub fn xdp_detach(iface: &str) -> bool {
    let mut st = STATE.lock();
    if let Some(pos) = st.xdp_attachments.iter().position(|a| a.iface == iface) {
        st.xdp_attachments.remove(pos);
        true
    } else {
        false
    }
}

/// Process a packet through the XDP hook for the given interface.
/// Returns the XDP action. If no program is attached, returns Pass.
pub fn xdp_process(iface: &str, packet: &[u8]) -> XdpAction {
    let mut st = STATE.lock();
    let att = match st.xdp_attachments.iter_mut().find(|a| a.iface == iface) {
        Some(a) => a,
        None => return XdpAction::Pass,
    };
    let prog_id = att.program_id;
    // Find the program and run it
    let prog = match st.programs.iter_mut().find(|p| p.id == prog_id) {
        Some(p) => p,
        None => return XdpAction::Pass,
    };
    let result = prog.vm.run(packet);
    prog.run_count += 1;
    TOTAL_RUNS.fetch_add(1, Ordering::Relaxed);
    let action = XdpAction::from_u64(result);
    // Find the attachment again (borrowing issue workaround by index)
    // We already have a mutable borrow on st, so update via direct field access
    if let Some(a) = st.xdp_attachments.iter_mut().find(|a2| a2.iface == iface) {
        a.packets_processed += 1;
        match action {
            XdpAction::Drop | XdpAction::Aborted => { a.packets_dropped += 1; }
            XdpAction::Pass => { a.packets_passed += 1; }
            XdpAction::Tx => { a.packets_tx += 1; }
            XdpAction::Redirect => { a.packets_redirect += 1; }
        }
    }
    action
}

/// Return general eBPF subsystem information.
pub fn ebpf_info() -> String {
    let st = STATE.lock();
    let mut out = String::new();
    out.push_str("=== Extended BPF (eBPF) Subsystem ===\n");
    out.push_str(&format!("Loaded programs: {}\n", st.programs.len()));
    out.push_str(&format!("Active maps: {}\n", st.maps.len()));
    out.push_str(&format!("XDP attachments: {}\n", st.xdp_attachments.len()));
    out.push_str(&format!("Total program runs: {}\n", TOTAL_RUNS.load(Ordering::Relaxed)));
    out
}

/// Return statistics about the eBPF subsystem.
pub fn ebpf_stats() -> String {
    let st = STATE.lock();
    let mut out = String::new();
    out.push_str("=== eBPF Statistics ===\n");
    out.push_str(&format!("Total program runs: {}\n", TOTAL_RUNS.load(Ordering::Relaxed)));
    out.push_str(&format!("Programs loaded: {}\n", st.programs.len()));
    out.push_str(&format!("Maps created: {}\n", st.maps.len()));
    let total_entries: usize = st.maps.iter().map(|m| m.entries.len()).sum();
    out.push_str(&format!("Total map entries: {}\n", total_entries));
    out
}

/// List all loaded eBPF programs.
pub fn list_programs() -> String {
    let st = STATE.lock();
    let mut out = String::new();
    out.push_str("=== eBPF Programs ===\n");
    if st.programs.is_empty() {
        out.push_str("(no programs loaded)\n");
    } else {
        out.push_str("ID   NAME                 TYPE            INSNS  RUNS\n");
        out.push_str("---- -------------------- --------------- ------ ----------\n");
        for p in &st.programs {
            let ty = match p.prog_type {
                ProgramType::Xdp => "XDP            ",
                ProgramType::TcClassifier => "TC_CLASSIFIER  ",
                ProgramType::SocketFilter => "SOCKET_FILTER  ",
                ProgramType::Kprobe => "KPROBE         ",
                ProgramType::Tracepoint => "TRACEPOINT     ",
            };
            out.push_str(&format!("{:<4} {:<20} {} {:<6} {}\n",
                p.id, p.name, ty, p.vm.program.len(), p.run_count));
        }
    }
    out
}

/// List all eBPF maps.
pub fn list_maps() -> String {
    let st = STATE.lock();
    let mut out = String::new();
    out.push_str("=== eBPF Maps ===\n");
    if st.maps.is_empty() {
        out.push_str("(no maps created)\n");
    } else {
        out.push_str("ID   TYPE           KEY_SZ VAL_SZ MAX_ENT ENTRIES\n");
        out.push_str("---- -------------- ------ ------ ------- -------\n");
        for m in &st.maps {
            let ty = match m.map_type {
                MapType::HashMap => "HashMap       ",
                MapType::Array => "Array         ",
                MapType::PerCpuArray => "PerCpuArray   ",
                MapType::LruHash => "LruHash       ",
                MapType::RingBuffer => "RingBuffer    ",
            };
            out.push_str(&format!("{:<4} {} {:<6} {:<6} {:<7} {}\n",
                m.id, ty, m.key_size, m.value_size, m.max_entries, m.entries.len()));
        }
    }
    out
}

/// Return XDP attachment information.
pub fn xdp_info() -> String {
    let st = STATE.lock();
    let mut out = String::new();
    out.push_str("=== XDP (eXpress Data Path) ===\n");
    if st.xdp_attachments.is_empty() {
        out.push_str("(no XDP programs attached)\n");
    } else {
        out.push_str("IFACE      PROG_ID PROCESSED  DROPPED    PASSED     TX         REDIRECT\n");
        out.push_str("---------- ------- ---------- ---------- ---------- ---------- ----------\n");
        for a in &st.xdp_attachments {
            out.push_str(&format!("{:<10} {:<7} {:<10} {:<10} {:<10} {:<10} {}\n",
                a.iface, a.program_id, a.packets_processed,
                a.packets_dropped, a.packets_passed,
                a.packets_tx, a.packets_redirect));
        }
    }
    out
}
