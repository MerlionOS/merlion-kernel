/// Machine learning training foundation — in-kernel ML with fixed-point math.
/// Implements linear regression (gradient descent), decision tree (Gini impurity),
/// and k-nearest neighbors. All arithmetic uses i32 with SCALE=256 (no floats).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::{println, timer};

// ---------------------------------------------------------------------------
// Fixed-point scale factor: 1.0 = 256
// ---------------------------------------------------------------------------

const SCALE: i32 = 256;

// ---------------------------------------------------------------------------
// Global stats
// ---------------------------------------------------------------------------

static TRAIN_COUNT: AtomicUsize = AtomicUsize::new(0);
static PREDICT_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn ml_stats() -> String {
    format!(
        "ML stats: {} training runs, {} predictions",
        TRAIN_COUNT.load(Ordering::Relaxed),
        PREDICT_COUNT.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// Dataset
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DataPoint {
    pub features: Vec<i32>,
    pub label: i32,
}

#[derive(Debug, Clone)]
pub struct Dataset {
    pub name: String,
    pub points: Vec<DataPoint>,
    pub feature_count: usize,
}

impl Dataset {
    pub fn new(name: &str, feature_count: usize) -> Self {
        Self {
            name: String::from(name),
            points: Vec::new(),
            feature_count,
        }
    }

    pub fn add(&mut self, features: Vec<i32>, label: i32) {
        assert_eq!(features.len(), self.feature_count);
        self.points.push(DataPoint { features, label });
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Split into (train, test) by percentage (0-100).
    pub fn split(&self, ratio_pct: usize) -> (Dataset, Dataset) {
        let split_idx = self.points.len() * ratio_pct / 100;
        let mut train = Dataset::new(&self.name, self.feature_count);
        let mut test = Dataset::new(&self.name, self.feature_count);
        for (i, p) in self.points.iter().enumerate() {
            if i < split_idx {
                train.points.push(p.clone());
            } else {
                test.points.push(p.clone());
            }
        }
        (train, test)
    }
}

// ---------------------------------------------------------------------------
// TrainResult
// ---------------------------------------------------------------------------

pub struct TrainResult {
    pub model_type: String,
    pub epochs: usize,
    pub final_loss: i32,
    pub accuracy_pct: usize,
    pub train_time_ticks: u64,
}

pub fn format_result(result: &TrainResult) -> String {
    format!(
        "[{}] epochs={} loss={}.{:02} accuracy={}% time={} ticks",
        result.model_type,
        result.epochs,
        result.final_loss / SCALE,
        ((result.final_loss % SCALE).abs() * 100 / SCALE),
        result.accuracy_pct,
        result.train_time_ticks,
    )
}

// ---------------------------------------------------------------------------
// Linear Regression (gradient descent, fixed-point)
// ---------------------------------------------------------------------------

pub struct LinearModel {
    pub weights: Vec<i32>,
    pub bias: i32,
    pub learning_rate: i32,
}

impl LinearModel {
    pub fn new(feature_count: usize) -> Self {
        Self {
            weights: alloc::vec![0i32; feature_count],
            bias: 0,
            learning_rate: 2, // 2/256 ~ 0.0078
        }
    }

    /// Dot product + bias (fixed-point).
    pub fn predict(&self, features: &[i32]) -> i32 {
        PREDICT_COUNT.fetch_add(1, Ordering::Relaxed);
        let mut sum: i64 = self.bias as i64 * SCALE as i64;
        for (w, f) in self.weights.iter().zip(features.iter()) {
            sum += (*w as i64) * (*f as i64);
        }
        (sum / SCALE as i64) as i32
    }

    /// Train via gradient descent. Returns TrainResult.
    pub fn train(&mut self, dataset: &Dataset, epochs: usize) -> TrainResult {
        let start = timer::ticks();
        let n = dataset.len() as i64;
        if n == 0 {
            return TrainResult {
                model_type: String::from("LinearRegression"),
                epochs: 0,
                final_loss: 0,
                accuracy_pct: 0,
                train_time_ticks: 0,
            };
        }

        let mut last_loss: i32 = 0;

        for _epoch in 0..epochs {
            // Accumulate gradients
            let mut grad_w: Vec<i64> = alloc::vec![0i64; self.weights.len()];
            let mut grad_b: i64 = 0;

            for p in &dataset.points {
                let pred = self.predict(&p.features);
                let error = pred - p.label; // fixed-point
                for (j, f) in p.features.iter().enumerate() {
                    grad_w[j] += (error as i64) * (*f as i64) / SCALE as i64;
                }
                grad_b += error as i64;
            }

            // Update weights: w -= lr * grad / (n * SCALE)
            let lr = self.learning_rate as i64;
            for (j, w) in self.weights.iter_mut().enumerate() {
                let delta = lr * grad_w[j] / (n * SCALE as i64);
                *w -= delta as i32;
            }
            self.bias -= (lr * grad_b / (n * SCALE as i64)) as i32;

            last_loss = self.evaluate(dataset);
        }

        TRAIN_COUNT.fetch_add(1, Ordering::Relaxed);
        TrainResult {
            model_type: String::from("LinearRegression"),
            epochs,
            final_loss: last_loss,
            accuracy_pct: 0, // regression — not applicable
            train_time_ticks: timer::ticks() - start,
        }
    }

    /// Mean squared error (fixed-point).
    pub fn evaluate(&self, dataset: &Dataset) -> i32 {
        if dataset.len() == 0 {
            return 0;
        }
        let mut total: i64 = 0;
        for p in &dataset.points {
            let err = self.predict(&p.features) - p.label;
            total += (err as i64) * (err as i64) / SCALE as i64;
        }
        (total / dataset.len() as i64) as i32
    }

    pub fn summary(&self) -> String {
        let mut s = String::from("LinearModel { weights: [");
        for (i, w) in self.weights.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&format!("{}.{:02}", w / SCALE, (w % SCALE).abs() * 100 / SCALE));
        }
        s.push_str(&format!("], bias: {}.{:02} }}", self.bias / SCALE, (self.bias % SCALE).abs() * 100 / SCALE));
        s
    }
}

// ---------------------------------------------------------------------------
// Decision Tree (Gini impurity, max depth 4)
// ---------------------------------------------------------------------------

struct TreeNode {
    feature_index: usize,
    threshold: i32,
    left: Option<Box<TreeNode>>,
    right: Option<Box<TreeNode>>,
    prediction: Option<i32>,
}

pub struct DecisionTree {
    root: Option<Box<TreeNode>>,
    max_depth: usize,
}

impl DecisionTree {
    pub fn new(max_depth: usize) -> Self {
        Self { root: None, max_depth }
    }

    pub fn train(&mut self, dataset: &Dataset) {
        let start = timer::ticks();
        self.root = Self::build(&dataset.points, 0, self.max_depth);
        TRAIN_COUNT.fetch_add(1, Ordering::Relaxed);
        let elapsed = timer::ticks() - start;
        let _ = elapsed; // used only for timing
    }

    fn majority_label(points: &[DataPoint]) -> i32 {
        // Find most common label
        let mut labels: Vec<(i32, usize)> = Vec::new();
        for p in points {
            let mut found = false;
            for entry in labels.iter_mut() {
                if entry.0 == p.label {
                    entry.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                labels.push((p.label, 1));
            }
        }
        labels.iter().max_by_key(|e| e.1).map(|e| e.0).unwrap_or(0)
    }

    /// Gini impurity (fixed-point, SCALE=256).
    fn gini(points: &[DataPoint]) -> i32 {
        if points.is_empty() {
            return 0;
        }
        let n = points.len() as i64;
        let mut labels: Vec<(i32, i64)> = Vec::new();
        for p in points {
            let mut found = false;
            for entry in labels.iter_mut() {
                if entry.0 == p.label {
                    entry.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                labels.push((p.label, 1));
            }
        }
        // gini = 1 - sum(p_i^2)
        let mut sum_sq: i64 = 0;
        for (_, count) in &labels {
            // p_i = count / n, p_i^2 = count^2 / n^2
            sum_sq += count * count;
        }
        // gini * SCALE = SCALE - sum_sq * SCALE / n^2
        (SCALE as i64 - sum_sq * SCALE as i64 / (n * n)) as i32
    }

    fn build(points: &[DataPoint], depth: usize, max_depth: usize) -> Option<Box<TreeNode>> {
        if points.is_empty() {
            return None;
        }
        // Leaf if pure or max depth
        let label = points[0].label;
        let pure = points.iter().all(|p| p.label == label);
        if pure || depth >= max_depth || points.len() <= 2 {
            return Some(Box::new(TreeNode {
                feature_index: 0,
                threshold: 0,
                left: None,
                right: None,
                prediction: Some(Self::majority_label(points)),
            }));
        }

        let feature_count = points[0].features.len();
        let mut best_feat = 0;
        let mut best_thresh = 0i32;
        let mut best_score = i32::MAX; // lower is better

        for fi in 0..feature_count {
            // Collect unique thresholds (midpoints of sorted values)
            let mut vals: Vec<i32> = points.iter().map(|p| p.features[fi]).collect();
            vals.sort();
            vals.dedup();

            for window in vals.windows(2) {
                let thresh = (window[0] / 2) + (window[1] / 2); // avoid overflow
                let left: Vec<&DataPoint> = points.iter().filter(|p| p.features[fi] <= thresh).collect();
                let right: Vec<&DataPoint> = points.iter().filter(|p| p.features[fi] > thresh).collect();
                if left.is_empty() || right.is_empty() {
                    continue;
                }
                // Weighted Gini
                let n = points.len() as i64;
                let gl = Self::gini_refs(&left) as i64;
                let gr = Self::gini_refs(&right) as i64;
                let score = (gl * left.len() as i64 + gr * right.len() as i64) / n;
                if (score as i32) < best_score {
                    best_score = score as i32;
                    best_feat = fi;
                    best_thresh = thresh;
                }
            }
        }

        let left_pts: Vec<DataPoint> = points.iter().filter(|p| p.features[best_feat] <= best_thresh).cloned().collect();
        let right_pts: Vec<DataPoint> = points.iter().filter(|p| p.features[best_feat] > best_thresh).cloned().collect();

        if left_pts.is_empty() || right_pts.is_empty() {
            return Some(Box::new(TreeNode {
                feature_index: 0,
                threshold: 0,
                left: None,
                right: None,
                prediction: Some(Self::majority_label(points)),
            }));
        }

        Some(Box::new(TreeNode {
            feature_index: best_feat,
            threshold: best_thresh,
            left: Self::build(&left_pts, depth + 1, max_depth),
            right: Self::build(&right_pts, depth + 1, max_depth),
            prediction: None,
        }))
    }

    fn gini_refs(points: &[&DataPoint]) -> i32 {
        if points.is_empty() {
            return 0;
        }
        let n = points.len() as i64;
        let mut labels: Vec<(i32, i64)> = Vec::new();
        for p in points {
            let mut found = false;
            for entry in labels.iter_mut() {
                if entry.0 == p.label {
                    entry.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                labels.push((p.label, 1));
            }
        }
        let mut sum_sq: i64 = 0;
        for (_, count) in &labels {
            sum_sq += count * count;
        }
        (SCALE as i64 - sum_sq * SCALE as i64 / (n * n)) as i32
    }

    pub fn predict(&self, features: &[i32]) -> i32 {
        PREDICT_COUNT.fetch_add(1, Ordering::Relaxed);
        Self::predict_node(&self.root, features)
    }

    fn predict_node(node: &Option<Box<TreeNode>>, features: &[i32]) -> i32 {
        match node {
            None => 0,
            Some(n) => {
                if let Some(pred) = n.prediction {
                    return pred;
                }
                if features[n.feature_index] <= n.threshold {
                    Self::predict_node(&n.left, features)
                } else {
                    Self::predict_node(&n.right, features)
                }
            }
        }
    }

    /// Returns (correct, total).
    pub fn evaluate(&self, dataset: &Dataset) -> (usize, usize) {
        let mut correct = 0;
        for p in &dataset.points {
            if self.predict(&p.features) == p.label {
                correct += 1;
            }
        }
        (correct, dataset.len())
    }

    pub fn display(&self) -> String {
        let mut out = String::from("DecisionTree:\n");
        Self::display_node(&self.root, &mut out, 0);
        out
    }

    fn display_node(node: &Option<Box<TreeNode>>, out: &mut String, indent: usize) {
        let pad: String = core::iter::repeat(' ').take(indent * 2).collect();
        match node {
            None => {
                out.push_str(&format!("{}(empty)\n", pad));
            }
            Some(n) => {
                if let Some(pred) = n.prediction {
                    out.push_str(&format!("{}-> predict {}\n", pad, pred));
                } else {
                    out.push_str(&format!("{}[f{} <= {}]\n", pad, n.feature_index, n.threshold));
                    Self::display_node(&n.left, out, indent + 1);
                    out.push_str(&format!("{}[f{} > {}]\n", pad, n.feature_index, n.threshold));
                    Self::display_node(&n.right, out, indent + 1);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// K-Nearest Neighbors
// ---------------------------------------------------------------------------

pub struct KNN {
    pub k: usize,
    pub training_data: Vec<DataPoint>,
}

impl KNN {
    pub fn new(k: usize) -> Self {
        Self { k, training_data: Vec::new() }
    }

    pub fn fit(&mut self, dataset: &Dataset) {
        self.training_data = dataset.points.clone();
        TRAIN_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    /// Squared Euclidean distance (no sqrt needed for comparison).
    fn distance_sq(a: &[i32], b: &[i32]) -> i64 {
        let mut sum: i64 = 0;
        for (x, y) in a.iter().zip(b.iter()) {
            let d = (*x as i64) - (*y as i64);
            sum += d * d;
        }
        sum
    }

    pub fn predict(&self, features: &[i32]) -> i32 {
        PREDICT_COUNT.fetch_add(1, Ordering::Relaxed);
        if self.training_data.is_empty() {
            return 0;
        }

        // Compute distances and find k nearest
        let mut dists: Vec<(i64, i32)> = self
            .training_data
            .iter()
            .map(|p| (Self::distance_sq(features, &p.features), p.label))
            .collect();
        dists.sort_by(|a, b| a.0.cmp(&b.0));

        // Majority vote among k nearest
        let k = if self.k > dists.len() { dists.len() } else { self.k };
        let mut votes: Vec<(i32, usize)> = Vec::new();
        for i in 0..k {
            let label = dists[i].1;
            let mut found = false;
            for v in votes.iter_mut() {
                if v.0 == label {
                    v.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                votes.push((label, 1));
            }
        }
        votes.iter().max_by_key(|v| v.1).map(|v| v.0).unwrap_or(0)
    }

    /// Returns (correct, total).
    pub fn evaluate(&self, dataset: &Dataset) -> (usize, usize) {
        let mut correct = 0;
        for p in &dataset.points {
            if self.predict(&p.features) == p.label {
                correct += 1;
            }
        }
        (correct, dataset.len())
    }
}

// ---------------------------------------------------------------------------
// Demo datasets
// ---------------------------------------------------------------------------

/// Create a simple 2D linear dataset, train linear regression, evaluate.
pub fn demo_linear() -> String {
    let mut ds = Dataset::new("linear_2d", 1);
    // y ≈ 2x + 10 (in fixed-point: y = 2*SCALE*x/SCALE + 10*SCALE)
    for i in 0..20 {
        let x = i * SCALE / 2; // 0.0, 0.5, 1.0, ...
        let y = 2 * x + 10 * SCALE; // y = 2x + 10
        // Add small "noise" via simple pattern
        let noise = if i % 3 == 0 { SCALE / 4 } else if i % 3 == 1 { -(SCALE / 4) } else { 0 };
        ds.add(alloc::vec![x], y + noise);
    }

    let (train, test) = ds.split(80);
    let mut model = LinearModel::new(1);
    let result = model.train(&train, 200);
    let test_mse = model.evaluate(&test);

    let mut out = String::from("=== Linear Regression Demo ===\n");
    out.push_str(&format!("Dataset: {} points ({} train, {} test)\n", ds.len(), train.len(), test.len()));
    out.push_str(&format!("Training: {}\n", format_result(&result)));
    out.push_str(&format!("Model: {}\n", model.summary()));
    out.push_str(&format!(
        "Test MSE: {}.{:02}\n",
        test_mse / SCALE,
        (test_mse % SCALE).abs() * 100 / SCALE
    ));
    out
}

/// Create a classification dataset, train decision tree + KNN.
pub fn demo_classify() -> String {
    let mut ds = Dataset::new("classify_2d", 2);
    // Two clusters: label 0 around (1,1), label 1 around (3,3) (fixed-point)
    let offsets: [i32; 5] = [0, SCALE / 4, -(SCALE / 4), SCALE / 2, -(SCALE / 2)];
    for &dx in &offsets {
        for &dy in &offsets {
            ds.add(alloc::vec![1 * SCALE + dx, 1 * SCALE + dy], 0);
            ds.add(alloc::vec![3 * SCALE + dx, 3 * SCALE + dy], 1);
        }
    }

    let (train, test) = ds.split(80);

    // Decision tree
    let mut tree = DecisionTree::new(4);
    tree.train(&train);
    let (dt_correct, dt_total) = tree.evaluate(&test);

    // KNN
    let mut knn = KNN::new(3);
    knn.fit(&train);
    let (knn_correct, knn_total) = knn.evaluate(&test);

    let mut out = String::from("=== Classification Demo ===\n");
    out.push_str(&format!("Dataset: {} points ({} train, {} test)\n", ds.len(), train.len(), test.len()));
    out.push_str(&format!("DecisionTree: {}/{} correct ({}%)\n", dt_correct, dt_total, dt_correct * 100 / dt_total.max(1)));
    out.push_str(&format!("KNN(k=3): {}/{} correct ({}%)\n", knn_correct, knn_total, knn_correct * 100 / knn_total.max(1)));
    out.push_str(&tree.display());
    out
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    println!("[ml_train] ML training subsystem initialized (fixed-point SCALE={})", SCALE);
}
