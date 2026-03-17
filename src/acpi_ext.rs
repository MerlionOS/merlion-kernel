/// Extended ACPI power management for MerlionOS.
/// Implements S3 sleep/wake, CPU frequency scaling via ACPI,
/// battery status reading, and lid switch handling.

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// --- Statistics ---
static SLEEP_COUNT: AtomicU64 = AtomicU64::new(0);
static WAKE_COUNT: AtomicU64 = AtomicU64::new(0);
static FREQ_CHANGES: AtomicU64 = AtomicU64::new(0);
static BATTERY_POLLS: AtomicU64 = AtomicU64::new(0);
static THERMAL_POLLS: AtomicU64 = AtomicU64::new(0);

// --- ACPI Sleep (S3) ---

/// Saved CPU context for S3 resume.
#[derive(Debug, Clone, Copy)]
pub struct CpuSleepContext {
    pub rsp: u64,
    pub rbp: u64,
    pub rip: u64,
    pub cr3: u64,
    pub gdt_base: u64,
    pub gdt_limit: u16,
    pub idt_base: u64,
    pub idt_limit: u16,
    pub rflags: u64,
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

impl CpuSleepContext {
    const fn zero() -> Self {
        Self {
            rsp: 0, rbp: 0, rip: 0, cr3: 0,
            gdt_base: 0, gdt_limit: 0,
            idt_base: 0, idt_limit: 0,
            rflags: 0, rbx: 0,
            r12: 0, r13: 0, r14: 0, r15: 0,
        }
    }
}

static SLEEP_CONTEXT: Mutex<CpuSleepContext> = Mutex::new(CpuSleepContext::zero());
static SYSTEM_SLEEPING: AtomicBool = AtomicBool::new(false);

/// PM1a control register address (simulated).
const PM1A_CNT_ADDR: u16 = 0x0404;
/// Sleep type for S3.
const SLP_TYP_S3: u16 = 0x05;
/// Sleep enable bit.
const SLP_EN: u16 = 1 << 13;

/// Enter S3 sleep state (simulated).
/// In real hardware: save CPU state, write SLP_TYP|SLP_EN to PM1a_CNT.
pub fn acpi_sleep() -> Result<(), &'static str> {
    if SYSTEM_SLEEPING.load(Ordering::SeqCst) {
        return Err("system is already sleeping");
    }

    crate::serial_println!("[acpi_ext] Preparing S3 sleep...");

    // Save CPU context (simulated values from current state)
    {
        let mut ctx = SLEEP_CONTEXT.lock();
        // In a real implementation, these would be read from actual registers
        ctx.rsp = 0xFFFF_8000_0000_0000;
        ctx.rbp = 0xFFFF_8000_0000_0000;
        ctx.rip = 0;
        ctx.cr3 = 0;
        ctx.gdt_base = 0;
        ctx.gdt_limit = 0;
        ctx.idt_base = 0;
        ctx.idt_limit = 0;
        ctx.rflags = 0x202; // IF set
        ctx.rbx = 0;
        ctx.r12 = 0;
        ctx.r13 = 0;
        ctx.r14 = 0;
        ctx.r15 = 0;
    }

    // Simulate writing to PM1a_CNT register
    let sleep_val = SLP_TYP_S3 | SLP_EN;
    crate::serial_println!("[acpi_ext] Writing {:#06x} to PM1a_CNT ({:#06x})", sleep_val, PM1A_CNT_ADDR);
    SYSTEM_SLEEPING.store(true, Ordering::SeqCst);
    SLEEP_COUNT.fetch_add(1, Ordering::Relaxed);

    // In real hardware, CPU would halt here and resume via firmware
    // For simulation, we immediately "wake"
    crate::serial_println!("[acpi_ext] S3 sleep simulated (immediate wake)");
    acpi_wake();
    Ok(())
}

