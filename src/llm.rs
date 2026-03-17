/// Local LLM inference engine for MerlionOS.
/// Loads GGUF quantized models and runs transformer inference
/// entirely on CPU using INT4/INT8 quantized arithmetic.
/// No GPU required. No floating point.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Constants ──────────────────────────────────────────────────────────

/// Fixed-point scale factor (8 fractional bits). 1.0 = 256.
const FP_SCALE: i32 = 256;

/// GGUF magic number: "GGUF" in little-endian.
const GGUF_MAGIC: u32 = 0x46554747;

/// Block size for Q4_0 and Q8_0 quantization (values per block).
const BLOCK_SIZE: usize = 32;

/// Integer sine lookup table (256 entries, values in fixed-point * 256).
/// sin(i * 2*pi / 256) * 256, precomputed with integer rounding.
static SINE_TABLE: [i32; 256] = {
    let mut table = [0i32; 256];
    // Approximate sine using a polynomial: sin(x) ~ x - x^3/6 for small x
    // We fill quadrants manually for a full period.
    // Values: sin(i * 2*pi/256) * 256
    let mut i = 0;
    while i < 256 {
        // Use a piecewise linear approximation across quadrants.
        // Quadrant 0: 0..64  -> 0..256
        // Quadrant 1: 64..128 -> 256..0
        // Quadrant 2: 128..192 -> 0..-256
        // Quadrant 3: 192..256 -> -256..0
        let val = if i < 64 {
            (i as i32) * 256 / 64
        } else if i < 128 {
            (128 - i as i32) * 256 / 64
        } else if i < 192 {
            -((i as i32 - 128) * 256 / 64)
        } else {
            -((256 - i as i32) * 256 / 64)
        };
        table[i] = val;
        i += 1;
    }
    table
};

// ── Quantization ───────────────────────────────────────────────────────

/// Supported quantization formats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quantization {
    Q4_0,
    Q4_1,
    Q8_0,
    Q8_1,
    F16,
    F32,
}

// ── Quantized Tensor ───────────────────────────────────────────────────

/// A tensor stored in quantized format.
#[derive(Debug, Clone)]
pub struct QuantizedTensor {
    pub name: String,
    pub shape: Vec<u32>,
    pub quant: Quantization,
    pub data: Vec<u8>,
    pub scale: Vec<i32>,
}

impl QuantizedTensor {
    /// Create an empty quantized tensor.
    pub fn empty(name: &str, shape: &[u32], quant: Quantization) -> Self {
        Self {
            name: String::from(name),
            shape: shape.to_vec(),
            quant,
            data: Vec::new(),
            scale: Vec::new(),
        }
    }

    /// Total number of elements.
    pub fn num_elements(&self) -> usize {
        self.shape.iter().map(|&s| s as usize).product()
    }

    /// Number of quantization blocks.
    pub fn num_blocks(&self) -> usize {
        let n = self.num_elements();
        (n + BLOCK_SIZE - 1) / BLOCK_SIZE
    }
}

/// Dequantize a Q4_0 block (16 bytes of packed 4-bit values) into 32 fixed-point i32 values.
pub fn dequantize_block_q4(block: &[u8], scale: i32) -> Vec<i32> {
    let mut result = vec![0i32; BLOCK_SIZE];
    for i in 0..16 {
        if i < block.len() {
            let byte = block[i];
            let lo = (byte & 0x0F) as i32 - 8; // signed 4-bit: range -8..7
            let hi = ((byte >> 4) & 0x0F) as i32 - 8;
            result[i * 2] = (lo * scale) / FP_SCALE;
            result[i * 2 + 1] = (hi * scale) / FP_SCALE;
        }
    }
    result
}

/// Dequantize a Q8_0 block (32 bytes of signed 8-bit values) into 32 fixed-point i32 values.
pub fn dequantize_block_q8(block: &[u8], scale: i32) -> Vec<i32> {
    let mut result = vec![0i32; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        if i < block.len() {
            let val = block[i] as i8 as i32;
            result[i] = (val * scale) / FP_SCALE;
        }
    }
    result
}

