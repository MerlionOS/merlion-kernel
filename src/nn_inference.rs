/// Quantized neural network inference engine for MerlionOS.
///
/// Provides a simple sequential neural network framework using **integer-only**
/// fixed-point arithmetic (i32 with scale factor 256).  No floating point is
/// used anywhere — all weights, activations, and intermediate results are
/// represented as `i32` values scaled by [`SCALE`].
///
/// Supports fully-connected (dense) layers, ReLU, sigmoid (piecewise linear
/// approximation), and softmax (integer approximation).  Models are stored in
/// a global registry protected by a spinlock and can be loaded from a simple
/// binary format stored in the VFS.
///
/// Thread-safe via `spin::Mutex`; suitable for `#![no_std]` kernel use.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::vfs;

/// Fixed-point scale factor.  Values are stored as `real_value * SCALE`.
/// For example, 1.0 is represented as 256, 0.5 as 128, etc.
const SCALE: i32 = 256;

// ── Tensor ───────────────────────────────────────────────────────────

/// A multi-dimensional tensor of fixed-point i32 values.
///
/// All values are stored multiplied by [`SCALE`] (256).  For example, the
/// real value 1.0 is stored as 256, and -0.5 as -128.
#[derive(Debug, Clone)]
pub struct Tensor {
    /// Fixed-point data, stored in row-major order.
    pub data: Vec<i32>,
    /// Shape of the tensor, e.g. `[rows, cols]` for a 2D matrix.
    pub shape: Vec<usize>,
}

impl Tensor {
    /// Create a new tensor with the given shape, filled with zeros.
    pub fn zeros(shape: &[usize]) -> Self {
        let len: usize = shape.iter().product();
        Self {
            data: vec![0i32; len],
            shape: shape.to_vec(),
        }
    }

    /// Create a 2D tensor from a flat slice of fixed-point values.
    pub fn from_slice(data: &[i32], shape: &[usize]) -> Self {
        let len: usize = shape.iter().product();
        assert!(data.len() == len, "data length does not match shape");
        Self {
            data: data.to_vec(),
            shape: shape.to_vec(),
        }
    }

    /// Create a 1D (vector) tensor from a slice.
    pub fn new(data: &[i32]) -> Self {
        let len = data.len();
        Self {
            data: data.to_vec(),
            shape: vec![1, len],
        }
    }

    /// Number of rows (first dimension).  Panics if tensor has no dimensions.
    pub fn rows(&self) -> usize {
        self.shape[0]
    }

    /// Number of columns (second dimension).  Panics if tensor has fewer than
    /// 2 dimensions.
    pub fn cols(&self) -> usize {
        self.shape[1]
    }

    /// Get the value at position `(row, col)` in a 2D tensor.
    pub fn get(&self, row: usize, col: usize) -> i32 {
        self.data[row * self.cols() + col]
    }

    /// Set the value at position `(row, col)` in a 2D tensor.
    pub fn set(&mut self, row: usize, col: usize, val: i32) {
        let c = self.cols();
        self.data[row * c + col] = val;
    }

    /// Total number of elements.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if the tensor contains no elements.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Human-readable representation of the tensor.
    pub fn display(&self) -> String {
        if self.shape.len() == 2 {
            let mut s = format!("Tensor({}x{}) [\n", self.rows(), self.cols());
            for r in 0..self.rows() {
                s.push_str("  [");
                for c in 0..self.cols() {
                    let v = self.get(r, c);
                    // Display as fixed-point: value/SCALE with 2 decimal places
                    let whole = v / SCALE;
                    let frac = ((v % SCALE).abs() * 100) / SCALE;
                    if v < 0 && whole == 0 {
                        s.push_str(&format!("-{}.{:02}", whole.abs(), frac));
                    } else {
                        s.push_str(&format!("{}.{:02}", whole, frac));
                    }
                    if c + 1 < self.cols() {
                        s.push_str(", ");
                    }
                }
                s.push_str("]\n");
            }
            s.push(']');
            s
        } else {
            format!("Tensor(shape={:?}, len={})", self.shape, self.len())
        }
    }
}