/// Resume from S3 sleep (restore CPU context).
pub fn acpi_wake() {
    if !SYSTEM_SLEEPING.load(Ordering::SeqCst) {
        return;
    }
    crate::serial_println!("[acpi_ext] Resuming from S3...");

    // Restore CPU context (simulated)
    let _ctx = SLEEP_CONTEXT.lock();
    // In a real implementation: restore GDT, IDT, CR3, stack, registers

    SYSTEM_SLEEPING.store(false, Ordering::SeqCst);
    WAKE_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!("[acpi_ext] S3 wake complete");
}

// --- CPU Frequency via ACPI _PSS ---

/// A CPU Performance Supported State (_PSS entry).
#[derive(Debug, Clone, Copy)]
pub struct PssState {
    /// Core frequency in MHz.
    pub core_freq_mhz: u32,
    /// Power consumption in milliwatts.
    pub power_mw: u32,
    /// Transition latency in microseconds.
    pub transition_latency_us: u32,
    /// Control value (written to PERF_CTL MSR).
    pub control: u32,
    /// Status value (read from PERF_STATUS MSR).
    pub status: u32,
}

/// Pre-defined _PSS states (parsed from ACPI in real systems).
const PSS_STATES: [PssState; 6] = [
    PssState { core_freq_mhz: 4000, power_mw: 125000, transition_latency_us: 10, control: 0x2800, status: 0x2800 },
    PssState { core_freq_mhz: 3600, power_mw: 95000,  transition_latency_us: 10, control: 0x2400, status: 0x2400 },
    PssState { core_freq_mhz: 3000, power_mw: 72000,  transition_latency_us: 10, control: 0x1E00, status: 0x1E00 },
    PssState { core_freq_mhz: 2400, power_mw: 52000,  transition_latency_us: 10, control: 0x1800, status: 0x1800 },
    PssState { core_freq_mhz: 1800, power_mw: 35000,  transition_latency_us: 10, control: 0x1200, status: 0x1200 },
    PssState { core_freq_mhz: 1200, power_mw: 20000,  transition_latency_us: 10, control: 0x0C00, status: 0x0C00 },
];

static CURRENT_PSS: AtomicU32 = AtomicU32::new(2); // Start at state 2 (3000 MHz)

/// Set CPU frequency to the given _PSS state index.
pub fn set_cpu_freq(state_idx: u32) -> Result<(), &'static str> {
    if state_idx as usize >= PSS_STATES.len() {
        return Err("invalid _PSS state index");
    }
    // Check thermal constraint
    let temp = thermal_read();
    if state_idx == 0 && temp >= 850 {
        return Err("thermal limit: cannot enter highest frequency");
    }
    CURRENT_PSS.store(state_idx, Ordering::Relaxed);
    FREQ_CHANGES.fetch_add(1, Ordering::Relaxed);
    let state = &PSS_STATES[state_idx as usize];
    crate::serial_println!("[acpi_ext] CPU freq set to {} MHz (control={:#06x})",
        state.core_freq_mhz, state.control);
    Ok(())
}

/// Get the current CPU frequency in MHz.
pub fn get_cpu_freq() -> u32 {
    let idx = CURRENT_PSS.load(Ordering::Relaxed) as usize;
    if idx < PSS_STATES.len() { PSS_STATES[idx].core_freq_mhz } else { 0 }
}

/// List all available CPU frequency states.
pub fn list_cpu_freqs() -> String {
    let current = CURRENT_PSS.load(Ordering::Relaxed) as usize;
    let mut out = String::from("ACPI _PSS CPU Frequency States:\n");
    for (i, s) in PSS_STATES.iter().enumerate() {
        let marker = if i == current { " <-- active" } else { "" };
        out.push_str(&format!(
            "  [{}] {} MHz  {} mW  lat={}us  ctl={:#06x}{}\n",
            i, s.core_freq_mhz, s.power_mw, s.transition_latency_us, s.control, marker,
        ));
    }
    out
}