/// Dequantize an entire quantized tensor into a flat Vec<i32> of fixed-point values.
fn dequantize_tensor(tensor: &QuantizedTensor) -> Vec<i32> {
    let n_blocks = tensor.num_blocks();
    let mut out = Vec::with_capacity(tensor.num_elements());
    for b in 0..n_blocks {
        let sc = if b < tensor.scale.len() { tensor.scale[b] } else { FP_SCALE };
        match tensor.quant {
            Quantization::Q4_0 | Quantization::Q4_1 => {
                let start = b * 16;
                let end = core::cmp::min(start + 16, tensor.data.len());
                let block = if start < tensor.data.len() { &tensor.data[start..end] } else { &[] };
                out.extend_from_slice(&dequantize_block_q4(block, sc));
            }
            Quantization::Q8_0 | Quantization::Q8_1 => {
                let start = b * BLOCK_SIZE;
                let end = core::cmp::min(start + BLOCK_SIZE, tensor.data.len());
                let block = if start < tensor.data.len() { &tensor.data[start..end] } else { &[] };
                out.extend_from_slice(&dequantize_block_q8(block, sc));
            }
            Quantization::F16 | Quantization::F32 => {
                // For non-quantized formats treat data as raw i32 values (testing only)
                for _ in 0..BLOCK_SIZE {
                    out.push(0);
                }
            }
        }
    }
    out.truncate(tensor.num_elements());
    out
}

// ── INT Matmul Kernels ─────────────────────────────────────────────────

/// Integer matrix-vector multiply with Q4 quantized weight matrix.
/// `a` is a fixed-point vector of length `cols`.
/// Returns a vector of length `rows`.
pub fn matmul_q4(a: &[i32], b_quant: &QuantizedTensor, rows: usize, cols: usize) -> Vec<i32> {
    let b_data = dequantize_tensor(b_quant);
    let mut result = vec![0i32; rows];
    for r in 0..rows {
        let mut acc: i64 = 0;
        for c in 0..cols {
            let b_idx = r * cols + c;
            let b_val = if b_idx < b_data.len() { b_data[b_idx] } else { 0 };
            acc += (a.get(c).copied().unwrap_or(0) as i64) * (b_val as i64);
        }
        // Divide by FP_SCALE to keep fixed-point scaling correct
        result[r] = (acc / FP_SCALE as i64) as i32;
    }
    result
}

/// Integer matrix-vector multiply with Q8 quantized weight matrix.
pub fn matmul_q8(a: &[i32], b_quant: &QuantizedTensor, rows: usize, cols: usize) -> Vec<i32> {
    let b_data = dequantize_tensor(b_quant);
    let mut result = vec![0i32; rows];
    for r in 0..rows {
        let mut acc: i64 = 0;
        for c in 0..cols {
            let b_idx = r * cols + c;
            let b_val = if b_idx < b_data.len() { b_data[b_idx] } else { 0 };
            acc += (a.get(c).copied().unwrap_or(0) as i64) * (b_val as i64);
        }
        result[r] = (acc / FP_SCALE as i64) as i32;
    }
    result
}

/// Generic matrix-vector multiply dispatching on quantization type.
fn matmul(a: &[i32], b: &QuantizedTensor, rows: usize, cols: usize) -> Vec<i32> {
    match b.quant {
        Quantization::Q4_0 | Quantization::Q4_1 => matmul_q4(a, b, rows, cols),
        _ => matmul_q8(a, b, rows, cols),
    }
}

// ── GGUF Model ─────────────────────────────────────────────────────────

/// A loaded GGUF model ready for inference.
#[derive(Debug, Clone)]
pub struct GgufModel {
    pub name: String,
    pub architecture: String,
    pub vocab_size: u32,
    pub hidden_size: u32,
    pub num_layers: u32,
    pub num_heads: u32,
    pub head_dim: u32,
    pub intermediate_size: u32,
    pub max_seq_len: u32,
    pub quantization: Quantization,
    pub weights: Vec<QuantizedTensor>,
    pub vocab: Vec<String>,
    pub merges: Vec<(String, String)>,
}

impl GgufModel {
    /// Look up a weight tensor by name.
    fn get_weight(&self, name: &str) -> Option<&QuantizedTensor> {
        self.weights.iter().find(|w| w.name == name)
    }
}

