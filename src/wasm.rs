/// Minimal WebAssembly interpreter for MerlionOS (foundation for self-hosting).
///
/// Parses WASM binary format and executes a subset of instructions on a
/// stack-based VM. Supported: i32.const, i32.add/sub/mul, local.get/set,
/// call, return.

use alloc::vec::Vec;
use alloc::string::String;

/// WASM binary magic number: `\0asm`.
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
/// WASM binary format version 1.
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];
/// Section IDs.
const SECTION_TYPE: u8 = 1;
const SECTION_FUNCTION: u8 = 3;
const SECTION_EXPORT: u8 = 7;
const SECTION_CODE: u8 = 10;
/// Value type tags.
const VALTYPE_I32: u8 = 0x7F;
const VALTYPE_I64: u8 = 0x7E;
/// Function-type constructor tag.
const FUNC_TYPE_TAG: u8 = 0x60;
/// Export kind: function.
const EXPORT_FUNC: u8 = 0x00;
/// WASM opcodes (supported subset).
const OP_UNREACHABLE: u8 = 0x00;
const OP_NOP: u8 = 0x01;
const OP_END: u8 = 0x0B;
const OP_RETURN: u8 = 0x0F;
const OP_CALL: u8 = 0x10;
const OP_LOCAL_GET: u8 = 0x20;
const OP_LOCAL_SET: u8 = 0x21;
const OP_I32_CONST: u8 = 0x41;
const OP_I32_ADD: u8 = 0x6A;
const OP_I32_SUB: u8 = 0x6B;
const OP_I32_MUL: u8 = 0x6C;

/// WASM value type (minimal subset: I32 and I64).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValType { I32, I64 }

impl ValType {
    fn from_byte(b: u8) -> Result<Self, WasmError> {
        match b {
            VALTYPE_I32 => Ok(ValType::I32),
            VALTYPE_I64 => Ok(ValType::I64),
            _ => Err(WasmError::InvalidValType(b)),
        }
    }
}

/// A WASM function signature: params -> results.
#[derive(Debug, Clone)]
pub struct FuncType { pub params: Vec<ValType>, pub results: Vec<ValType> }

/// A parsed function body (locals + bytecode).
#[derive(Debug, Clone)]
pub struct FuncBody { pub locals: Vec<ValType>, pub code: Vec<u8> }

/// An exported symbol (name + function index).
#[derive(Debug, Clone)]
pub struct Export { pub name: String, pub func_idx: u32 }

/// A fully parsed WASM module (subset of sections).
#[derive(Debug, Clone)]
pub struct WasmModule {
    pub types: Vec<FuncType>,           // type section
    pub func_type_indices: Vec<u32>,    // function section
    pub bodies: Vec<FuncBody>,          // code section
    pub exports: Vec<Export>,           // export section
}

/// Errors produced by the parser or VM.
#[derive(Debug)]
pub enum WasmError {
    TooShort, BadMagic, BadVersion, UnexpectedEof,
    UnknownSection(u8), InvalidValType(u8), BadFuncTypeTag,
    UnsupportedExportKind(u8), FuncNotFound(String), FuncIndexOob(u32),
    StackUnderflow, StackOverflow, CallStackOverflow,
    Trap, UnknownOpcode(u8), LocalIndexOob(u32),
}

/// Decode an unsigned LEB128 value, returning (value, bytes_consumed).
fn decode_u32(data: &[u8]) -> Result<(u32, usize), WasmError> {
    let mut result: u32 = 0;
    let mut shift = 0u32;
    for (i, &byte) in data.iter().enumerate() {
        result |= ((byte & 0x7F) as u32) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, i + 1));
        }
        shift += 7;
        if shift >= 35 { return Err(WasmError::UnexpectedEof); }
    }
    Err(WasmError::UnexpectedEof)
}