// --- Battery (ACPI _BIF / _BST) ---

/// Battery Information (_BIF).
#[derive(Debug, Clone, Copy)]
pub struct BatteryInfo {
    /// Design capacity in mWh.
    pub design_capacity_mwh: u32,
    /// Last full charge capacity in mWh.
    pub last_full_capacity_mwh: u32,
    /// Design voltage in mV.
    pub design_voltage_mv: u32,
    /// Battery technology: 0=non-rechargeable, 1=rechargeable.
    pub technology: u8,
    /// Cycle count.
    pub cycle_count: u32,
}

/// Battery Status (_BST).
#[derive(Debug, Clone, Copy)]
pub struct BatteryStatus {
    /// 0=not charging/discharging, 1=discharging, 2=charging.
    pub state: u32,
    /// Present discharge/charge rate in mW.
    pub present_rate_mw: u32,
    /// Remaining capacity in mWh.
    pub remaining_capacity_mwh: u32,
    /// Present voltage in mV.
    pub present_voltage_mv: u32,
}

/// Simulated battery info.
const BATTERY_BIF: BatteryInfo = BatteryInfo {
    design_capacity_mwh: 50000,
    last_full_capacity_mwh: 47500,
    design_voltage_mv: 11400,
    technology: 1,
    cycle_count: 142,
};

static BATTERY_REMAINING: AtomicU32 = AtomicU32::new(35625); // 75% of 47500
static BATTERY_STATE: AtomicU32 = AtomicU32::new(1); // 1 = discharging
static BATTERY_RATE: AtomicU32 = AtomicU32::new(15000); // 15W discharge

/// Low battery threshold (percent x 10).
const LOW_BATTERY_THRESHOLD: u32 = 100;  // 10%
/// Critical battery threshold (percent x 10).
const CRITICAL_BATTERY_THRESHOLD: u32 = 50;  // 5%

/// Read battery status.
pub fn battery_read() -> BatteryStatus {
    BATTERY_POLLS.fetch_add(1, Ordering::Relaxed);
    BatteryStatus {
        state: BATTERY_STATE.load(Ordering::Relaxed),
        present_rate_mw: BATTERY_RATE.load(Ordering::Relaxed),
        remaining_capacity_mwh: BATTERY_REMAINING.load(Ordering::Relaxed),
        present_voltage_mv: 11200,
    }
}

/// Get battery percentage (0-100).
pub fn battery_percent() -> u8 {
    let remaining = BATTERY_REMAINING.load(Ordering::Relaxed) as u64;
    let full = BATTERY_BIF.last_full_capacity_mwh as u64;
    if full == 0 { return 0; }
    let pct = remaining * 100 / full;
    if pct > 100 { 100 } else { pct as u8 }
}

/// Detailed battery information string.
pub fn battery_detail() -> String {
    let bst = battery_read();
    let pct = battery_percent();
    let state_str = match bst.state {
        0 => "idle",
        1 => "discharging",
        2 => "charging",
        _ => "unknown",
    };
    // Estimate time remaining: remaining_mwh / rate_mw * 60 = minutes
    let remaining_min = if bst.present_rate_mw > 0 {
        bst.remaining_capacity_mwh as u64 * 60 / bst.present_rate_mw as u64
    } else {
        0
    };
    let hours = remaining_min / 60;
    let mins = remaining_min % 60;

    let pct_x10 = pct as u32 * 10;
    let warning = if pct_x10 <= CRITICAL_BATTERY_THRESHOLD {
        " *** CRITICAL ***"
    } else if pct_x10 <= LOW_BATTERY_THRESHOLD {
        " * LOW *"
    } else {
        ""
    };

    format!(
        "Battery Status:\n\
         State:            {}{}\n\
         Charge:           {}%\n\
         Remaining:        {} mWh / {} mWh\n\
         Rate:             {} mW\n\
         Voltage:          {} mV\n\
         Time remaining:   {}h {}m\n\
         Design capacity:  {} mWh\n\
         Design voltage:   {} mV\n\
         Technology:       {}\n\
         Cycle count:      {}",
        state_str, warning,
        pct,
        bst.remaining_capacity_mwh, BATTERY_BIF.last_full_capacity_mwh,
        bst.present_rate_mw,
        bst.present_voltage_mv,
        hours, mins,
        BATTERY_BIF.design_capacity_mwh,
        BATTERY_BIF.design_voltage_mv,
        if BATTERY_BIF.technology == 1 { "Li-ion (rechargeable)" } else { "non-rechargeable" },
        BATTERY_BIF.cycle_count,
    )
}

