/// Advanced power management for MerlionOS.
/// CPU frequency scaling (P-states), C-states for idle, thermal management,
/// battery simulation, and power profiles.
///
/// All values are simulated — no real hardware access — but the interfaces
/// mirror what a real ACPI/OSPM implementation would expose, making this
/// useful for driver development and power-aware scheduling experiments.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ── CPU P-States (Performance States) ────────────────────────────────

/// A CPU performance state.
#[derive(Debug, Clone, Copy)]
pub struct PState {
    /// State identifier (0 = highest performance).
    pub id: u8,
    /// Frequency in MHz.
    pub freq_mhz: u32,
    /// Core voltage in millivolts.
    pub voltage_mv: u32,
    /// Typical power consumption in milliwatts.
    pub power_mw: u32,
}

/// Pre-defined P-states (P0 = max, P4 = min).
const PSTATES: [PState; 5] = [
    PState { id: 0, freq_mhz: 3600, voltage_mv: 1200, power_mw: 95000 },
    PState { id: 1, freq_mhz: 3000, voltage_mv: 1100, power_mw: 72000 },
    PState { id: 2, freq_mhz: 2400, voltage_mv: 1000, power_mw: 52000 },
    PState { id: 3, freq_mhz: 1800, voltage_mv: 900,  power_mw: 35000 },
    PState { id: 4, freq_mhz: 1200, voltage_mv: 800,  power_mw: 20000 },
];

static CURRENT_PSTATE: AtomicU32 = AtomicU32::new(2); // Start at P2 (Balanced)

/// Set the current P-state by id (0-4).
pub fn set_pstate(id: u8) -> Result<(), String> {
    if id as usize >= PSTATES.len() {
        return Err(format!("Invalid P-state: {}. Valid range: 0-{}", id, PSTATES.len() - 1));
    }
    // Check thermal throttle
    let temp = get_temperature();
    if id == 0 && temp >= THROTTLE_TEMP_C {
        return Err(format!("Thermal throttle active ({}C). Cannot enter P0.", temp));
    }
    CURRENT_PSTATE.store(id as u32, Ordering::Relaxed);
    PSTATE_CHANGES.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[power] P-state set to P{} ({}MHz, {}mV)",
        id, PSTATES[id as usize].freq_mhz, PSTATES[id as usize].voltage_mv);
    Ok(())
}

/// Get the current P-state.
pub fn get_pstate() -> PState {
    let id = CURRENT_PSTATE.load(Ordering::Relaxed) as usize;
    PSTATES[id.min(PSTATES.len() - 1)]
}

/// List all available P-states.
pub fn list_pstates() -> String {
    let current = CURRENT_PSTATE.load(Ordering::Relaxed) as usize;
    let mut out = String::from("=== CPU P-States ===\n");
    out.push_str("  ID   Freq(MHz)  Voltage(mV)  Power(mW)  Status\n");
    out.push_str("  ──   ─────────  ───────────  ─────────  ──────\n");
    for ps in &PSTATES {
        let marker = if ps.id as usize == current { " [active]" } else { "" };
        out.push_str(&format!(
            "  P{}   {:>5}      {:>5}        {:>5}    {}\n",
            ps.id, ps.freq_mhz, ps.voltage_mv, ps.power_mw, marker,
        ));
    }
    out
}

/// Auto-scale P-state based on simulated CPU load.
pub fn auto_scale() {
    let profile = get_profile();
    let load = SIMULATED_LOAD.load(Ordering::Relaxed);
    let temp = get_temperature();

    let target = if temp >= THROTTLE_TEMP_C {
        // Thermal throttle: force low P-state
        4u8
    } else {
        match profile {
            PowerProfile::Performance => {
                if load > 50 { 0 } else { 1 }
            }
            PowerProfile::Balanced => {
                if load > 80 { 0 }
                else if load > 50 { 1 }
                else if load > 20 { 2 }
                else { 3 }
            }
            PowerProfile::PowerSaver => {
                if load > 80 { 2 }
                else if load > 40 { 3 }
                else { 4 }
            }
            PowerProfile::Custom => {
                // Custom uses same logic as Balanced
                if load > 80 { 0 }
                else if load > 50 { 1 }
                else if load > 20 { 2 }
                else { 3 }
            }
        }
    };

    let current = CURRENT_PSTATE.load(Ordering::Relaxed) as u8;
    if target != current {
        let _ = set_pstate(target);
    }
}

