/// Berkeley Packet Filter (BPF) for MerlionOS.
/// Implements the classic BPF virtual machine for packet filtering.
/// Programs are sequences of {opcode, jt, jf, k} instructions.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ── BPF instruction classes ──
const BPF_LD: u16   = 0x00;
const BPF_LDX: u16  = 0x01;
const BPF_ST: u16   = 0x02;
const BPF_STX: u16  = 0x03;
const BPF_ALU: u16  = 0x04;
const BPF_JMP: u16  = 0x05;
const BPF_RET: u16  = 0x06;
const BPF_MISC: u16 = 0x07;

// ── Size modifiers ──
const BPF_W: u16 = 0x00;   // word (32-bit)
const BPF_H: u16 = 0x08;   // halfword (16-bit)
const BPF_B: u16 = 0x10;   // byte

// ── Mode modifiers ──
const BPF_IMM: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_IND: u16 = 0x40;
const BPF_MEM: u16 = 0x60;
const BPF_LEN: u16 = 0x80;

// ── ALU operations ──
const BPF_ADD: u16 = 0x00;
const BPF_SUB: u16 = 0x10;
const BPF_MUL: u16 = 0x20;
const BPF_DIV: u16 = 0x30;
const BPF_OR: u16  = 0x40;
const BPF_AND: u16 = 0x50;
const BPF_LSH: u16 = 0x60;
const BPF_RSH: u16 = 0x70;
const BPF_NEG: u16 = 0x80;
const BPF_MOD: u16 = 0x90;
const BPF_XOR: u16 = 0xa0;

// ── Jump operations ──
const BPF_JA: u16   = 0x00;
const BPF_JEQ: u16  = 0x10;
const BPF_JGT: u16  = 0x20;
const BPF_JGE: u16  = 0x30;
const BPF_JSET: u16 = 0x40;

// ── Source modifiers ──
const BPF_K: u16 = 0x00;
const BPF_X: u16 = 0x08;

// ── MISC operations ──
const BPF_TAX: u16 = 0x00;
const BPF_TXA: u16 = 0x80;

/// Scratch memory slots.
const BPF_MEMWORDS: usize = 16;

/// Maximum program length.
const MAX_INSNS: usize = 4096;

/// A single classic BPF instruction.
#[derive(Debug, Clone, Copy)]
pub struct BpfInsn {
    pub opcode: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

impl BpfInsn {
    pub const fn new(opcode: u16, jt: u8, jf: u8, k: u32) -> Self {
        Self { opcode, jt, jf, k }
    }
}

/// The classic BPF virtual machine.
pub struct BpfVm {
    program: Vec<BpfInsn>,
    mem: [u32; BPF_MEMWORDS],
}

impl BpfVm {
    /// Create a new BPF VM with a validated program.
    pub fn new(program: Vec<BpfInsn>) -> Result<Self, &'static str> {
        if program.is_empty() {
            return Err("empty program");
        }
        if program.len() > MAX_INSNS {
            return Err("program too long");
        }
        // Validate: all jump targets in range, program ends with RET
        for (i, insn) in program.iter().enumerate() {
            let class = insn.opcode & 0x07;
            if class == BPF_JMP {
                let op = insn.opcode & 0xf0;
                if op == BPF_JA {
                    let target = i + 1 + insn.k as usize;
                    if target >= program.len() {
                        return Err("jump target out of bounds");
                    }
                } else {
                    let jt_target = i + 1 + insn.jt as usize;
                    let jf_target = i + 1 + insn.jf as usize;
                    if jt_target >= program.len() || jf_target >= program.len() {
                        return Err("conditional jump target out of bounds");
                    }
                }
            }
            if class == BPF_ST || class == BPF_STX {
                if insn.k as usize >= BPF_MEMWORDS {
                    return Err("memory index out of bounds");
                }
            }
            if class == BPF_ALU {
                let op = insn.opcode & 0xf0;
                if (op == BPF_DIV || op == BPF_MOD) && (insn.opcode & BPF_X) == 0 && insn.k == 0 {
                    return Err("division by zero in constant");
                }
            }
        }
        // Last instruction must be a RET
        let last_class = program.last().unwrap().opcode & 0x07;
        if last_class != BPF_RET {
            return Err("program does not end with RET");
        }
        Ok(Self { program, mem: [0; BPF_MEMWORDS] })
    }

