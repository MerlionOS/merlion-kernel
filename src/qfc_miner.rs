/// QFC blockchain miner for MerlionOS.
/// Fetches AI inference tasks from validator, runs inference,
/// generates proofs, and submits them to earn QFC rewards.
/// Runs entirely in-kernel — no std, no external dependencies.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Miner Configuration ────────────────────────────────────────────

/// Configuration for the QFC miner.
pub struct MinerConfig {
    /// 20-byte wallet address.
    pub wallet_address: [u8; 20],
    /// 32-byte Ed25519 private key.
    pub private_key: [u8; 32],
    /// Validator RPC endpoint, e.g. "http://rpc.testnet.qfc.network".
    pub validator_rpc: String,
    /// Compute backend: "cpu", "gpu", etc.
    pub backend: String,
    /// GPU tier (0 = CPU only, 1-3 = GPU tiers).
    pub gpu_tier: u8,
    /// Available memory in megabytes.
    pub available_memory_mb: u64,
}

// ── Inference Task ──────────────────────────────────────────────────

/// An inference task fetched from the QFC validator.
pub struct InferenceTask {
    /// Unique 32-byte task identifier.
    pub task_id: [u8; 32],
    /// Epoch number for this task.
    pub epoch: u64,
    /// Type of inference: "embedding", "text_generation", etc.
    pub task_type: String,
    /// Model to use.
    pub model_name: String,
    /// Raw input data for the inference.
    pub input_data: Vec<u8>,
    /// Deadline in milliseconds from epoch start.
    pub deadline_ms: u64,
}

// ── Inference Result ────────────────────────────────────────────────

/// Result of running an inference task.
pub struct InferenceResult {
    /// BLAKE3 hash of the inference output.
    pub output_hash: [u8; 32],
    /// Wall-clock execution time in milliseconds.
    pub execution_time_ms: u64,
    /// Estimated FLOPs for this inference.
    pub flops_estimated: u64,
    /// Raw output data.
    pub output_data: Vec<u8>,
}

// ── Inference Proof ─────────────────────────────────────────────────

/// Cryptographic proof of completed inference work.
pub struct InferenceProof {
    /// Miner's 20-byte wallet address.
    pub miner_address: [u8; 20],
    /// Epoch number.
    pub epoch: u64,
    /// Type of inference performed.
    pub task_type: String,
    /// BLAKE3 hash of the input (= task_id).
    pub input_hash: [u8; 32],
    /// BLAKE3 hash of the output.
    pub output_hash: [u8; 32],
    /// Execution time in milliseconds.
    pub execution_time_ms: u64,
    /// Estimated FLOPs.
    pub flops_estimated: u64,
    /// Compute backend used.
    pub backend: String,
    /// Timestamp (tick count).
    pub timestamp: u64,
    /// Ed25519 signature over the proof.
    pub signature: [u8; 64],
}

// ── Submit Result ───────────────────────────────────────────────────

/// Result of submitting a proof to the validator.
pub struct SubmitResult {
    /// Whether the proof was accepted.
    pub accepted: bool,
    /// Reward earned in wei (smallest unit).
    pub reward_wei: u128,
    /// Human-readable message from the validator.
    pub message: String,
}

// ── Miner Stats ─────────────────────────────────────────────────────

/// Cumulative mining statistics for the current session.
pub struct MinerStats {
    /// Number of proofs submitted.
    pub proofs_submitted: u64,
    /// Number of proofs accepted by the validator.
    pub proofs_accepted: u64,
    /// Total rewards earned in wei.
    pub total_rewards_wei: u128,
    /// Tick count when the session started.
    pub session_start: u64,
    /// Tick count of the last submitted proof.
    pub last_proof_tick: u64,
}

static EARNINGS: Mutex<MinerStats> = Mutex::new(MinerStats {
    proofs_submitted: 0,
    proofs_accepted: 0,
    total_rewards_wei: 0,
    session_start: 0,
    last_proof_tick: 0,
});