/// Decode a signed LEB128 i32 value, returning (value, bytes_consumed).
fn decode_i32(data: &[u8]) -> Result<(i32, usize), WasmError> {
    let mut result: i32 = 0;
    let mut shift = 0u32;
    let mut last_byte = 0u8;
    for (i, &byte) in data.iter().enumerate() {
        last_byte = byte;
        result |= ((byte & 0x7F) as i32) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            if shift < 32 && (last_byte & 0x40) != 0 {
                result |= !0i32 << shift; // sign-extend
            }
            return Ok((result, i + 1));
        }
        if shift >= 35 { return Err(WasmError::UnexpectedEof); }
    }
    Err(WasmError::UnexpectedEof)
}

/// Parse a WASM binary module from raw bytes.
///
/// Decodes Type, Function, Export, and Code sections; others are skipped.
pub fn parse_module(data: &[u8]) -> Result<WasmModule, WasmError> {
    if data.len() < 8 { return Err(WasmError::TooShort); }
    if data[0..4] != WASM_MAGIC { return Err(WasmError::BadMagic); }
    if data[4..8] != WASM_VERSION { return Err(WasmError::BadVersion); }

    let mut module = WasmModule {
        types: Vec::new(), func_type_indices: Vec::new(),
        bodies: Vec::new(), exports: Vec::new(),
    };
    let mut pos = 8usize;

    while pos < data.len() {
        let section_id = data[pos];
        pos += 1;
        let (section_len, n) = decode_u32(&data[pos..])?;
        pos += n;
        let section_end = pos + section_len as usize;
        if section_end > data.len() { return Err(WasmError::UnexpectedEof); }

        match section_id {
            SECTION_TYPE => {
                let (count, n) = decode_u32(&data[pos..])?;
                pos += n;
                for _ in 0..count {
                    if data[pos] != FUNC_TYPE_TAG { return Err(WasmError::BadFuncTypeTag); }
                    pos += 1;
                    let (pc, n) = decode_u32(&data[pos..])?; pos += n;
                    let mut params = Vec::with_capacity(pc as usize);
                    for _ in 0..pc { params.push(ValType::from_byte(data[pos])?); pos += 1; }
                    let (rc, n) = decode_u32(&data[pos..])?; pos += n;
                    let mut results = Vec::with_capacity(rc as usize);
                    for _ in 0..rc { results.push(ValType::from_byte(data[pos])?); pos += 1; }
                    module.types.push(FuncType { params, results });
                }
            }
            SECTION_FUNCTION => {
                let (count, n) = decode_u32(&data[pos..])?; pos += n;
                for _ in 0..count {
                    let (ti, n) = decode_u32(&data[pos..])?; pos += n;
                    module.func_type_indices.push(ti);
                }
            }
            SECTION_EXPORT => {
                let (count, n) = decode_u32(&data[pos..])?; pos += n;
                for _ in 0..count {
                    let (nlen, n) = decode_u32(&data[pos..])?; pos += n;
                    let name_end = pos + nlen as usize;
                    let name = String::from(
                        core::str::from_utf8(&data[pos..name_end]).unwrap_or("?"),
                    );
                    pos = name_end;
                    let kind = data[pos]; pos += 1;
                    let (idx, n) = decode_u32(&data[pos..])?; pos += n;
                    if kind == EXPORT_FUNC {
                        module.exports.push(Export { name, func_idx: idx });
                    }
                }
            }
            SECTION_CODE => {
                let (count, n) = decode_u32(&data[pos..])?; pos += n;
                for _ in 0..count {
                    let (body_size, n) = decode_u32(&data[pos..])?; pos += n;
                    let body_end = pos + body_size as usize;
                    let (ldcnt, n) = decode_u32(&data[pos..])?; pos += n;
                    let mut locals = Vec::new();
                    for _ in 0..ldcnt {
                        let (lc, n) = decode_u32(&data[pos..])?; pos += n;
                        let vt = ValType::from_byte(data[pos])?; pos += 1;
                        for _ in 0..lc { locals.push(vt); }
                    }
                    let code_end = if body_end > 0 && data[body_end - 1] == OP_END {
                        body_end - 1
                    } else { body_end };
                    let code = data[pos..code_end].to_vec();
                    module.bodies.push(FuncBody { locals, code });
                    pos = body_end;
                }
            }
            _ => { pos = section_end; } // skip unknown sections
        }
    }
    Ok(module)
}