    /// Execute the BPF program against a packet. Returns the verdict:
    /// 0 = reject, >0 = accept (number of bytes to capture).
    pub fn run(&mut self, packet: &[u8]) -> u32 {
        let mut a: u32 = 0;  // accumulator
        let mut x: u32 = 0;  // index register
        let mut pc: usize = 0;
        self.mem = [0; BPF_MEMWORDS];

        while pc < self.program.len() {
            let insn = self.program[pc];
            let class = insn.opcode & 0x07;
            let size = insn.opcode & 0x18;
            let mode = insn.opcode & 0xe0;
            let op = insn.opcode & 0xf0;
            let src = insn.opcode & 0x08;

            match class {
                BPF_LD => {
                    match mode {
                        BPF_IMM => { a = insn.k; }
                        BPF_ABS => {
                            a = self.load_packet(packet, insn.k as usize, size);
                        }
                        BPF_IND => {
                            a = self.load_packet(packet, (x + insn.k) as usize, size);
                        }
                        BPF_MEM => {
                            let idx = insn.k as usize;
                            if idx < BPF_MEMWORDS { a = self.mem[idx]; }
                        }
                        BPF_LEN => { a = packet.len() as u32; }
                        _ => { return 0; }
                    }
                }
                BPF_LDX => {
                    match mode {
                        BPF_IMM => { x = insn.k; }
                        BPF_MEM => {
                            let idx = insn.k as usize;
                            if idx < BPF_MEMWORDS { x = self.mem[idx]; }
                        }
                        BPF_LEN => { x = packet.len() as u32; }
                        _ => { return 0; }
                    }
                }
                BPF_ST => {
                    let idx = insn.k as usize;
                    if idx < BPF_MEMWORDS { self.mem[idx] = a; }
                }
                BPF_STX => {
                    let idx = insn.k as usize;
                    if idx < BPF_MEMWORDS { self.mem[idx] = x; }
                }
                BPF_ALU => {
                    let val = if src == BPF_X { x } else { insn.k };
                    match op {
                        BPF_ADD => { a = a.wrapping_add(val); }
                        BPF_SUB => { a = a.wrapping_sub(val); }
                        BPF_MUL => { a = a.wrapping_mul(val); }
                        BPF_DIV => { if val == 0 { return 0; } a /= val; }
                        BPF_MOD => { if val == 0 { return 0; } a %= val; }
                        BPF_AND => { a &= val; }
                        BPF_OR  => { a |= val; }
                        BPF_XOR => { a ^= val; }
                        BPF_LSH => { a = a.wrapping_shl(val); }
                        BPF_RSH => { a = a.wrapping_shr(val); }
                        BPF_NEG => { a = (!a).wrapping_add(1); }
                        _ => { return 0; }
                    }
                }
                BPF_JMP => {
                    match op {
                        BPF_JA => {
                            pc += insn.k as usize;
                        }
                        _ => {
                            let val = if src == BPF_X { x } else { insn.k };
                            let cond = match op {
                                BPF_JEQ  => a == val,
                                BPF_JGT  => a > val,
                                BPF_JGE  => a >= val,
                                BPF_JSET => (a & val) != 0,
                                _ => false,
                            };
                            if cond {
                                pc += insn.jt as usize;
                            } else {
                                pc += insn.jf as usize;
                            }
                        }
                    }
                }
                BPF_RET => {
                    let val = if (insn.opcode & 0x18) == BPF_K as u16 { insn.k } else { a };
                    return val;
                }
                BPF_MISC => {
                    match op {
                        BPF_TAX => { x = a; }
                        BPF_TXA => { a = x; }
                        _ => { return 0; }
                    }
                }
                _ => { return 0; }
            }
            pc += 1;
        }
        0
    }

    /// Load a value from the packet at the given offset.
    fn load_packet(&self, pkt: &[u8], off: usize, size: u16) -> u32 {
        match size {
            BPF_W => {
                if off + 4 > pkt.len() { return 0; }
                u32::from_be_bytes([pkt[off], pkt[off+1], pkt[off+2], pkt[off+3]])
            }
            BPF_H => {
                if off + 2 > pkt.len() { return 0; }
                u16::from_be_bytes([pkt[off], pkt[off+1]]) as u32
            }
            BPF_B => {
                if off >= pkt.len() { return 0; }
                pkt[off] as u32
            }
            _ => 0,
        }
    }
}