// ── CPU C-States (Idle States) ───────────────────────────────────────

/// CPU idle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CState {
    /// C0: Active — CPU executing instructions.
    C0,
    /// C1: Halt — clock gated, instant wakeup.
    C1,
    /// C2: Stop-Clock — deeper idle, longer wakeup.
    C2,
    /// C3: Sleep — cache flushed, longest wakeup.
    C3,
}

impl CState {
    pub fn name(&self) -> &'static str {
        match self {
            CState::C0 => "C0 (Active)",
            CState::C1 => "C1 (Halt)",
            CState::C2 => "C2 (Stop-Clock)",
            CState::C3 => "C3 (Sleep)",
        }
    }

    pub fn wakeup_latency_us(&self) -> u32 {
        match self {
            CState::C0 => 0,
            CState::C1 => 1,
            CState::C2 => 100,
            CState::C3 => 1000,
        }
    }

    pub fn power_mw(&self) -> u32 {
        match self {
            CState::C0 => 0, // Depends on P-state
            CState::C1 => 5000,
            CState::C2 => 2000,
            CState::C3 => 500,
        }
    }
}

struct CStateStats {
    time_c0_ticks: u64,
    time_c1_ticks: u64,
    time_c2_ticks: u64,
    time_c3_ticks: u64,
    current: CState,
    transitions: u64,
}

impl CStateStats {
    const fn new() -> Self {
        Self {
            time_c0_ticks: 0,
            time_c1_ticks: 0,
            time_c2_ticks: 0,
            time_c3_ticks: 0,
            current: CState::C0,
            transitions: 0,
        }
    }

    fn enter(&mut self, state: CState) {
        if self.current != state {
            self.current = state;
            self.transitions += 1;
        }
    }

    fn tick(&mut self) {
        match self.current {
            CState::C0 => self.time_c0_ticks += 1,
            CState::C1 => self.time_c1_ticks += 1,
            CState::C2 => self.time_c2_ticks += 1,
            CState::C3 => self.time_c3_ticks += 1,
        }
    }
}

static CSTATE_STATS: Mutex<CStateStats> = Mutex::new(CStateStats::new());

/// Enter a C-state (called by idle loop).
pub fn enter_cstate(state: CState) {
    CSTATE_STATS.lock().enter(state);
}

/// Tick C-state counters (called from timer interrupt).
pub fn cstate_tick() {
    CSTATE_STATS.lock().tick();
}

/// Format C-state statistics.
pub fn cstate_info() -> String {
    let stats = CSTATE_STATS.lock();
    let total = stats.time_c0_ticks + stats.time_c1_ticks
        + stats.time_c2_ticks + stats.time_c3_ticks;
    let total = if total == 0 { 1 } else { total };

    let mut out = String::from("=== CPU C-State Statistics ===\n");
    out.push_str(&format!("Current state  : {}\n", stats.current.name()));
    out.push_str(&format!("Transitions    : {}\n", stats.transitions));
    out.push_str(&format!("C0 (Active)    : {} ticks ({}%)\n",
        stats.time_c0_ticks, stats.time_c0_ticks * 100 / total));
    out.push_str(&format!("C1 (Halt)      : {} ticks ({}%)\n",
        stats.time_c1_ticks, stats.time_c1_ticks * 100 / total));
    out.push_str(&format!("C2 (Stop-Clock): {} ticks ({}%)\n",
        stats.time_c2_ticks, stats.time_c2_ticks * 100 / total));
    out.push_str(&format!("C3 (Sleep)     : {} ticks ({}%)\n",
        stats.time_c3_ticks, stats.time_c3_ticks * 100 / total));
    out
}

// ── Power Profiles ───────────────────────────────────────────────────