const MAX_STACK: usize = 1024;
const MAX_CALL_DEPTH: usize = 256;

/// A single frame on the call stack.
#[derive(Debug, Clone)]
struct CallFrame {
    func_idx: u32,       // function being executed
    ip: usize,           // instruction pointer within body
    locals_base: usize,  // offset into vm.locals
    locals_count: usize, // params + declared locals
    stack_base: usize,   // operand stack depth on entry
    result_count: usize, // expected return values
}

/// WebAssembly stack-based virtual machine.
#[derive(Debug)]
pub struct WasmVm {
    /// Operand value stack.
    stack: Vec<i32>,
    /// Locals storage (all frames share one flat vector).
    locals: Vec<i32>,
    /// Call stack frames.
    call_stack: Vec<CallFrame>,
}

impl WasmVm {
    /// Create a new VM instance.
    pub fn new() -> Self {
        Self {
            stack: Vec::with_capacity(128),
            locals: Vec::with_capacity(64),
            call_stack: Vec::with_capacity(16),
        }
    }

    /// Execute an exported function by name, returning its result values.
    pub fn execute(
        &mut self, module: &WasmModule, func_name: &str, args: &[i32],
    ) -> Result<Vec<i32>, WasmError> {
        let func_idx = module.exports.iter()
            .find(|e| e.name == func_name)
            .map(|e| e.func_idx)
            .ok_or_else(|| WasmError::FuncNotFound(String::from(func_name)))?;
        self.stack.clear();
        self.locals.clear();
        self.call_stack.clear();
        self.call_func(module, func_idx, args)?;
        self.run(module)
    }

    /// Set up a function call: allocate locals frame and push call-stack entry.
    fn call_func(
        &mut self, module: &WasmModule, func_idx: u32, args: &[i32],
    ) -> Result<(), WasmError> {
        if self.call_stack.len() >= MAX_CALL_DEPTH {
            return Err(WasmError::CallStackOverflow);
        }
        let idx = func_idx as usize;
        if idx >= module.bodies.len() { return Err(WasmError::FuncIndexOob(func_idx)); }
        let type_idx = module.func_type_indices[idx] as usize;
        let func_type = &module.types[type_idx];
        let body = &module.bodies[idx];
        let locals_base = self.locals.len();
        let param_count = func_type.params.len();
        let locals_count = param_count + body.locals.len();
        self.locals.resize(locals_base + locals_count, 0);
        for (i, &val) in args.iter().enumerate().take(param_count) {
            self.locals[locals_base + i] = val;
        }
        self.call_stack.push(CallFrame {
            func_idx, ip: 0, locals_base, locals_count,
            stack_base: self.stack.len(), result_count: func_type.results.len(),
        });
        Ok(())
    }