/// Compile a simplified tcpdump-style filter expression into BPF instructions.
/// Supports: `tcp`, `udp`, `icmp`, `port <N>`, `host <a.b.c.d>`, `tcp port <N>`.
pub fn compile_filter(expr: &str) -> Result<Vec<BpfInsn>, &'static str> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.is_empty() {
        return Err("empty filter expression");
    }

    let accept = BpfInsn::new(BPF_RET | BPF_K, 0, 0, 0xFFFFFFFF);
    let reject = BpfInsn::new(BPF_RET | BPF_K, 0, 0, 0);

    // Ethernet header: [0..5] dst MAC, [6..11] src MAC, [12..13] EtherType
    // IP header starts at offset 14 for Ethernet
    // IP protocol is at offset 14+9 = 23
    // Source IP: 14+12 = 26, Dest IP: 14+16 = 30
    // TCP/UDP src port: 14+20 = 34, dst port: 14+20+2 = 36

    match parts.as_slice() {
        ["tcp"] => {
            Ok(alloc::vec![
                BpfInsn::new(BPF_LD | BPF_B | BPF_ABS, 0, 0, 23),  // load IP proto
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 6),  // if TCP goto accept
                accept,
                reject,
            ])
        }
        ["udp"] => {
            Ok(alloc::vec![
                BpfInsn::new(BPF_LD | BPF_B | BPF_ABS, 0, 0, 23),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 17),
                accept,
                reject,
            ])
        }
        ["icmp"] => {
            Ok(alloc::vec![
                BpfInsn::new(BPF_LD | BPF_B | BPF_ABS, 0, 0, 23),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 1),
                accept,
                reject,
            ])
        }
        ["port", p] => {
            let port: u32 = p.parse().map_err(|_| "invalid port number")?;
            Ok(alloc::vec![
                // Check src port
                BpfInsn::new(BPF_LD | BPF_H | BPF_ABS, 0, 0, 34),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 2, 0, port), // match -> accept
                // Check dst port
                BpfInsn::new(BPF_LD | BPF_H | BPF_ABS, 0, 0, 36),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, port),
                accept,
                reject,
            ])
        }
        ["host", addr] => {
            let ip = parse_ipv4(addr)?;
            let ip_u32 = u32::from_be_bytes(ip);
            Ok(alloc::vec![
                // Check src IP
                BpfInsn::new(BPF_LD | BPF_W | BPF_ABS, 0, 0, 26),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 2, 0, ip_u32),
                // Check dst IP
                BpfInsn::new(BPF_LD | BPF_W | BPF_ABS, 0, 0, 30),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, ip_u32),
                accept,
                reject,
            ])
        }
        ["tcp", "port", p] => {
            let port: u32 = p.parse().map_err(|_| "invalid port number")?;
            Ok(alloc::vec![
                // Check TCP protocol
                BpfInsn::new(BPF_LD | BPF_B | BPF_ABS, 0, 0, 23),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 0, 5, 6),
                // Check src port
                BpfInsn::new(BPF_LD | BPF_H | BPF_ABS, 0, 0, 34),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 2, 0, port),
                // Check dst port
                BpfInsn::new(BPF_LD | BPF_H | BPF_ABS, 0, 0, 36),
                BpfInsn::new(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, port),
                accept,
                reject,
            ])
        }
        _ => Err("unsupported filter expression"),
    }
}

/// Parse an IPv4 address string into 4 bytes.
fn parse_ipv4(s: &str) -> Result<[u8; 4], &'static str> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return Err("invalid IPv4 address");
    }
    let mut result = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        result[i] = part.parse().map_err(|_| "invalid IPv4 octet")?;
    }
    Ok(result)
}

// ── Interface attachment ──

struct BpfAttachment {
    iface: String,
    vm: BpfVm,
    packets_matched: u64,
    packets_total: u64,
}

struct BpfState {
    attachments: Vec<BpfAttachment>,
}

impl BpfState {
    const fn new() -> Self {
        Self { attachments: Vec::new() }
    }
}