// --- Lid Switch ---

static LID_OPEN: AtomicBool = AtomicBool::new(true);

/// Lid close action.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LidAction {
    Nothing,
    Suspend,
    Lock,
}

static LID_ACTION: AtomicU32 = AtomicU32::new(1); // 1 = Suspend

impl LidAction {
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => LidAction::Nothing,
            1 => LidAction::Suspend,
            2 => LidAction::Lock,
            _ => LidAction::Nothing,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LidAction::Nothing => "nothing",
            LidAction::Suspend => "suspend",
            LidAction::Lock => "lock",
        }
    }
}

/// Check if lid is open.
pub fn lid_is_open() -> bool {
    LID_OPEN.load(Ordering::Relaxed)
}

/// Set lid state (for ACPI event simulation).
pub fn set_lid_state(open: bool) {
    let was_open = LID_OPEN.swap(open, Ordering::SeqCst);
    if was_open && !open {
        // Lid just closed
        let action = LidAction::from_u32(LID_ACTION.load(Ordering::Relaxed));
        crate::serial_println!("[acpi_ext] Lid closed, action: {}", action.name());
        match action {
            LidAction::Suspend => { let _ = acpi_sleep(); }
            LidAction::Lock => { crate::serial_println!("[acpi_ext] Screen locked"); }
            LidAction::Nothing => {}
        }
    } else if !was_open && open {
        crate::serial_println!("[acpi_ext] Lid opened");
    }
}

/// Configure lid close action.
pub fn set_lid_action(action: LidAction) {
    LID_ACTION.store(action as u32, Ordering::Relaxed);
}

/// Lid status string.
pub fn lid_status() -> String {
    let open = lid_is_open();
    let action = LidAction::from_u32(LID_ACTION.load(Ordering::Relaxed));
    format!(
        "Lid Status:\n\
         State:       {}\n\
         Close action: {}",
        if open { "open" } else { "closed" },
        action.name(),
    )
}

// --- Thermal Zone ---

/// Simulated temperature in deci-Celsius (e.g., 450 = 45.0 C).
static THERMAL_TEMP: AtomicU32 = AtomicU32::new(520); // 52.0 C

/// Passive cooling trip point (throttle) in deci-Celsius.
const THERMAL_PASSIVE: u32 = 850; // 85.0 C
/// Critical trip point (shutdown) in deci-Celsius.
const THERMAL_CRITICAL: u32 = 1050; // 105.0 C

/// Read thermal zone temperature in deci-Celsius.
pub fn thermal_read() -> i32 {
    THERMAL_POLLS.fetch_add(1, Ordering::Relaxed);
    THERMAL_TEMP.load(Ordering::Relaxed) as i32
}

/// Set simulated temperature (for testing).
pub fn thermal_set(deci_celsius: u32) {
    THERMAL_TEMP.store(deci_celsius, Ordering::Relaxed);
    if deci_celsius >= THERMAL_CRITICAL {
        crate::serial_println!("[acpi_ext] CRITICAL: temperature {}.{} C >= critical threshold!",
            deci_celsius / 10, deci_celsius % 10);
    } else if deci_celsius >= THERMAL_PASSIVE {
        crate::serial_println!("[acpi_ext] WARNING: temperature {}.{} C >= passive threshold, throttling",
            deci_celsius / 10, deci_celsius % 10);
    }
}