/// System power profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    /// Maximum performance, highest power consumption.
    Performance,
    /// Balance between performance and power saving.
    Balanced,
    /// Minimal power consumption, reduced performance.
    PowerSaver,
    /// User-defined custom profile.
    Custom,
}

impl PowerProfile {
    pub fn name(&self) -> &'static str {
        match self {
            PowerProfile::Performance => "Performance",
            PowerProfile::Balanced => "Balanced",
            PowerProfile::PowerSaver => "PowerSaver",
            PowerProfile::Custom => "Custom",
        }
    }

    /// Default P-state for this profile.
    pub fn default_pstate(&self) -> u8 {
        match self {
            PowerProfile::Performance => 0,
            PowerProfile::Balanced => 2,
            PowerProfile::PowerSaver => 4,
            PowerProfile::Custom => 2,
        }
    }

    /// Screen dim timeout in seconds.
    pub fn screen_timeout_sec(&self) -> u32 {
        match self {
            PowerProfile::Performance => 600,
            PowerProfile::Balanced => 300,
            PowerProfile::PowerSaver => 120,
            PowerProfile::Custom => 300,
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "performance" | "perf" => Some(PowerProfile::Performance),
            "balanced" | "bal" => Some(PowerProfile::Balanced),
            "powersaver" | "saver" | "save" => Some(PowerProfile::PowerSaver),
            "custom" => Some(PowerProfile::Custom),
            _ => None,
        }
    }
}

static CURRENT_PROFILE: Mutex<PowerProfile> = Mutex::new(PowerProfile::Balanced);

/// Set the active power profile.
pub fn set_profile(profile: PowerProfile) {
    *CURRENT_PROFILE.lock() = profile;
    let _ = set_pstate(profile.default_pstate());
    PROFILE_CHANGES.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[power] Profile set to {}", profile.name());
}

/// Get the active power profile.
pub fn get_profile() -> PowerProfile {
    *CURRENT_PROFILE.lock()
}

/// List all profiles.
pub fn list_profiles() -> String {
    let current = get_profile();
    let mut out = String::from("=== Power Profiles ===\n");
    let profiles = [
        PowerProfile::Performance,
        PowerProfile::Balanced,
        PowerProfile::PowerSaver,
        PowerProfile::Custom,
    ];
    for p in &profiles {
        let marker = if *p == current { " [active]" } else { "" };
        out.push_str(&format!(
            "  {:12} P-state=P{}  screen_timeout={}s{}\n",
            p.name(), p.default_pstate(), p.screen_timeout_sec(), marker,
        ));
    }
    out
}

// ── Thermal Management ──────────────────────────────────────────────

/// Temperature threshold for throttling (Celsius).
const THROTTLE_TEMP_C: u32 = 90;
/// Critical temperature — emergency shutdown.
const CRITICAL_TEMP_C: u32 = 105;
/// Base ambient temperature.
const AMBIENT_TEMP_C: u32 = 35;

static SIMULATED_LOAD: AtomicU32 = AtomicU32::new(20);
static THERMAL_THROTTLE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Get simulated CPU temperature based on load and P-state.
pub fn get_temperature() -> u32 {
    let load = SIMULATED_LOAD.load(Ordering::Relaxed);
    let pstate = get_pstate();
    // Temperature model: ambient + (load_contribution) + (freq_contribution)
    let load_contrib = load * 45 / 100; // 0-45C from load
    let freq_contrib = pstate.freq_mhz / 200; // ~6-18C from frequency
    AMBIENT_TEMP_C + load_contrib + freq_contrib
}

/// Set simulated CPU load (0-100).
pub fn set_simulated_load(load: u32) {
    SIMULATED_LOAD.store(load.min(100), Ordering::Relaxed);
}