    /// Main interpreter loop: fetch-decode-execute until outermost return.
    fn run(&mut self, module: &WasmModule) -> Result<Vec<i32>, WasmError> {
        loop {
            if self.call_stack.is_empty() { break; }
            let frame = self.call_stack.last_mut().unwrap();
            let body = &module.bodies[frame.func_idx as usize];

            if frame.ip >= body.code.len() {
                let r = self.do_return(module)?;
                if self.call_stack.is_empty() { return Ok(r); }
                continue;
            }

            let opcode = body.code[frame.ip];
            frame.ip += 1;

            match opcode {
                OP_NOP => {}
                OP_UNREACHABLE => return Err(WasmError::Trap),

                OP_RETURN | OP_END => {
                    let r = self.do_return(module)?;
                    if self.call_stack.is_empty() { return Ok(r); }
                }

                OP_CALL => {
                    let frame = self.call_stack.last_mut().unwrap();
                    let body = &module.bodies[frame.func_idx as usize];
                    let (callee_idx, n) = decode_u32(&body.code[frame.ip..])?;
                    frame.ip += n;
                    let ci = callee_idx as usize;
                    if ci >= module.func_type_indices.len() {
                        return Err(WasmError::FuncIndexOob(callee_idx));
                    }
                    let cti = module.func_type_indices[ci] as usize;
                    let n_params = module.types[cti].params.len();
                    if self.stack.len() < n_params {
                        return Err(WasmError::StackUnderflow);
                    }
                    let start = self.stack.len() - n_params;
                    let args: Vec<i32> = self.stack[start..].to_vec();
                    self.stack.truncate(start);
                    self.call_func(module, callee_idx, &args)?;
                }

                OP_LOCAL_GET => {
                    let frame = self.call_stack.last_mut().unwrap();
                    let body = &module.bodies[frame.func_idx as usize];
                    let (li, n) = decode_u32(&body.code[frame.ip..])?;
                    frame.ip += n;
                    if li as usize >= frame.locals_count {
                        return Err(WasmError::LocalIndexOob(li));
                    }
                    let val = self.locals[frame.locals_base + li as usize];
                    self.push(val)?;
                }

                OP_LOCAL_SET => {
                    let frame = self.call_stack.last_mut().unwrap();
                    let body = &module.bodies[frame.func_idx as usize];
                    let (li, n) = decode_u32(&body.code[frame.ip..])?;
                    frame.ip += n;
                    if li as usize >= frame.locals_count {
                        return Err(WasmError::LocalIndexOob(li));
                    }
                    let val = self.pop()?;
                    self.locals[frame.locals_base + li as usize] = val;
                }

                OP_I32_CONST => {
                    let frame = self.call_stack.last_mut().unwrap();
                    let body = &module.bodies[frame.func_idx as usize];
                    let (val, n) = decode_i32(&body.code[frame.ip..])?;
                    frame.ip += n;
                    self.push(val)?;
                }

                OP_I32_ADD => {
                    let (b, a) = (self.pop()?, self.pop()?);
                    self.push(a.wrapping_add(b))?;
                }
                OP_I32_SUB => {
                    let (b, a) = (self.pop()?, self.pop()?);
                    self.push(a.wrapping_sub(b))?;
                }
                OP_I32_MUL => {
                    let (b, a) = (self.pop()?, self.pop()?);
                    self.push(a.wrapping_mul(b))?;
                }

                _ => return Err(WasmError::UnknownOpcode(opcode)),
            }
        }
        Ok(self.stack.clone())
    }

    /// Pop the current frame, collect results, restore caller state.
    fn do_return(&mut self, module: &WasmModule) -> Result<Vec<i32>, WasmError> {
        let frame = self.call_stack.pop().unwrap();
        let ti = module.func_type_indices[frame.func_idx as usize] as usize;
        let rc = module.types[ti].results.len();
        if self.stack.len() < rc { return Err(WasmError::StackUnderflow); }
        let start = self.stack.len() - rc;
        let results: Vec<i32> = self.stack[start..].to_vec();
        self.stack.truncate(frame.stack_base);
        for &v in &results { self.push(v)?; }
        self.locals.truncate(frame.locals_base);
        Ok(results)
    }

    /// Push a value onto the operand stack.
    fn push(&mut self, val: i32) -> Result<(), WasmError> {
        if self.stack.len() >= MAX_STACK { return Err(WasmError::StackOverflow); }
        self.stack.push(val);
        Ok(())
    }

    /// Pop a value from the operand stack.
    fn pop(&mut self) -> Result<i32, WasmError> {
        self.stack.pop().ok_or(WasmError::StackUnderflow)
    }
}