/// Detailed thermal info string.
pub fn thermal_detail() -> String {
    let temp = thermal_read() as u32;
    let status = if temp >= THERMAL_CRITICAL {
        "CRITICAL"
    } else if temp >= THERMAL_PASSIVE {
        "THROTTLING"
    } else {
        "normal"
    };
    format!(
        "Thermal Zone:\n\
         Temperature:     {}.{} C\n\
         Status:          {}\n\
         Passive trip:    {}.{} C\n\
         Critical trip:   {}.{} C",
        temp / 10, temp % 10,
        status,
        THERMAL_PASSIVE / 10, THERMAL_PASSIVE % 10,
        THERMAL_CRITICAL / 10, THERMAL_CRITICAL % 10,
    )
}

// --- AC Adapter ---

static AC_ONLINE: AtomicBool = AtomicBool::new(false);

/// Check if AC adapter is connected.
pub fn ac_is_online() -> bool {
    AC_ONLINE.load(Ordering::Relaxed)
}

/// Simulate AC plug/unplug event.
pub fn set_ac_state(online: bool) {
    let was = AC_ONLINE.swap(online, Ordering::SeqCst);
    if !was && online {
        crate::serial_println!("[acpi_ext] AC adapter plugged in");
        BATTERY_STATE.store(2, Ordering::Relaxed); // charging
    } else if was && !online {
        crate::serial_println!("[acpi_ext] AC adapter unplugged");
        BATTERY_STATE.store(1, Ordering::Relaxed); // discharging
    }
}

// --- Global State & API ---
static INITIALIZED: spin::Once = spin::Once::new();

/// Initialize extended ACPI subsystem.
pub fn init() {
    INITIALIZED.call_once(|| {
        crate::serial_println!("[acpi_ext] Extended ACPI power management initialized");
        crate::serial_println!("[acpi_ext] CPU freq: {} MHz, thermal: {}.{} C",
            get_cpu_freq(),
            THERMAL_TEMP.load(Ordering::Relaxed) / 10,
            THERMAL_TEMP.load(Ordering::Relaxed) % 10);
    });
}

/// Enter S3 sleep (convenience API).
pub fn sleep() -> Result<(), &'static str> {
    acpi_sleep()
}

/// ACPI extended subsystem info.
pub fn acpi_ext_info() -> String {
    let freq = get_cpu_freq();
    let temp = thermal_read() as u32;
    let batt = battery_percent();
    let lid = if lid_is_open() { "open" } else { "closed" };
    let ac = if ac_is_online() { "online" } else { "offline" };
    let sleeping = SYSTEM_SLEEPING.load(Ordering::SeqCst);
    format!(
        "ACPI Extended Power Management:\n\
         Sleep state:     {}\n\
         CPU frequency:   {} MHz\n\
         Temperature:     {}.{} C\n\
         Battery:         {}%\n\
         AC adapter:      {}\n\
         Lid:             {}",
        if sleeping { "S3 (sleeping)" } else { "S0 (working)" },
        freq,
        temp / 10, temp % 10,
        batt,
        ac,
        lid,
    )
}

/// ACPI extended statistics.
pub fn acpi_ext_stats() -> String {
    format!(
        "ACPI Extended Statistics:\n\
         Sleep cycles:    {}\n\
         Wake cycles:     {}\n\
         Freq changes:    {}\n\
         Battery polls:   {}\n\
         Thermal polls:   {}",
        SLEEP_COUNT.load(Ordering::Relaxed),
        WAKE_COUNT.load(Ordering::Relaxed),
        FREQ_CHANGES.load(Ordering::Relaxed),
        BATTERY_POLLS.load(Ordering::Relaxed),
        THERMAL_POLLS.load(Ordering::Relaxed),
    )
}