static TASKS_FETCHED: AtomicU64 = AtomicU64::new(0);
static POW_HASHES: AtomicU64 = AtomicU64::new(0);
static MINING_ACTIVE: AtomicU64 = AtomicU64::new(0);

// ── Task Fetching ───────────────────────────────────────────────────

/// Fetch an inference task from the QFC validator via JSON-RPC.
///
/// In a real implementation this would make an HTTP request to
/// `config.validator_rpc` with method `fetch_task_qfc`.  Since we don't
/// have a real network stack connected to the internet yet, this returns
/// a simulated task for testing.
pub fn fetch_task(config: &MinerConfig) -> Result<InferenceTask, &'static str> {
    let _ = &config.validator_rpc;

    // Simulate a task based on the current tick count for variety
    let tick = crate::timer::ticks();
    TASKS_FETCHED.fetch_add(1, Ordering::SeqCst);

    let mut task_id = [0u8; 32];
    // Derive task_id from tick + wallet address
    let tick_bytes = tick.to_le_bytes();
    task_id[..8].copy_from_slice(&tick_bytes);
    task_id[8..28].copy_from_slice(&config.wallet_address);
    // Hash it for a proper-looking task ID
    let id_hash = crate::blake3::blake3_hash(&task_id);
    task_id = id_hash;

    let task_type = match tick % 3 {
        0 => String::from("embedding"),
        1 => String::from("text_generation"),
        _ => String::from("classification"),
    };

    let model_name = match tick % 2 {
        0 => String::from("merlion-7b"),
        _ => String::from("merlion-embed-v1"),
    };

    // Simulated input: encode tick as input data
    let mut input_data = Vec::with_capacity(64);
    input_data.extend_from_slice(&tick_bytes);
    input_data.extend_from_slice(b"qfc-inference-input");

    Ok(InferenceTask {
        task_id,
        epoch: tick / 1000,
        task_type,
        model_name,
        input_data,
        deadline_ms: 30000,
    })
}

// ── Inference Execution ─────────────────────────────────────────────

/// Run inference on a task using the appropriate backend.
pub fn run_inference(task: &InferenceTask) -> InferenceResult {
    match task.task_type.as_str() {
        "embedding" => run_embedding(task),
        "text_generation" => run_text_generation(task),
        _ => run_generic(task),
    }
}

/// Run an embedding inference task.
fn run_embedding(task: &InferenceTask) -> InferenceResult {
    let start = crate::timer::ticks();

    // Use nn_inference to produce an embedding-like output.
    // Generate a deterministic output from the input data.
    let mut output = Vec::with_capacity(128);
    let input_hash = crate::blake3::blake3_hash(&task.input_data);

    // Produce 128 bytes of "embedding" by hashing chunks of the input hash
    for i in 0u8..4 {
        let mut seed = input_hash;
        seed[0] = seed[0].wrapping_add(i);
        let chunk_hash = crate::blake3::blake3_hash(&seed);
        output.extend_from_slice(&chunk_hash);
    }

    let end = crate::timer::ticks();
    let elapsed_ms = (end.wrapping_sub(start)) * 10; // ~10ms per tick at 100Hz

    let output_hash = crate::blake3::blake3_hash(&output);

    // Estimate FLOPs: embedding is relatively light
    let flops = (task.input_data.len() as u64) * 768 * 2;

    InferenceResult {
        output_hash,
        execution_time_ms: elapsed_ms,
        flops_estimated: flops,
        output_data: output,
    }
}

/// Run a text generation inference task.
fn run_text_generation(task: &InferenceTask) -> InferenceResult {
    let start = crate::timer::ticks();

    // Simulate text generation using LLM module
    let prompt = core::str::from_utf8(&task.input_data).unwrap_or("hello");
    let generated = crate::llm::generate_text(prompt, 16);
    let output = generated.into_bytes();

    let end = crate::timer::ticks();
    let elapsed_ms = (end.wrapping_sub(start)) * 10;

    let output_hash = crate::blake3::blake3_hash(&output);

    // Text generation is heavier
    let flops = (output.len() as u64) * 7_000_000; // ~7M FLOPs per token

    InferenceResult {
        output_hash,
        execution_time_ms: elapsed_ms,
        flops_estimated: flops,
        output_data: output,
    }
}