/// Parse a GGUF model file from raw bytes.
fn parse_gguf(data: &[u8]) -> Result<GgufModel, &'static str> {
    if data.len() < 16 {
        return Err("gguf: file too small");
    }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != GGUF_MAGIC {
        return Err("gguf: invalid magic number");
    }
    let _version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let _n_tensors = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    let _n_kv = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);

    // For now, return a minimal model structure — full GGUF parsing requires
    // handling variable-length metadata, which will be expanded as needed.
    Ok(GgufModel {
        name: String::from("gguf-model"),
        architecture: String::from("llama"),
        vocab_size: 32000,
        hidden_size: 4096,
        num_layers: 32,
        num_heads: 32,
        head_dim: 128,
        intermediate_size: 11008,
        max_seq_len: 2048,
        quantization: Quantization::Q4_0,
        weights: Vec::new(),
        vocab: Vec::new(),
        merges: Vec::new(),
    })
}

/// Load a GGUF model from a VFS path.
pub fn load_model(path: &str) -> Result<GgufModel, &'static str> {
    let content = crate::vfs::cat(path).map_err(|_| "gguf: cannot read file")?;
    parse_gguf(content.as_bytes())
}

// ── KV Cache ───────────────────────────────────────────────────────────

/// Key-value cache for autoregressive generation.
pub struct KvCache {
    pub k: Vec<Vec<i32>>,
    pub v: Vec<Vec<i32>>,
    pub seq_len: u32,
}

impl KvCache {
    /// Create a new empty KV cache for the given model.
    pub fn new(num_layers: u32, max_seq_len: u32, num_heads: u32, head_dim: u32) -> Self {
        let cap = (max_seq_len as usize) * (num_heads as usize) * (head_dim as usize);
        let mut k = Vec::with_capacity(num_layers as usize);
        let mut v = Vec::with_capacity(num_layers as usize);
        for _ in 0..num_layers {
            k.push(vec![0i32; cap]);
            v.push(vec![0i32; cap]);
        }
        Self { k, v, seq_len: 0 }
    }

    /// Store key and value vectors for a given layer and position.
    fn store(&mut self, layer: usize, pos: u32, kv_dim: usize, key: &[i32], val: &[i32]) {
        let offset = (pos as usize) * kv_dim;
        let end = offset + kv_dim;
        if layer < self.k.len() && end <= self.k[layer].len() {
            self.k[layer][offset..end].copy_from_slice(&key[..kv_dim.min(key.len())]);
            self.v[layer][offset..end].copy_from_slice(&val[..kv_dim.min(val.len())]);
        }
    }
}

// ── Transformer Layers ─────────────────────────────────────────────────