/// Check for thermal throttling — called periodically.
pub fn thermal_check() {
    let temp = get_temperature();
    if temp >= CRITICAL_TEMP_C {
        crate::serial_println!("[power] CRITICAL TEMPERATURE {}C! Emergency P4!", temp);
        CURRENT_PSTATE.store(4, Ordering::Relaxed);
        THERMAL_THROTTLE_COUNT.fetch_add(1, Ordering::Relaxed);
    } else if temp >= THROTTLE_TEMP_C {
        let current = CURRENT_PSTATE.load(Ordering::Relaxed);
        if current < 3 {
            crate::serial_println!("[power] Thermal throttle at {}C, forcing P3+", temp);
            CURRENT_PSTATE.store(3, Ordering::Relaxed);
            THERMAL_THROTTLE_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Thermal information display.
pub fn thermal_info() -> String {
    let temp = get_temperature();
    let load = SIMULATED_LOAD.load(Ordering::Relaxed);
    let pstate = get_pstate();
    let throttles = THERMAL_THROTTLE_COUNT.load(Ordering::Relaxed);

    let status = if temp >= CRITICAL_TEMP_C {
        "CRITICAL"
    } else if temp >= THROTTLE_TEMP_C {
        "THROTTLED"
    } else if temp >= 75 {
        "WARM"
    } else {
        "NORMAL"
    };

    let mut out = String::from("=== Thermal Status ===\n");
    out.push_str(&format!("CPU Temperature  : {}C\n", temp));
    out.push_str(&format!("Status           : {}\n", status));
    out.push_str(&format!("CPU Load (sim)   : {}%\n", load));
    out.push_str(&format!("Current P-state  : P{} ({}MHz)\n", pstate.id, pstate.freq_mhz));
    out.push_str(&format!("Throttle temp    : {}C\n", THROTTLE_TEMP_C));
    out.push_str(&format!("Critical temp    : {}C\n", CRITICAL_TEMP_C));
    out.push_str(&format!("Ambient temp     : {}C\n", AMBIENT_TEMP_C));
    out.push_str(&format!("Throttle events  : {}\n", throttles));

    // Temperature bar
    let bar_len = (temp as usize).min(50);
    let bar_char = if temp >= THROTTLE_TEMP_C { "!" } else { "#" };
    let bar: String = (0..bar_len).map(|_| bar_char).collect::<String>();
    out.push_str(&format!("Temp bar         : [{}] {}C\n", bar, temp));

    out
}

// ── Battery Simulation ──────────────────────────────────────────────

/// Simulated battery state.
pub struct BatteryState {
    /// Remaining capacity (0-100).
    pub capacity_pct: u8,
    /// Whether the battery is charging.
    pub charging: bool,
    /// Battery voltage in millivolts.
    pub voltage_mv: u32,
    /// Current draw in milliamps (negative = discharging).
    pub current_ma: i32,
    /// Estimated time remaining in minutes.
    pub time_remaining_min: u32,
    /// Charge cycle count.
    pub cycle_count: u32,
    /// Battery health percentage.
    pub health_pct: u8,
    /// Total energy drained (mWh simulated).
    pub total_drained_mwh: u64,
}

impl BatteryState {
    const fn new() -> Self {
        Self {
            capacity_pct: 85,
            charging: false,
            voltage_mv: 11400,
            current_ma: -1500,
            time_remaining_min: 240,
            cycle_count: 127,
            health_pct: 94,
            total_drained_mwh: 0,
        }
    }

    /// Update voltage based on capacity.
    fn update_voltage(&mut self) {
        // Lithium-ion voltage curve approximation (integer math only)
        // Full: ~12600mV, Empty: ~9000mV
        self.voltage_mv = 9000 + (self.capacity_pct as u32) * 36;
    }

    /// Update time remaining estimate.
    fn update_time_remaining(&mut self) {
        if self.charging {
            // Time to full: estimate based on remaining capacity
            let remaining = 100u32.saturating_sub(self.capacity_pct as u32);
            // Assume ~2 hours for full charge at ~30%/hour
            self.time_remaining_min = remaining * 2;
        } else {
            // Time to empty: based on current draw and remaining capacity
            let current_abs = (self.current_ma.unsigned_abs()).max(1);
            // Battery capacity: ~50Wh = 50000mWh
            let remaining_mwh = (self.capacity_pct as u32) * 500;
            let power_mw = (current_abs * self.voltage_mv / 1000).max(1);
            self.time_remaining_min = remaining_mwh * 60 / power_mw;
        }
    }
}

static BATTERY: Mutex<BatteryState> = Mutex::new(BatteryState::new());

/// Get battery info display.
pub fn battery_info() -> String {
    let bat = BATTERY.lock();
    let status = if bat.charging { "Charging" } else { "Discharging" };

    let mut out = String::from("=== Battery Status ===\n");
    out.push_str(&format!("Status           : {}\n", status));
    out.push_str(&format!("Capacity         : {}%\n", bat.capacity_pct));
    out.push_str(&format!("Voltage          : {}.{}V\n", bat.voltage_mv / 1000, (bat.voltage_mv % 1000) / 100));
    out.push_str(&format!("Current          : {}mA\n", bat.current_ma));
    out.push_str(&format!("Time remaining   : {} min\n", bat.time_remaining_min));
    out.push_str(&format!("Cycle count      : {}\n", bat.cycle_count));
    out.push_str(&format!("Health           : {}%\n", bat.health_pct));
    out.push_str(&format!("Total drained    : {} mWh\n", bat.total_drained_mwh));

    // Capacity bar (50 chars wide)
    let filled = bat.capacity_pct as usize / 2;
    let empty = 50 - filled;
    let fill_str: String = (0..filled).map(|_| '#').collect();
    let empty_str: String = (0..empty).map(|_| '.').collect();
    out.push_str(&format!("                 : [{}{}] {}%\n", fill_str, empty_str, bat.capacity_pct));

    out
}

/// Set charging state.
pub fn set_charging(charging: bool) {
    let mut bat = BATTERY.lock();
    let was_charging = bat.charging;
    bat.charging = charging;
    if charging {
        bat.current_ma = 2000; // Charging current
    } else {
        bat.current_ma = -1500; // Discharge current
    }
    bat.update_time_remaining();

    if !was_charging && charging {
        log_acpi_event(AcpiEvent::AcAdapterPlug);
    } else if was_charging && !charging {
        log_acpi_event(AcpiEvent::AcAdapterUnplug);
    }
}

/// Simulate battery drain tick (called periodically).
pub fn drain_tick() {
    let mut bat = BATTERY.lock();
    let pstate = get_pstate();

    if bat.charging {
        // Charge: gain ~1% every few ticks
        if bat.capacity_pct < 100 {
            bat.capacity_pct = (bat.capacity_pct + 1).min(100);
            if bat.capacity_pct == 100 {
                bat.cycle_count += 1;
                // Degrade health slightly every 50 cycles
                if bat.cycle_count % 50 == 0 && bat.health_pct > 50 {
                    bat.health_pct -= 1;
                }
            }
        }
    } else {
        // Drain based on P-state power consumption
        let drain_rate = pstate.power_mw / 20000; // Rough scaling
        let drain = (drain_rate as u8).max(1);
        bat.capacity_pct = bat.capacity_pct.saturating_sub(drain);
        bat.total_drained_mwh += pstate.power_mw as u64 / 60;
    }

    bat.update_voltage();
    bat.update_time_remaining();
}

// ── ACPI Events ─────────────────────────────────────────────────────

/// ACPI event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiEvent {
    PowerButtonPress,
    LidClose,
    LidOpen,
    AcAdapterPlug,
    AcAdapterUnplug,
    ThermalTrip,
    SleepButtonPress,
    WakeUp,
}

impl AcpiEvent {
    pub fn name(&self) -> &'static str {
        match self {
            AcpiEvent::PowerButtonPress => "Power Button Press",
            AcpiEvent::LidClose => "Lid Close",
            AcpiEvent::LidOpen => "Lid Open",
            AcpiEvent::AcAdapterPlug => "AC Adapter Plugged In",
            AcpiEvent::AcAdapterUnplug => "AC Adapter Unplugged",
            AcpiEvent::ThermalTrip => "Thermal Trip",
            AcpiEvent::SleepButtonPress => "Sleep Button Press",
            AcpiEvent::WakeUp => "Wake Up",
        }
    }
}

/// ACPI event log entry.
struct AcpiEventRecord {
    event: AcpiEvent,
    tick: u64,
}

const MAX_ACPI_EVENTS: usize = 128;

struct AcpiEventLog {
    records: Vec<AcpiEventRecord>,
}

impl AcpiEventLog {
    const fn new() -> Self {
        Self { records: Vec::new() }
    }

    fn log(&mut self, event: AcpiEvent, tick: u64) {
        if self.records.len() >= MAX_ACPI_EVENTS {
            self.records.remove(0);
        }
        self.records.push(AcpiEventRecord { event, tick });
    }

    fn recent(&self, count: usize) -> &[AcpiEventRecord] {
        let start = if self.records.len() > count {
            self.records.len() - count
        } else {
            0
        };
        &self.records[start..]
    }
}

static ACPI_EVENTS: Mutex<AcpiEventLog> = Mutex::new(AcpiEventLog::new());

/// Log an ACPI event.
pub fn log_acpi_event(event: AcpiEvent) {
    let tick = crate::timer::ticks();
    ACPI_EVENTS.lock().log(event, tick);
    ACPI_EVENT_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[power] ACPI event: {}", event.name());
}

/// Format recent ACPI events.
pub fn acpi_event_log() -> String {
    let log = ACPI_EVENTS.lock();
    let records = log.recent(20);
    if records.is_empty() {
        return String::from("No ACPI events recorded.");
    }

    let mut out = String::from("=== ACPI Event Log ===\n");
    out.push_str("  TICK        EVENT\n");
    out.push_str("  ──────────  ─────\n");
    for rec in records {
        out.push_str(&format!("  {:>10}  {}\n", rec.tick, rec.event.name()));
    }
    out
}

// ── Energy Counters ─────────────────────────────────────────────────

/// Per-subsystem energy tracking.
struct EnergyCounters {
    cpu_mwh: u64,
    memory_mwh: u64,
    disk_mwh: u64,
    network_mwh: u64,
    display_mwh: u64,
    other_mwh: u64,
}

impl EnergyCounters {
    const fn new() -> Self {
        Self {
            cpu_mwh: 0,
            memory_mwh: 0,
            disk_mwh: 0,
            network_mwh: 0,
            display_mwh: 0,
            other_mwh: 0,
        }
    }

    fn total(&self) -> u64 {
        self.cpu_mwh + self.memory_mwh + self.disk_mwh
            + self.network_mwh + self.display_mwh + self.other_mwh
    }

    /// Tick energy counters based on current state.
    fn tick(&mut self) {
        let pstate = get_pstate();
        // CPU energy: based on P-state power
        self.cpu_mwh += pstate.power_mw as u64 / 3600;
        // Memory: relatively constant
        self.memory_mwh += 5;
        // Disk: periodic
        self.disk_mwh += 2;
        // Network: low
        self.network_mwh += 1;
        // Display: moderate
        self.display_mwh += 8;
        // Other
        self.other_mwh += 1;
    }
}

static ENERGY: Mutex<EnergyCounters> = Mutex::new(EnergyCounters::new());

/// Tick energy counters (called periodically).
pub fn energy_tick() {
    ENERGY.lock().tick();
}

/// Format energy counter information.
pub fn energy_info() -> String {
    let e = ENERGY.lock();
    let total = e.total().max(1);

    let mut out = String::from("=== Energy Consumption ===\n");
    out.push_str(&format!("Total energy     : {} mWh\n", total));
    out.push_str(&format!("  CPU            : {} mWh ({}%)\n", e.cpu_mwh, e.cpu_mwh * 100 / total));
    out.push_str(&format!("  Memory         : {} mWh ({}%)\n", e.memory_mwh, e.memory_mwh * 100 / total));
    out.push_str(&format!("  Disk           : {} mWh ({}%)\n", e.disk_mwh, e.disk_mwh * 100 / total));
    out.push_str(&format!("  Network        : {} mWh ({}%)\n", e.network_mwh, e.network_mwh * 100 / total));
    out.push_str(&format!("  Display        : {} mWh ({}%)\n", e.display_mwh, e.display_mwh * 100 / total));
    out.push_str(&format!("  Other          : {} mWh ({}%)\n", e.other_mwh, e.other_mwh * 100 / total));
    out
}

// ── Global Statistics ───────────────────────────────────────────────

static PSTATE_CHANGES: AtomicU64 = AtomicU64::new(0);
static PROFILE_CHANGES: AtomicU64 = AtomicU64::new(0);
static ACPI_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);