// ── Matrix Operations ────────────────────────────────────────────────

/// Matrix multiplication of two 2D tensors using fixed-point arithmetic.
///
/// Given `a` of shape `[M, K]` and `b` of shape `[K, N]`, returns a tensor
/// of shape `[M, N]`.  Because both operands are scaled by `SCALE`, the
/// product of two elements is divided by `SCALE` to maintain the correct
/// scale.
pub fn matmul(a: &Tensor, b: &Tensor) -> Tensor {
    let m = a.rows();
    let k = a.cols();
    let n = b.cols();
    assert!(k == b.rows(), "matmul: incompatible shapes");

    let mut out = Tensor::zeros(&[m, n]);
    for i in 0..m {
        for j in 0..n {
            let mut sum: i64 = 0;
            for p in 0..k {
                sum += a.get(i, p) as i64 * b.get(p, j) as i64;
            }
            // Divide by SCALE to keep the result in fixed-point.
            out.set(i, j, (sum / SCALE as i64) as i32);
        }
    }
    out
}

/// Element-wise addition of two tensors.  Shapes must match.
pub fn add(a: &Tensor, b: &Tensor) -> Tensor {
    assert!(a.data.len() == b.data.len(), "add: length mismatch");
    let data: Vec<i32> = a.data.iter().zip(b.data.iter()).map(|(x, y)| x + y).collect();
    // If b is 1-row (bias), broadcast to a's shape.
    Tensor { data, shape: a.shape.clone() }
}

/// ReLU activation: `max(0, x)` for every element.
pub fn relu(t: &Tensor) -> Tensor {
    let data: Vec<i32> = t.data.iter().map(|&x| if x > 0 { x } else { 0 }).collect();
    Tensor { data, shape: t.shape.clone() }
}

/// Piecewise linear sigmoid approximation (integer-only).
///
/// Maps input `x` (fixed-point) to an output in `[0, SCALE]` (representing
/// `[0.0, 1.0]`) using a 5-segment piecewise linear approximation:
///
/// - `x <= -5*SCALE` => 0
/// - `-5*SCALE < x <= -2.5*SCALE` => linear from 0 to 0.05*SCALE
/// - `-2.5*SCALE < x <= 0` => linear from 0.05*SCALE to 0.5*SCALE
/// - `0 < x <= 2.5*SCALE` => linear from 0.5*SCALE to 0.95*SCALE
/// - `x > 5*SCALE` => SCALE
pub fn sigmoid_approx(t: &Tensor) -> Tensor {
    let s = SCALE as i64;
    let data: Vec<i32> = t.data.iter().map(|&x| {
        let x = x as i64;
        let result = if x <= -5 * s {
            0
        } else if x <= -5 * s / 2 {
            // Linear from 0 to 0.05*SCALE over [-5*S, -2.5*S]
            let range = 5 * s / 2; // width of segment
            let offset = x + 5 * s; // 0..range
            (offset * (s / 20)) / range
        } else if x <= 0 {
            // Linear from 0.05*SCALE to 0.5*SCALE over [-2.5*S, 0]
            let range = 5 * s / 2;
            let offset = x + 5 * s / 2;
            s / 20 + (offset * (9 * s / 20)) / range
        } else if x <= 5 * s / 2 {
            // Linear from 0.5*SCALE to 0.95*SCALE over [0, 2.5*S]
            let range = 5 * s / 2;
            s / 2 + (x * (9 * s / 20)) / range
        } else if x <= 5 * s {
            // Linear from 0.95*SCALE to SCALE over [2.5*S, 5*S]
            let range = 5 * s / 2;
            let offset = x - 5 * s / 2;
            19 * s / 20 + (offset * (s / 20)) / range
        } else {
            s
        };
        result as i32
    }).collect();
    Tensor { data, shape: t.shape.clone() }
}