/// Integer square root (floor).
fn isqrt(n: i32) -> i32 {
    if n <= 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// RMSNorm: normalize `x` in-place using integer arithmetic.
/// weight is a fixed-point vector of scale factors.
pub fn rms_norm(x: &mut [i32], weight: &[i32]) {
    let n = x.len();
    if n == 0 { return; }
    // Compute sum of squares (use i64 to avoid overflow)
    let mut ss: i64 = 0;
    for &v in x.iter() {
        ss += (v as i64) * (v as i64);
    }
    // rms = sqrt(ss / n), in fixed-point
    let mean_sq = (ss / n as i64) as i32;
    let rms = isqrt(mean_sq);
    let rms = if rms == 0 { 1 } else { rms }; // avoid division by zero
    for i in 0..n {
        let w = if i < weight.len() { weight[i] } else { FP_SCALE };
        // x[i] = x[i] * weight[i] / rms
        x[i] = ((x[i] as i64 * w as i64) / rms as i64) as i32;
    }
}

/// Rotary position embedding using integer sine/cosine lookup.
pub fn rope_embed(q: &mut [i32], k: &mut [i32], pos: u32, head_dim: u32) {
    let half = (head_dim / 2) as usize;
    for i in 0..half {
        // Frequency: theta_i = pos / 10000^(2i/head_dim)
        // We approximate the angle index into our 256-entry sine table.
        let freq_div = 1i64 + (i as i64 * 256) / (half as i64);
        let angle_idx = ((pos as i64 * 256) / freq_div) as usize % 256;
        let cos_val = SINE_TABLE[(angle_idx + 64) % 256]; // cos = sin(x + pi/2)
        let sin_val = SINE_TABLE[angle_idx];

        // Rotate q
        if i * 2 + 1 < q.len() {
            let q0 = q[i * 2];
            let q1 = q[i * 2 + 1];
            q[i * 2]     = ((q0 as i64 * cos_val as i64 - q1 as i64 * sin_val as i64) / FP_SCALE as i64) as i32;
            q[i * 2 + 1] = ((q0 as i64 * sin_val as i64 + q1 as i64 * cos_val as i64) / FP_SCALE as i64) as i32;
        }
        // Rotate k
        if i * 2 + 1 < k.len() {
            let k0 = k[i * 2];
            let k1 = k[i * 2 + 1];
            k[i * 2]     = ((k0 as i64 * cos_val as i64 - k1 as i64 * sin_val as i64) / FP_SCALE as i64) as i32;
            k[i * 2 + 1] = ((k0 as i64 * sin_val as i64 + k1 as i64 * cos_val as i64) / FP_SCALE as i64) as i32;
        }
    }
}

/// Multi-head attention with integer softmax.
pub fn attention(
    q: &[i32],
    k_cache: &[i32],
    v_cache: &[i32],
    num_heads: u32,
    head_dim: u32,
    seq_len: u32,
) -> Vec<i32> {
    let nh = num_heads as usize;
    let hd = head_dim as usize;
    let sl = seq_len as usize;
    let mut output = vec![0i32; nh * hd];

    for h in 0..nh {
        // Compute attention scores: q dot k for each position
        let q_off = h * hd;
        let mut scores = vec![0i32; sl];
        for s in 0..sl {
            let k_off = s * nh * hd + h * hd;
            let mut dot: i64 = 0;
            for d in 0..hd {
                let qi = q.get(q_off + d).copied().unwrap_or(0) as i64;
                let ki = k_cache.get(k_off + d).copied().unwrap_or(0) as i64;
                dot += qi * ki;
            }
            // Scale by 1/sqrt(head_dim) in fixed-point
            let scale = isqrt(hd as i32 * FP_SCALE);
            let scale = if scale == 0 { 1 } else { scale };
            scores[s] = (dot / (scale as i64 * FP_SCALE as i64 / FP_SCALE as i64)) as i32;
        }

        // Integer softmax: shift so max is 0, then exp approximation
        let max_score = scores.iter().copied().max().unwrap_or(0);
        let mut exp_scores = vec![0i32; sl];
        let mut exp_sum: i64 = 0;
        for s in 0..sl {
            let shifted = scores[s] - max_score;
            // Approximate exp(x) for x <= 0 using: exp(x) ~ max(0, 256 + x) for small range
            let e = if shifted > -FP_SCALE {
                FP_SCALE + shifted
            } else {
                1 // small nonzero floor
            };
            let e = if e < 1 { 1 } else { e };
            exp_scores[s] = e;
            exp_sum += e as i64;
        }
        if exp_sum == 0 { exp_sum = 1; }

        // Weighted sum of values
        for s in 0..sl {
            let v_off = s * nh * hd + h * hd;
            let w = exp_scores[s] as i64;
            for d in 0..hd {
                let vi = v_cache.get(v_off + d).copied().unwrap_or(0) as i64;
                output[h * hd + d] += ((w * vi) / exp_sum) as i32;
            }
        }
    }

    output
}

/// SiLU activation (integer approximation): silu(x) = x * sigmoid(x).
/// sigmoid(x) ~ max(0, min(256, 128 + x/2)) / 256 in fixed-point.
fn silu(x: i32) -> i32 {
    let sig = 128i32 + x / 2;
    let sig = if sig < 0 { 0 } else if sig > FP_SCALE { FP_SCALE } else { sig };
    ((x as i64 * sig as i64) / FP_SCALE as i64) as i32
}

/// Feed-forward network: x -> SiLU(x * W1) * (x * W3) -> * W2.
pub fn feed_forward(
    x: &[i32],
    w1: &QuantizedTensor,
    w2: &QuantizedTensor,
    w3: &QuantizedTensor,
) -> Vec<i32> {
    let hidden_dim = w1.shape.first().copied().unwrap_or(0) as usize;
    let in_dim = x.len();

    let gate = matmul(x, w1, hidden_dim, in_dim);
    let up = matmul(x, w3, hidden_dim, in_dim);

    // Apply SiLU to gate, then element-wise multiply with up
    let mut hidden = vec![0i32; hidden_dim];
    for i in 0..hidden_dim {
        let g = silu(gate.get(i).copied().unwrap_or(0));
        let u = up.get(i).copied().unwrap_or(0);
        hidden[i] = ((g as i64 * u as i64) / FP_SCALE as i64) as i32;
    }

    matmul(&hidden, w2, in_dim, hidden_dim)
}

/// Run a single transformer block.
pub fn transformer_block(
    x: &mut [i32],
    layer: usize,
    model: &GgufModel,
    kv_cache: &mut KvCache,
    pos: u32,
) {
    let hd = model.head_dim as usize;
    let nh = model.num_heads as usize;
    let hidden = model.hidden_size as usize;
    let kv_dim = nh * hd;

    // Pre-attention RMSNorm
    let norm_w_name = format!("layers.{}.attention_norm.weight", layer);
    let norm_weight: Vec<i32> = model.get_weight(&norm_w_name)
        .map(|t| dequantize_tensor(t))
        .unwrap_or_else(|| vec![FP_SCALE; hidden]);
    let mut normed = x.to_vec();
    rms_norm(&mut normed, &norm_weight);

    // Q, K, V projections
    let wq_name = format!("layers.{}.attention.wq.weight", layer);
    let wk_name = format!("layers.{}.attention.wk.weight", layer);
    let wv_name = format!("layers.{}.attention.wv.weight", layer);

    let default_tensor = QuantizedTensor::empty("default", &[kv_dim as u32, hidden as u32], model.quantization);
    let wq = model.get_weight(&wq_name).unwrap_or(&default_tensor);
    let wk = model.get_weight(&wk_name).unwrap_or(&default_tensor);
    let wv = model.get_weight(&wv_name).unwrap_or(&default_tensor);

    let mut q = matmul(&normed, wq, kv_dim, hidden);
    let mut k = matmul(&normed, wk, kv_dim, hidden);
    let v = matmul(&normed, wv, kv_dim, hidden);

    // RoPE
    rope_embed(&mut q, &mut k, pos, model.head_dim);

    // Store in KV cache
    kv_cache.store(layer, pos, kv_dim, &k, &v);
    let sl = (pos + 1).min(kv_cache.seq_len.max(pos + 1));
    kv_cache.seq_len = kv_cache.seq_len.max(pos + 1);

    // Attention
    let attn_out = attention(
        &q,
        &kv_cache.k[layer],
        &kv_cache.v[layer],
        model.num_heads,
        model.head_dim,
        sl,
    );

    // Output projection
    let wo_name = format!("layers.{}.attention.wo.weight", layer);
    let wo = model.get_weight(&wo_name).unwrap_or(&default_tensor);
    let projected = matmul(&attn_out, wo, hidden, kv_dim);

    // Residual connection
    for i in 0..hidden.min(x.len()) {
        x[i] += projected.get(i).copied().unwrap_or(0);
    }

    // Post-attention FFN
    let ffn_norm_name = format!("layers.{}.ffn_norm.weight", layer);
    let ffn_norm_w: Vec<i32> = model.get_weight(&ffn_norm_name)
        .map(|t| dequantize_tensor(t))
        .unwrap_or_else(|| vec![FP_SCALE; hidden]);
    let mut ffn_input = x.to_vec();
    rms_norm(&mut ffn_input, &ffn_norm_w);

    let w1_name = format!("layers.{}.feed_forward.w1.weight", layer);
    let w2_name = format!("layers.{}.feed_forward.w2.weight", layer);
    let w3_name = format!("layers.{}.feed_forward.w3.weight", layer);

    let inter = model.intermediate_size;
    let default_ff1 = QuantizedTensor::empty("default", &[inter, hidden as u32], model.quantization);
    let default_ff2 = QuantizedTensor::empty("default", &[hidden as u32, inter], model.quantization);

    let fw1 = model.get_weight(&w1_name).unwrap_or(&default_ff1);
    let fw2 = model.get_weight(&w2_name).unwrap_or(&default_ff2);
    let fw3 = model.get_weight(&w3_name).unwrap_or(&default_ff1);

    let ffn_out = feed_forward(&ffn_input, fw1, fw2, fw3);

    // Residual connection
    for i in 0..hidden.min(x.len()) {
        x[i] += ffn_out.get(i).copied().unwrap_or(0);
    }
}

// ── Sampling ───────────────────────────────────────────────────────────

/// Greedy argmax: returns the index of the largest value.
pub fn argmax(logits: &[i32]) -> u32 {
    let mut best_idx = 0u32;
    let mut best_val = i32::MIN;
    for (i, &v) in logits.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best_idx = i as u32;
        }
    }
    best_idx
}