// ── Init & Info ─────────────────────────────────────────────────────

/// Initialize power management subsystem.
pub fn init() {
    // Set default profile
    let profile = PowerProfile::Balanced;
    *CURRENT_PROFILE.lock() = profile;
    CURRENT_PSTATE.store(profile.default_pstate() as u32, Ordering::Relaxed);

    // Log initial ACPI event
    log_acpi_event(AcpiEvent::PowerButtonPress);

    // Initialize battery state
    {
        let mut bat = BATTERY.lock();
        bat.update_voltage();
        bat.update_time_remaining();
    }

    crate::serial_println!("[ok] Power management initialized (profile: {})", profile.name());
}

/// Overall power management info.
pub fn power_info() -> String {
    let profile = get_profile();
    let pstate = get_pstate();
    let temp = get_temperature();
    let load = SIMULATED_LOAD.load(Ordering::Relaxed);
    let bat = BATTERY.lock();

    let mut out = String::from("=== Power Management ===\n");
    out.push_str(&format!("Profile          : {}\n", profile.name()));
    out.push_str(&format!("CPU P-state      : P{} ({}MHz, {}mV, {}mW)\n",
        pstate.id, pstate.freq_mhz, pstate.voltage_mv, pstate.power_mw));
    out.push_str(&format!("CPU Load (sim)   : {}%\n", load));
    out.push_str(&format!("CPU Temperature  : {}C\n", temp));

    let thermal_status = if temp >= CRITICAL_TEMP_C {
        "CRITICAL"
    } else if temp >= THROTTLE_TEMP_C {
        "THROTTLED"
    } else {
        "OK"
    };
    out.push_str(&format!("Thermal status   : {}\n", thermal_status));

    let bat_status = if bat.charging { "Charging" } else { "On Battery" };
    out.push_str(&format!("Battery          : {}% ({})\n", bat.capacity_pct, bat_status));
    out.push_str(&format!("Battery voltage  : {}.{}V\n", bat.voltage_mv / 1000, (bat.voltage_mv % 1000) / 100));
    out.push_str(&format!("Time remaining   : {} min\n", bat.time_remaining_min));

    drop(bat);

    let energy_total = ENERGY.lock().total();
    out.push_str(&format!("Total energy     : {} mWh\n", energy_total));

    out
}