/// Approximate softmax using integer arithmetic.
///
/// Uses a shifted-exponential approximation: for each element, compute
/// `x - max` (so the largest becomes 0), then approximate `exp(x)` via
/// `max(0, SCALE + x)` (a linear approximation of `e^x` near 0).  The
/// results are normalised so they sum to `SCALE`.
pub fn softmax_approx(t: &Tensor) -> Tensor {
    // Find max across all elements.
    let max_val = t.data.iter().copied().max().unwrap_or(0);

    // Compute approximate exp for each element.
    let exps: Vec<i64> = t.data.iter().map(|&x| {
        let shifted = x - max_val; // shifted <= 0
        // Linear approximation: exp(x) ~ max(1, SCALE + shifted)
        let approx = SCALE as i64 + shifted as i64;
        if approx > 0 { approx } else { 1 }
    }).collect();

    let sum: i64 = exps.iter().sum();

    let data: Vec<i32> = exps.iter().map(|&e| {
        if sum > 0 {
            ((e * SCALE as i64) / sum) as i32
        } else {
            SCALE / t.data.len() as i32
        }
    }).collect();

    Tensor { data, shape: t.shape.clone() }
}

/// Return the index of the maximum element in the tensor.
pub fn argmax(t: &Tensor) -> usize {
    t.data.iter()
        .enumerate()
        .max_by_key(|&(_, &v)| v)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

// ── Layer Types ──────────────────────────────────────────────────────

/// A single layer in a sequential neural network.
pub enum LayerType {
    /// Fully-connected (dense) layer with weights and bias.
    /// Weights shape: `[in_features, out_features]`.
    /// Bias shape: `[1, out_features]`.
    Dense { weights: Tensor, bias: Tensor },
    /// ReLU activation function.
    ReLU,
    /// Sigmoid activation function (piecewise linear approximation).
    Sigmoid,
    /// Softmax activation function (integer approximation).
    Softmax,
}

impl LayerType {
    /// Human-readable name and parameter count for this layer.
    fn summary(&self) -> String {
        match self {
            LayerType::Dense { weights, bias } => {
                let params = weights.len() + bias.len();
                format!("Dense({}x{})  params={}", weights.rows(), weights.cols(), params)
            }
            LayerType::ReLU => "ReLU".into(),
            LayerType::Sigmoid => "Sigmoid".into(),
            LayerType::Softmax => "Softmax".into(),
        }
    }
}

// ── Model ────────────────────────────────────────────────────────────

/// A sequential neural network model composed of ordered layers.
pub struct Model {
    /// Human-readable model name.
    pub name: String,
    /// Ordered list of layers.
    pub layers: Vec<LayerType>,
    /// Expected number of input features.
    pub input_size: usize,
    /// Expected number of output features.
    pub output_size: usize,
}

impl Clone for Model {
    fn clone(&self) -> Self {
        let layers = self.layers.iter().map(|l| match l {
            LayerType::Dense { weights, bias } => LayerType::Dense {
                weights: weights.clone(),
                bias: bias.clone(),
            },
            LayerType::ReLU => LayerType::ReLU,
            LayerType::Sigmoid => LayerType::Sigmoid,
            LayerType::Softmax => LayerType::Softmax,
        }).collect();
        Self {
            name: self.name.clone(),
            layers,
            input_size: self.input_size,
            output_size: self.output_size,
        }
    }
}

impl Model {
    /// Create a new empty model.
    pub fn new(name: &str, input_size: usize, output_size: usize) -> Self {
        Self {
            name: String::from(name),
            layers: Vec::new(),
            input_size,
            output_size,
        }
    }

    /// Add a dense (fully-connected) layer with zero-initialised weights.
    pub fn add_dense(&mut self, in_features: usize, out_features: usize) {
        let weights = Tensor::zeros(&[in_features, out_features]);
        let bias = Tensor::zeros(&[1, out_features]);
        self.layers.push(LayerType::Dense { weights, bias });
    }

    /// Add a ReLU activation layer.
    pub fn add_relu(&mut self) {
        self.layers.push(LayerType::ReLU);
    }

    /// Add a sigmoid activation layer.
    pub fn add_sigmoid(&mut self) {
        self.layers.push(LayerType::Sigmoid);
    }

    /// Add a softmax activation layer.
    pub fn add_softmax(&mut self) {
        self.layers.push(LayerType::Softmax);
    }

    /// Run forward inference through all layers.
    ///
    /// `input` should be a 2D tensor of shape `[1, input_size]` with
    /// fixed-point values.  Returns the output tensor after passing through
    /// every layer sequentially.
    pub fn forward(&self, input: &Tensor) -> Tensor {
        let mut x = input.clone();
        for layer in &self.layers {
            x = match layer {
                LayerType::Dense { weights, bias } => {
                    let z = matmul(&x, weights);
                    // Broadcast-add bias (bias is [1, out_features]).
                    add(&z, bias)
                }
                LayerType::ReLU => relu(&x),
                LayerType::Sigmoid => sigmoid_approx(&x),
                LayerType::Softmax => softmax_approx(&x),
            };
        }
        x
    }

    /// Return a human-readable summary of the model architecture.
    pub fn summary(&self) -> String {
        let mut s = format!("Model: {} (input={}, output={})\n", self.name, self.input_size, self.output_size);
        s.push_str("--------------------------------------\n");
        for (i, layer) in self.layers.iter().enumerate() {
            s.push_str(&format!("  [{}] {}\n", i, layer.summary()));
        }
        let total_params: usize = self.layers.iter().map(|l| match l {
            LayerType::Dense { weights, bias } => weights.len() + bias.len(),
            _ => 0,
        }).sum();
        s.push_str(&format!("--------------------------------------\n"));
        s.push_str(&format!("Total parameters: {}\n", total_params));
        s
    }
}

// ── Global Model Registry ────────────────────────────────────────────

/// Global registry of loaded models, protected by a spinlock.
static MODELS: Mutex<Vec<Model>> = Mutex::new(Vec::new());

/// Total number of inference runs (lock-free atomic).
static TOTAL_INFERENCES: AtomicU64 = AtomicU64::new(0);

/// Cumulative "time" of all inferences in arbitrary ticks (for averaging).
static TOTAL_INFERENCE_TICKS: AtomicU64 = AtomicU64::new(0);

/// Register a model in the global registry.
///
/// If a model with the same name already exists it is replaced.
pub fn register_model(model: Model) {
    let mut models = MODELS.lock();
    if let Some(pos) = models.iter().position(|m| m.name == model.name) {
        models[pos] = model;
    } else {
        models.push(model);
    }
}

/// Retrieve a clone of a model by name from the registry.
pub fn get_model(name: &str) -> Option<Model> {
    let models = MODELS.lock();
    models.iter().find(|m| m.name == name).cloned()
}

/// List all registered models.
pub fn list_models() -> String {
    let models = MODELS.lock();
    if models.is_empty() {
        return "No models registered.\n".into();
    }
    let mut s = format!("Registered models ({}):\n", models.len());
    for m in models.iter() {
        let params: usize = m.layers.iter().map(|l| match l {
            LayerType::Dense { weights, bias } => weights.len() + bias.len(),
            _ => 0,
        }).sum();
        s.push_str(&format!("  {} — in={} out={} layers={} params={}\n",
            m.name, m.input_size, m.output_size, m.layers.len(), params));
    }
    s
}

/// Run inference on a named model with the given raw fixed-point input.
///
/// Returns the output tensor data as a `Vec<i32>` on success.
pub fn run_inference(model_name: &str, input: &[i32]) -> Result<Vec<i32>, &'static str> {
    let model = get_model(model_name).ok_or("model not found")?;
    if input.len() != model.input_size {
        return Err("input size mismatch");
    }

    let input_tensor = Tensor::new(input);

    // Simple tick counter for stats (use data length as proxy if no timer).
    let tick_start = TOTAL_INFERENCES.load(Ordering::Relaxed);
    let output = model.forward(&input_tensor);
    let _ = tick_start; // avoid unused warning

    TOTAL_INFERENCES.fetch_add(1, Ordering::Relaxed);
    // Approximate cost: sum of weight counts traversed.
    let cost: u64 = model.layers.iter().map(|l| match l {
        LayerType::Dense { weights, .. } => weights.len() as u64,
        _ => 0u64,
    }).sum();
    TOTAL_INFERENCE_TICKS.fetch_add(cost, Ordering::Relaxed);

    Ok(output.data)
}