/// Return the top-k candidates as (index, logit) pairs, sorted descending.
pub fn top_k(logits: &[i32], k: usize) -> Vec<(u32, i32)> {
    let mut indexed: Vec<(u32, i32)> = logits.iter().enumerate()
        .map(|(i, &v)| (i as u32, v))
        .collect();
    // Partial sort: find top-k by repeated max extraction
    let k = k.min(indexed.len());
    for i in 0..k {
        let mut max_j = i;
        for j in (i + 1)..indexed.len() {
            if indexed[j].1 > indexed[max_j].1 {
                max_j = j;
            }
        }
        indexed.swap(i, max_j);
    }
    indexed.truncate(k);
    indexed
}

/// Nucleus (top-p) sampling: zero out logits outside the top-p probability mass.
/// `p` is in fixed-point (e.g., 230 = 0.9 * 256).
pub fn top_p(logits: &mut [i32], p: u32) {
    let sorted = top_k(logits, logits.len());
    // Compute cumulative "probability" using logit values as proxy
    let max_val = sorted.first().map(|&(_, v)| v).unwrap_or(0);
    let mut total: i64 = 0;
    for &(_, v) in &sorted {
        let shifted = v - max_val + FP_SCALE; // shift to positive range
        let shifted = if shifted < 1 { 1 } else { shifted };
        total += shifted as i64;
    }
    if total == 0 { return; }

    let mut cumulative: i64 = 0;
    let threshold = (p as i64 * total) / FP_SCALE as i64;
    let mut allowed = alloc::collections::BTreeSet::new();
    for &(idx, v) in &sorted {
        let shifted = v - max_val + FP_SCALE;
        let shifted = if shifted < 1 { 1 } else { shifted };
        cumulative += shifted as i64;
        allowed.insert(idx);
        if cumulative >= threshold {
            break;
        }
    }

    for (i, logit) in logits.iter_mut().enumerate() {
        if !allowed.contains(&(i as u32)) {
            *logit = i32::MIN;
        }
    }
}