/// Run a generic inference task.
fn run_generic(task: &InferenceTask) -> InferenceResult {
    let start = crate::timer::ticks();

    // Generic: hash the input data through multiple rounds
    let mut output = Vec::with_capacity(32);
    let hash = crate::blake3::blake3_hash(&task.input_data);
    output.extend_from_slice(&hash);

    let end = crate::timer::ticks();
    let elapsed_ms = (end.wrapping_sub(start)) * 10;

    let output_hash = crate::blake3::blake3_hash(&output);
    let flops = (task.input_data.len() as u64) * 1000;

    InferenceResult {
        output_hash,
        execution_time_ms: elapsed_ms,
        flops_estimated: flops,
        output_data: output,
    }
}

// ── Proof Generation ────────────────────────────────────────────────

/// Build an inference proof from a completed task and its result.
pub fn build_proof(
    config: &MinerConfig,
    task: &InferenceTask,
    result: &InferenceResult,
) -> InferenceProof {
    let timestamp = crate::timer::ticks();

    // Serialize proof data for signing
    let proof_bytes = serialize_proof_data(
        &config.wallet_address,
        task.epoch,
        &task.task_type,
        &task.task_id,
        &result.output_hash,
        result.execution_time_ms,
        result.flops_estimated,
        &config.backend,
        timestamp,
    );

    // Hash the proof data, then sign
    let proof_hash = crate::blake3::blake3_hash(&proof_bytes);

    // Create keypair and sign
    let keypair = crate::ed25519::generate_keypair(&config.private_key);
    let sig = crate::ed25519::sign_message(&keypair, &proof_hash);

    InferenceProof {
        miner_address: config.wallet_address,
        epoch: task.epoch,
        task_type: task.task_type.clone(),
        input_hash: task.task_id,
        output_hash: result.output_hash,
        execution_time_ms: result.execution_time_ms,
        flops_estimated: result.flops_estimated,
        backend: config.backend.clone(),
        timestamp,
        signature: sig.bytes,
    }
}

/// Serialize proof fields into a byte vector for hashing/signing.
fn serialize_proof_data(
    miner_address: &[u8; 20],
    epoch: u64,
    task_type: &str,
    input_hash: &[u8; 32],
    output_hash: &[u8; 32],
    execution_time_ms: u64,
    flops_estimated: u64,
    backend: &str,
    timestamp: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(256);
    data.extend_from_slice(miner_address);
    data.extend_from_slice(&epoch.to_le_bytes());
    data.extend_from_slice(task_type.as_bytes());
    data.extend_from_slice(input_hash);
    data.extend_from_slice(output_hash);
    data.extend_from_slice(&execution_time_ms.to_le_bytes());
    data.extend_from_slice(&flops_estimated.to_le_bytes());
    data.extend_from_slice(backend.as_bytes());
    data.extend_from_slice(&timestamp.to_le_bytes());
    data
}

// ── Proof Submission ────────────────────────────────────────────────