// ── Built-in Demo Model ──────────────────────────────────────────────

/// Initialise the neural network subsystem and register a demo XOR
/// classifier model.
///
/// The XOR model has architecture: Dense(2->4) -> ReLU -> Dense(4->2) ->
/// Softmax.  Weights are set manually to approximate XOR behaviour.
pub fn init() {
    let mut model = Model::new("xor", 2, 2);

    // Layer 0: Dense(2 -> 4)
    // Weights chosen to separate XOR inputs.
    //   neuron 0:  x1 + x2  (fires when both high)
    //   neuron 1:  x1 - x2  (fires when x1 high, x2 low)
    //   neuron 2: -x1 + x2  (fires when x2 high, x1 low)
    //   neuron 3: -x1 - x2  (fires when both low)
    let w1 = Tensor::from_slice(&[
        // row 0 (input x1):  [+S, +S, -S, -S]
         SCALE,  SCALE, -SCALE, -SCALE,
        // row 1 (input x2):  [+S, -S, +S, -S]
         SCALE, -SCALE,  SCALE, -SCALE,
    ], &[2, 4]);
    let b1 = Tensor::from_slice(&[
        // bias: shift so neuron 3 fires for (0,0) and neuron 0 needs both
        -SCALE / 2, 0, 0, SCALE / 2,
    ], &[1, 4]);
    model.layers.push(LayerType::Dense { weights: w1, bias: b1 });

    // Layer 1: ReLU
    model.layers.push(LayerType::ReLU);

    // Layer 2: Dense(4 -> 2)
    // Output class 0 = "not XOR" (inputs same), class 1 = "XOR" (inputs differ)
    let w2 = Tensor::from_slice(&[
        // neuron 0 (both high) -> class 0
         SCALE, -SCALE,
        // neuron 1 (x1 high only) -> class 1
        -SCALE,  SCALE,
        // neuron 2 (x2 high only) -> class 1
        -SCALE,  SCALE,
        // neuron 3 (both low) -> class 0
         SCALE, -SCALE,
    ], &[4, 2]);
    let b2 = Tensor::zeros(&[1, 2]);
    model.layers.push(LayerType::Dense { weights: w2, bias: b2 });

    // Layer 3: Softmax
    model.layers.push(LayerType::Softmax);

    register_model(model);
}