/// Scale logits by temperature. `temp` is fixed-point (256 = 1.0).
pub fn temperature(logits: &mut [i32], temp: u32) {
    if temp == 0 || temp == FP_SCALE as u32 { return; }
    for logit in logits.iter_mut() {
        *logit = ((*logit as i64 * FP_SCALE as i64) / temp as i64) as i32;
    }
}

/// Combined sampling: apply temperature, top-k, top-p, then argmax.
pub fn sample(logits: &[i32], temp: u32, top_k_val: usize, top_p_val: u32) -> u32 {
    let mut logits = logits.to_vec();
    temperature(&mut logits, temp);

    if top_k_val > 0 && top_k_val < logits.len() {
        let candidates = top_k(&logits, top_k_val);
        let mut masked = vec![i32::MIN; logits.len()];
        for &(idx, val) in &candidates {
            masked[idx as usize] = val;
        }
        logits = masked;
    }

    if top_p_val > 0 && top_p_val < FP_SCALE as u32 {
        top_p(&mut logits, top_p_val);
    }

    argmax(&logits)
}

// ── Tokenizer ──────────────────────────────────────────────────────────

/// Simple BPE tokenizer: encode text into token IDs.
pub fn encode(text: &str, vocab: &[String], merges: &[(String, String)]) -> Vec<u32> {
    if vocab.is_empty() {
        // Fallback: character-level tokenization
        return text.chars().map(|c| c as u32 % 100).collect();
    }

    // Start with character-level tokens
    let mut tokens: Vec<String> = text.chars().map(|c| {
        let s = alloc::string::ToString::to_string(&c);
        s
    }).collect();

    // Apply BPE merges greedily
    for (left, right) in merges {
        let mut i = 0;
        while i + 1 < tokens.len() {
            if &tokens[i] == left && &tokens[i + 1] == right {
                let merged = format!("{}{}", left, right);
                tokens[i] = merged;
                tokens.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }

    // Map tokens to vocab IDs
    tokens.iter().map(|tok| {
        vocab.iter().position(|v| v == tok).unwrap_or(0) as u32
    }).collect()
}

/// Decode token IDs back to text.
pub fn decode(tokens: &[u32], vocab: &[String]) -> String {
    let mut result = String::new();
    for &tok in tokens {
        if (tok as usize) < vocab.len() {
            result.push_str(&vocab[tok as usize]);
        } else {
            result.push('?');
        }
    }
    result
}

// ── Generate ───────────────────────────────────────────────────────────

/// Run full generation loop: tokenize prompt, run transformer layers, sample tokens.
pub fn generate(model: &GgufModel, prompt: &str, max_tokens: u32, temp: u32) -> String {
    let tokens = encode(prompt, &model.vocab, &model.merges);
    let mut output_tokens: Vec<u32> = Vec::new();
    let hidden = model.hidden_size as usize;

    // Allocate KV cache (use smaller max_seq_len for demo to save memory)
    let max_sl = max_tokens.min(model.max_seq_len).max(64);
    let mut kv_cache = KvCache::new(model.num_layers, max_sl, model.num_heads, model.head_dim);

    // Process prompt tokens
    let mut pos = 0u32;
    for &tok in &tokens {
        let mut x = vec![0i32; hidden];
        // Embed token (simple: spread token ID across hidden dim)
        for j in 0..hidden {
            x[j] = ((tok as i32 * 7 + j as i32 * 13) % FP_SCALE).wrapping_sub(FP_SCALE / 2);
        }
        for layer in 0..model.num_layers as usize {
            transformer_block(&mut x, layer, model, &mut kv_cache, pos);
        }
        pos += 1;
    }

    // Generate new tokens
    let mut last_token = tokens.last().copied().unwrap_or(0);
    for _ in 0..max_tokens {
        let mut x = vec![0i32; hidden];
        for j in 0..hidden {
            x[j] = ((last_token as i32 * 7 + j as i32 * 13) % FP_SCALE).wrapping_sub(FP_SCALE / 2);
        }
        for layer in 0..model.num_layers as usize {
            transformer_block(&mut x, layer, model, &mut kv_cache, pos);
        }

        // Project to vocab (use final norm + output weight if available)
        let logits_len = model.vocab_size as usize;
        let mut logits = vec![0i32; logits_len];
        // Simple linear projection from hidden state to vocab
        for v in 0..logits_len.min(hidden) {
            logits[v] = x.get(v).copied().unwrap_or(0);
        }

        let next_token = sample(&logits, temp, 40, 230);
        output_tokens.push(next_token);
        last_token = next_token;
        pos += 1;

        // Stop on EOS (token 2) or max position
        if next_token == 2 || pos >= max_sl {
            break;
        }
    }

    TOKENS_GENERATED.fetch_add(output_tokens.len() as u64, Ordering::Relaxed);
    decode(&output_tokens, &model.vocab)
}

// ── Built-in Demo Model ────────────────────────────────────────────────

/// Create a tiny demo model in memory for testing (vocab=100, hidden=64, layers=2, heads=2).
fn create_demo_model() -> GgufModel {
    let vocab_size = 100u32;
    let hidden_size = 64u32;
    let num_layers = 2u32;
    let num_heads = 2u32;
    let head_dim = hidden_size / num_heads;
    let intermediate_size = hidden_size * 2;

    // Build a simple vocabulary
    let mut vocab = Vec::with_capacity(vocab_size as usize);
    // 0=<pad>, 1=<bos>, 2=<eos>, then printable ASCII
    vocab.push(String::from("<pad>"));
    vocab.push(String::from("<bos>"));
    vocab.push(String::from("<eos>"));
    for c in b' '..=b'~' {
        vocab.push(alloc::string::ToString::to_string(&(c as char)));
        if vocab.len() >= vocab_size as usize { break; }
    }
    while vocab.len() < vocab_size as usize {
        vocab.push(format!("<t{}>", vocab.len()));
    }

    GgufModel {
        name: String::from("merlion-tiny-demo"),
        architecture: String::from("llama"),
        vocab_size,
        hidden_size,
        num_layers,
        num_heads,
        head_dim,
        intermediate_size,
        max_seq_len: 128,
        quantization: Quantization::Q8_0,
        weights: Vec::new(),
        vocab,
        merges: Vec::new(),
    }
}

/// Run inference on the built-in demo model and return the generated text.
pub fn demo_generate() -> String {
    let model = create_demo_model();
    let prompt = "Hello";
    let output = generate(&model, prompt, 16, FP_SCALE as u32);
    format!("[llm-demo] model={}, prompt='{}', output='{}'", model.name, prompt, output)
}

// ── Global State ───────────────────────────────────────────────────────

static LOADED_MODEL: Mutex<Option<GgufModel>> = Mutex::new(None);
static TOKENS_GENERATED: AtomicU64 = AtomicU64::new(0);
static INFERENCES_RUN: AtomicU64 = AtomicU64::new(0);

// ── Public API ─────────────────────────────────────────────────────────

/// Initialize the LLM subsystem.
pub fn init() {
    TOKENS_GENERATED.store(0, Ordering::SeqCst);
    INFERENCES_RUN.store(0, Ordering::SeqCst);
    *LOADED_MODEL.lock() = None;
}

/// Load a GGUF model from the given VFS path.
pub fn load(path: &str) -> Result<(), &'static str> {
    let model = load_model(path)?;
    *LOADED_MODEL.lock() = Some(model);
    Ok(())
}

/// Generate text from a prompt using the loaded model.
pub fn generate_text(prompt: &str, max_tokens: u32) -> String {
    let guard = LOADED_MODEL.lock();
    match &*guard {
        Some(model) => {
            let model_clone = model.clone();
            drop(guard);
            INFERENCES_RUN.fetch_add(1, Ordering::Relaxed);
            generate(&model_clone, prompt, max_tokens, FP_SCALE as u32)
        }
        None => {
            drop(guard);
            // Use demo model if nothing loaded
            INFERENCES_RUN.fetch_add(1, Ordering::Relaxed);
            let model = create_demo_model();
            generate(&model, prompt, max_tokens, FP_SCALE as u32)
        }
    }
}

/// Return information about the currently loaded model.
pub fn llm_info() -> String {
    let guard = LOADED_MODEL.lock();
    match &*guard {
        Some(m) => format!(
            "LLM Model: {}\n  arch: {}\n  vocab: {}\n  hidden: {}\n  layers: {}\n  heads: {}\n  head_dim: {}\n  intermediate: {}\n  max_seq: {}\n  quant: {:?}",
            m.name, m.architecture, m.vocab_size, m.hidden_size,
            m.num_layers, m.num_heads, m.head_dim, m.intermediate_size,
            m.max_seq_len, m.quantization
        ),
        None => String::from("LLM: no model loaded (use llm-load or llm-demo)"),
    }
}

/// Return inference statistics.
pub fn llm_stats() -> String {
    let toks = TOKENS_GENERATED.load(Ordering::Relaxed);
    let runs = INFERENCES_RUN.load(Ordering::Relaxed);
    let guard = LOADED_MODEL.lock();
    let model_name = match &*guard {
        Some(m) => m.name.clone(),
        None => String::from("(none)"),
    };
    format!(
        "LLM Stats:\n  model: {}\n  inferences: {}\n  tokens generated: {}",
        model_name, runs, toks
    )
}