static BPF_STATE: Mutex<BpfState> = Mutex::new(BpfState::new());
static BPF_PROGRAMS_LOADED: AtomicU64 = AtomicU64::new(0);
static BPF_PACKETS_FILTERED: AtomicU64 = AtomicU64::new(0);

/// Initialise the BPF subsystem.
pub fn init() {
    let mut st = BPF_STATE.lock();
    st.attachments = Vec::new();
    crate::serial_println!("[bpf] classic BPF subsystem initialised");
}

/// Attach a BPF program to an interface.
pub fn bpf_attach(iface: &str, program: Vec<BpfInsn>) -> Result<(), &'static str> {
    let vm = BpfVm::new(program)?;
    let mut st = BPF_STATE.lock();
    // Replace existing attachment for this interface
    if let Some(pos) = st.attachments.iter().position(|a| a.iface == iface) {
        st.attachments[pos].vm = vm;
        st.attachments[pos].packets_matched = 0;
        st.attachments[pos].packets_total = 0;
    } else {
        st.attachments.push(BpfAttachment {
            iface: String::from(iface),
            vm,
            packets_matched: 0,
            packets_total: 0,
        });
    }
    BPF_PROGRAMS_LOADED.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Detach the BPF program from an interface.
pub fn bpf_detach(iface: &str) -> bool {
    let mut st = BPF_STATE.lock();
    if let Some(pos) = st.attachments.iter().position(|a| a.iface == iface) {
        st.attachments.remove(pos);
        true
    } else {
        false
    }
}

/// Run the BPF filter for a packet on a given interface.
/// Returns the verdict (0 = drop, >0 = accept N bytes).
/// If no program is attached, returns packet length (accept all).
pub fn bpf_filter(iface: &str, packet: &[u8]) -> u32 {
    let mut st = BPF_STATE.lock();
    if let Some(att) = st.attachments.iter_mut().find(|a| a.iface == iface) {
        att.packets_total += 1;
        let verdict = att.vm.run(packet);
        if verdict > 0 {
            att.packets_matched += 1;
        }
        BPF_PACKETS_FILTERED.fetch_add(1, Ordering::Relaxed);
        verdict
    } else {
        packet.len() as u32
    }
}

/// Return information about the BPF subsystem.
pub fn bpf_info() -> String {
    let st = BPF_STATE.lock();
    let mut out = String::new();
    out.push_str("=== Classic BPF Subsystem ===\n");
    out.push_str(&format!("Programs loaded (total): {}\n",
        BPF_PROGRAMS_LOADED.load(Ordering::Relaxed)));
    out.push_str(&format!("Active attachments: {}\n", st.attachments.len()));
    out.push_str(&format!("Packets filtered: {}\n",
        BPF_PACKETS_FILTERED.load(Ordering::Relaxed)));
    if !st.attachments.is_empty() {
        out.push_str("\nIFACE      INSNS  MATCHED    TOTAL\n");
        out.push_str("---------- ------ ---------- ----------\n");
        for a in &st.attachments {
            out.push_str(&format!("{:<10} {:<6} {:<10} {}\n",
                a.iface, a.vm.program.len(), a.packets_matched, a.packets_total));
        }
    }
    out
}

/// Return BPF statistics.
pub fn bpf_stats() -> String {
    let st = BPF_STATE.lock();
    let mut out = String::new();
    out.push_str("=== BPF Statistics ===\n");
    out.push_str(&format!("Total programs loaded: {}\n",
        BPF_PROGRAMS_LOADED.load(Ordering::Relaxed)));
    out.push_str(&format!("Total packets filtered: {}\n",
        BPF_PACKETS_FILTERED.load(Ordering::Relaxed)));
    let mut total_matched: u64 = 0;
    let mut total_processed: u64 = 0;
    for a in &st.attachments {
        total_matched += a.packets_matched;
        total_processed += a.packets_total;
    }
    out.push_str(&format!("Packets matched: {}\n", total_matched));
    out.push_str(&format!("Packets processed: {}\n", total_processed));
    if total_processed > 0 {
        let pct = (total_matched * 100) / total_processed;
        out.push_str(&format!("Match rate: {}%\n", pct));
    }
    out
}