/// Run the XOR demo model on all four input combinations and return a
/// formatted string showing predictions.
pub fn demo_inference() -> String {
    let inputs: &[(i32, i32)] = &[
        (0, 0),
        (0, SCALE),    // 0, 1.0
        (SCALE, 0),    // 1.0, 0
        (SCALE, SCALE), // 1.0, 1.0
    ];
    let labels = ["0 XOR 0", "0 XOR 1", "1 XOR 0", "1 XOR 1"];
    let expected = [0usize, 1, 1, 0]; // expected class

    let mut out = String::from("XOR Neural Network Demo (quantized i32, scale=256)\n");
    out.push_str("===================================================\n");

    for (i, &(x1, x2)) in inputs.iter().enumerate() {
        match run_inference("xor", &[x1, x2]) {
            Ok(result) => {
                let class = result.iter()
                    .enumerate()
                    .max_by_key(|&(_, &v)| v)
                    .map(|(idx, _)| idx)
                    .unwrap_or(0);
                let correct = if class == expected[i] { "ok" } else { "WRONG" };
                out.push_str(&format!(
                    "  {} => class {} (scores: [{}, {}]) [{}]\n",
                    labels[i], class,
                    result[0], result[1],
                    correct,
                ));
            }
            Err(e) => {
                out.push_str(&format!("  {} => ERROR: {}\n", labels[i], e));
            }
        }
    }
    out.push('\n');
    out.push_str(&inference_stats());
    out
}