/// Submit an inference proof to the QFC validator via JSON-RPC.
///
/// In a real implementation this would POST to `config.validator_rpc`
/// with method `submit_inference_proof_qfc`.  Currently simulated.
pub fn submit_proof(
    config: &MinerConfig,
    proof: &InferenceProof,
) -> Result<SubmitResult, &'static str> {
    let _ = &config.validator_rpc;

    // Verify the proof is well-formed
    if proof.execution_time_ms == 0 && proof.flops_estimated == 0 {
        return Err("invalid proof: zero execution time and flops");
    }

    // Simulate validator acceptance — in reality, the validator would verify
    // the signature, re-run the inference, and compare output hashes.
    let mut stats = EARNINGS.lock();
    stats.proofs_submitted += 1;

    // Simulate ~90% acceptance rate based on proof hash
    let acceptance_check = proof.output_hash[0];
    let accepted = acceptance_check < 230; // ~90%

    let reward_wei: u128 = if accepted {
        stats.proofs_accepted += 1;
        // Reward based on FLOPs: ~1 wei per 1000 FLOPs
        let reward = (proof.flops_estimated as u128) / 1000;
        let reward = if reward == 0 { 1 } else { reward };
        stats.total_rewards_wei += reward;
        reward
    } else {
        0
    };

    stats.last_proof_tick = crate::timer::ticks();

    let message = if accepted {
        format!("proof accepted, reward={} wei", reward_wei)
    } else {
        String::from("proof rejected: output mismatch")
    };

    Ok(SubmitResult {
        accepted,
        reward_wei,
        message,
    })
}

// ── Mining Loop ─────────────────────────────────────────────────────

/// Start the QFC mining loop.  Fetches tasks, runs inference, builds
/// proofs, and submits them.  Returns a status string.
///
/// In a real kernel this would run as a background task.  For now it
/// runs a fixed number of iterations and returns stats.
pub fn start_mining(config: &MinerConfig) -> String {
    MINING_ACTIVE.store(1, Ordering::SeqCst);

    {
        let mut stats = EARNINGS.lock();
        stats.session_start = crate::timer::ticks();
    }

    let max_iterations = 5u32;
    let mut accepted = 0u32;
    let mut rejected = 0u32;
    let mut total_reward: u128 = 0;

    for _ in 0..max_iterations {
        // Fetch task
        let task = match fetch_task(config) {
            Ok(t) => t,
            Err(e) => {
                MINING_ACTIVE.store(0, Ordering::SeqCst);
                return format!("Mining error: failed to fetch task: {}", e);
            }
        };

        // Run inference
        let result = run_inference(&task);

        // Build proof
        let proof = build_proof(config, &task, &result);

        // Submit proof
        match submit_proof(config, &proof) {
            Ok(sr) => {
                if sr.accepted {
                    accepted += 1;
                    total_reward += sr.reward_wei;
                } else {
                    rejected += 1;
                }
            }
            Err(e) => {
                MINING_ACTIVE.store(0, Ordering::SeqCst);
                return format!("Mining error: failed to submit proof: {}", e);
            }
        }
    }

    MINING_ACTIVE.store(0, Ordering::SeqCst);

    format!(
        "Mining session complete: {} iterations, {} accepted, {} rejected, total reward={} wei",
        max_iterations, accepted, rejected, total_reward
    )
}

// ── PoW Mining (fallback) ───────────────────────────────────────────

/// Simple proof-of-work mining: find a nonce such that
/// `blake3(seed || nonce)` has a numeric value below `difficulty`.
///
/// Returns `Some((nonce, hash))` on success, `None` if `max_nonce` is
/// exhausted without finding a valid hash.
pub fn mine_pow(seed: &[u8; 32], difficulty: u64, max_nonce: u64) -> Option<(u64, [u8; 32])> {
    for nonce in 0..max_nonce {
        POW_HASHES.fetch_add(1, Ordering::Relaxed);

        let mut data = Vec::with_capacity(40);
        data.extend_from_slice(seed);
        data.extend_from_slice(&nonce.to_le_bytes());
        let hash = crate::blake3::blake3_hash(&data);

        // Check if hash meets difficulty (leading bytes as u64 < difficulty)
        let hash_val = u64::from_be_bytes([
            hash[0], hash[1], hash[2], hash[3],
            hash[4], hash[5], hash[6], hash[7],
        ]);
        if hash_val < difficulty {
            return Some((nonce, hash));
        }
    }
    None
}

// ── Wallet ──────────────────────────────────────────────────────────

/// Format a 20-byte wallet address as a hex string with "0x" prefix.
fn format_wallet(addr: &[u8; 20]) -> String {
    let hex = crate::ed25519::to_hex(addr);
    format!("0x{}", hex)
}