/// Power statistics.
pub fn power_stats() -> String {
    let pstate_changes = PSTATE_CHANGES.load(Ordering::Relaxed);
    let profile_changes = PROFILE_CHANGES.load(Ordering::Relaxed);
    let acpi_events = ACPI_EVENT_COUNT.load(Ordering::Relaxed);
    let throttles = THERMAL_THROTTLE_COUNT.load(Ordering::Relaxed);

    let bat = BATTERY.lock();
    let cycles = bat.cycle_count;
    let health = bat.health_pct;
    let drained = bat.total_drained_mwh;
    drop(bat);

    let cstats = CSTATE_STATS.lock();
    let c_transitions = cstats.transitions;
    drop(cstats);

    let energy_total = ENERGY.lock().total();

    let mut out = String::from("=== Power Statistics ===\n");
    out.push_str(&format!("P-state changes  : {}\n", pstate_changes));
    out.push_str(&format!("Profile changes  : {}\n", profile_changes));
    out.push_str(&format!("ACPI events      : {}\n", acpi_events));
    out.push_str(&format!("Thermal throttles: {}\n", throttles));
    out.push_str(&format!("C-state changes  : {}\n", c_transitions));
    out.push_str(&format!("Battery cycles   : {}\n", cycles));
    out.push_str(&format!("Battery health   : {}%\n", health));
    out.push_str(&format!("Energy consumed  : {} mWh\n", energy_total));
    out.push_str(&format!("Energy drained   : {} mWh\n", drained));
    out
}