// ── ONNX-like Model Loading ──────────────────────────────────────────

/// Load a model from a simple binary format stored in the VFS.
///
/// Binary format (all values little-endian):
///
/// ```text
/// [name_len: u8][name: u8 * name_len]
/// [input_size: u32][output_size: u32][num_layers: u32]
/// For each layer:
///   [type: u8]
///     0 = Dense: [in_feat: u32][out_feat: u32]
///                [weights: i32 * in_feat * out_feat]
///                [bias: i32 * out_feat]
///     1 = ReLU   (no extra data)
///     2 = Sigmoid (no extra data)
///     3 = Softmax (no extra data)
/// ```
pub fn load_model(path: &str) -> Result<Model, &'static str> {
    let content = vfs::cat(path).map_err(|_| "failed to read model file")?;
    let bytes = content.as_bytes();
    let mut pos: usize = 0;

    if bytes.is_empty() {
        return Err("empty model file");
    }

    // Read model name.
    let name_len = bytes[pos] as usize;
    pos += 1;
    if pos + name_len > bytes.len() {
        return Err("truncated model name");
    }
    let name = core::str::from_utf8(&bytes[pos..pos + name_len])
        .map_err(|_| "invalid model name encoding")?;
    pos += name_len;

    // Helper to read a little-endian u32.
    let read_u32 = |data: &[u8], offset: &mut usize| -> Result<u32, &'static str> {
        if *offset + 4 > data.len() {
            return Err("truncated model data");
        }
        let val = u32::from_le_bytes([
            data[*offset], data[*offset + 1],
            data[*offset + 2], data[*offset + 3],
        ]);
        *offset += 4;
        Ok(val)
    };

    // Helper to read a little-endian i32.
    let read_i32 = |data: &[u8], offset: &mut usize| -> Result<i32, &'static str> {
        if *offset + 4 > data.len() {
            return Err("truncated model data");
        }
        let val = i32::from_le_bytes([
            data[*offset], data[*offset + 1],
            data[*offset + 2], data[*offset + 3],
        ]);
        *offset += 4;
        Ok(val)
    };

    let input_size = read_u32(bytes, &mut pos)? as usize;
    let output_size = read_u32(bytes, &mut pos)? as usize;
    let num_layers = read_u32(bytes, &mut pos)? as usize;

    let mut model = Model::new(name, input_size, output_size);

    for _ in 0..num_layers {
        if pos >= bytes.len() {
            return Err("truncated layer data");
        }
        let layer_type = bytes[pos];
        pos += 1;

        match layer_type {
            0 => {
                // Dense layer
                let in_feat = read_u32(bytes, &mut pos)? as usize;
                let out_feat = read_u32(bytes, &mut pos)? as usize;

                let mut w_data = Vec::with_capacity(in_feat * out_feat);
                for _ in 0..(in_feat * out_feat) {
                    w_data.push(read_i32(bytes, &mut pos)?);
                }
                let weights = Tensor::from_slice(&w_data, &[in_feat, out_feat]);

                let mut b_data = Vec::with_capacity(out_feat);
                for _ in 0..out_feat {
                    b_data.push(read_i32(bytes, &mut pos)?);
                }
                let bias = Tensor::from_slice(&b_data, &[1, out_feat]);

                model.layers.push(LayerType::Dense { weights, bias });
            }
            1 => model.layers.push(LayerType::ReLU),
            2 => model.layers.push(LayerType::Sigmoid),
            3 => model.layers.push(LayerType::Softmax),
            _ => return Err("unknown layer type"),
        }
    }

    Ok(model)
}

// ── Statistics ────────────────────────────────────────────────────────

/// Return inference statistics as a formatted string.
pub fn inference_stats() -> String {
    let count = TOTAL_INFERENCES.load(Ordering::Relaxed);
    let ticks = TOTAL_INFERENCE_TICKS.load(Ordering::Relaxed);
    let avg = if count > 0 { ticks / count } else { 0 };
    format!(
        "Inference stats: {} runs, {} total ops, ~{} ops/inference\n",
        count, ticks, avg,
    )
}