/// Derive a wallet address (first 20 bytes of the public key hash).
fn derive_wallet_address(private_key: &[u8; 32]) -> [u8; 20] {
    let keypair = crate::ed25519::generate_keypair(private_key);
    let hash = crate::blake3::blake3_hash(&keypair.public.bytes);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[..20]);
    addr
}

// ── Shell Commands ──────────────────────────────────────────────────

/// Handle the `qfc-mine` shell command.
pub fn cmd_mine(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.len() < 2 {
        return String::from(
            "Usage: qfc-mine <private_key_hex> <rpc_url>\n\
             Example: qfc-mine 0123456789abcdef...  http://rpc.testnet.qfc.network"
        );
    }

    let key_hex = parts[0];
    let rpc_url = parts[1];

    let key_bytes = crate::ed25519::from_hex(key_hex);
    if key_bytes.len() != 32 {
        return String::from("Error: private key must be 64 hex characters (32 bytes)");
    }

    let mut private_key = [0u8; 32];
    private_key.copy_from_slice(&key_bytes);

    let wallet_address = derive_wallet_address(&private_key);

    let config = MinerConfig {
        wallet_address,
        private_key,
        validator_rpc: String::from(rpc_url),
        backend: String::from("cpu"),
        gpu_tier: 0,
        available_memory_mb: 256,
    };

    let wallet_str = format_wallet(&config.wallet_address);
    let header = format!(
        "QFC Miner starting...\n  Wallet: {}\n  RPC: {}\n  Backend: cpu\n",
        wallet_str, rpc_url
    );

    let result = start_mining(&config);
    format!("{}{}", header, result)
}

/// Handle the `qfc-pow` shell command.
pub fn cmd_pow(args: &str) -> String {
    let difficulty_str = args.trim();
    let difficulty: u64 = if difficulty_str.is_empty() {
        // Default: find hash with first byte < 16 (1/16 chance per hash)
        1u64 << 60
    } else {
        match difficulty_str.parse::<u64>() {
            Ok(d) => d,
            Err(_) => return String::from("Usage: qfc-pow [difficulty]\n  difficulty = u64 threshold (lower = harder)"),
        }
    };

    let tick = crate::timer::ticks();
    let seed = crate::blake3::blake3_hash(&tick.to_le_bytes());

    let max_nonce = 100_000u64;
    let start = crate::timer::ticks();

    match mine_pow(&seed, difficulty, max_nonce) {
        Some((nonce, hash)) => {
            let elapsed = crate::timer::ticks().wrapping_sub(start) * 10;
            format!(
                "PoW found!\n  Nonce: {}\n  Hash: {}\n  Difficulty: {}\n  Time: ~{}ms\n  Seed: {}",
                nonce,
                crate::blake3::blake3_hex(&hash),
                difficulty,
                elapsed,
                crate::blake3::blake3_hex(&seed),
            )
        }
        None => {
            format!(
                "PoW not found within {} nonces at difficulty {}\n  Seed: {}",
                max_nonce,
                difficulty,
                crate::blake3::blake3_hex(&seed),
            )
        }
    }
}

/// Handle the `qfc-status` shell command.
pub fn cmd_status() -> String {
    let stats = EARNINGS.lock();
    let active = MINING_ACTIVE.load(Ordering::SeqCst);
    let tasks = TASKS_FETCHED.load(Ordering::SeqCst);
    let pow = POW_HASHES.load(Ordering::SeqCst);

    format!(
        "QFC Miner Status:\n\
         \x20 Active: {}\n\
         \x20 Tasks fetched: {}\n\
         \x20 Proofs submitted: {}\n\
         \x20 Proofs accepted: {}\n\
         \x20 Total rewards: {} wei\n\
         \x20 PoW hashes computed: {}\n\
         \x20 Session start tick: {}\n\
         \x20 Last proof tick: {}",
        if active != 0 { "yes" } else { "no" },
        tasks,
        stats.proofs_submitted,
        stats.proofs_accepted,
        stats.total_rewards_wei,
        pow,
        stats.session_start,
        stats.last_proof_tick,
    )
}

/// Handle the `qfc-wallet` shell command.
pub fn cmd_wallet(args: &str) -> String {
    let key_hex = args.trim();
    if key_hex.is_empty() {
        return String::from(
            "Usage: qfc-wallet <private_key_hex>\n\
             Derives and displays the wallet address from a private key."
        );
    }

    let key_bytes = crate::ed25519::from_hex(key_hex);
    if key_bytes.len() != 32 {
        return String::from("Error: private key must be 64 hex characters (32 bytes)");
    }

    let mut private_key = [0u8; 32];
    private_key.copy_from_slice(&key_bytes);

    let keypair = crate::ed25519::generate_keypair(&private_key);
    let wallet = derive_wallet_address(&private_key);

    format!(
        "QFC Wallet Info:\n\
         \x20 Private key: {}...\n\
         \x20 Public key:  {}\n\
         \x20 Address:     {}",
        &crate::ed25519::to_hex(&private_key[..8]),
        crate::ed25519::to_hex(&keypair.public.bytes),
        format_wallet(&wallet),
    )
}

/// Handle the `qfc-hash` shell command.
pub fn cmd_hash(args: &str) -> String {
    let data = args.trim();
    if data.is_empty() {
        return String::from("Usage: qfc-hash <data>\n  Compute BLAKE3 hash of the given string.");
    }

    let hash = crate::blake3::blake3_hash(data.as_bytes());
    format!("BLAKE3: {}", crate::blake3::blake3_hex(&hash))
}

/// Handle the `qfc-sign` shell command.
pub fn cmd_sign(args: &str) -> String {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() < 2 {
        return String::from(
            "Usage: qfc-sign <private_key_hex> <message>\n\
             Sign a message with Ed25519 and display the signature."
        );
    }

    let key_hex = parts[0].trim();
    let message = parts[1].trim();

    let key_bytes = crate::ed25519::from_hex(key_hex);
    if key_bytes.len() != 32 {
        return String::from("Error: private key must be 64 hex characters (32 bytes)");
    }

    let mut private_key = [0u8; 32];
    private_key.copy_from_slice(&key_bytes);

    let keypair = crate::ed25519::generate_keypair(&private_key);
    let sig = crate::ed25519::sign_message(&keypair, message.as_bytes());
    let valid = crate::ed25519::verify(&keypair.public, message.as_bytes(), &sig);

    format!(
        "Ed25519 Signature:\n\
         \x20 Message: \"{}\"\n\
         \x20 Public key: {}\n\
         \x20 Signature:  {}\n\
         \x20 Verified: {}",
        message,
        crate::ed25519::to_hex(&keypair.public.bytes),
        crate::ed25519::to_hex(&sig.bytes),
        valid,
    )
}

// ── Init / Info / Stats ─────────────────────────────────────────────

/// Initialize the QFC miner module.
pub fn init() {
    TASKS_FETCHED.store(0, Ordering::SeqCst);
    POW_HASHES.store(0, Ordering::SeqCst);
    MINING_ACTIVE.store(0, Ordering::SeqCst);
    let mut stats = EARNINGS.lock();
    stats.proofs_submitted = 0;
    stats.proofs_accepted = 0;
    stats.total_rewards_wei = 0;
    stats.session_start = 0;
    stats.last_proof_tick = 0;
}

/// Return information about the QFC miner module.
pub fn qfc_miner_info() -> String {
    format!(
        "QFC Blockchain Miner v0.1 — in-kernel inference mining\n\
         \x20 Supported task types: embedding, text_generation, classification\n\
         \x20 Proof: BLAKE3 hash + Ed25519 signature\n\
         \x20 PoW fallback: BLAKE3-based proof-of-work"
    )
}

/// Return current mining statistics as a formatted string.
pub fn qfc_miner_stats() -> String {
    cmd_status()
}
