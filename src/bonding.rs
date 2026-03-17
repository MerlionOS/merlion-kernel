/// Network bonding / link aggregation for MerlionOS.
/// Combines multiple network interfaces into a single logical bond
/// for increased bandwidth and/or fault tolerance.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of bonds.
const MAX_BONDS: usize = 16;

/// Maximum slave interfaces per bond.
const MAX_SLAVES: usize = 8;

/// MII monitoring interval in ticks (~100 Hz, so 100 = 1 second).
const DEFAULT_MII_INTERVAL: u64 = 100;

/// LACP timeout (long = 90 sec = 9000 ticks).
const LACP_LONG_TIMEOUT: u64 = 9000;

/// LACP timeout (short = 3 sec = 300 ticks).
const LACP_SHORT_TIMEOUT: u64 = 300;

// ---------------------------------------------------------------------------
// Bond mode
// ---------------------------------------------------------------------------

/// Bonding modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BondMode {
    /// Mode 0: Round-robin TX across slaves.
    BalanceRR,
    /// Mode 1: Only one active slave; failover on link loss.
    ActiveBackup,
    /// Mode 2: XOR hash on src+dst for TX slave selection.
    BalanceXor,
    /// Mode 4: IEEE 802.3ad LACP.
    Lacp802_3ad,
}

impl BondMode {
    pub fn name(&self) -> &'static str {
        match self {
            BondMode::BalanceRR => "balance-rr (0)",
            BondMode::ActiveBackup => "active-backup (1)",
            BondMode::BalanceXor => "balance-xor (2)",
            BondMode::Lacp802_3ad => "802.3ad (4)",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "rr" | "balance-rr" | "0" => Some(BondMode::BalanceRR),
            "ab" | "active-backup" | "1" => Some(BondMode::ActiveBackup),
            "xor" | "balance-xor" | "2" => Some(BondMode::BalanceXor),
            "lacp" | "802.3ad" | "4" => Some(BondMode::Lacp802_3ad),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Link / MII status
// ---------------------------------------------------------------------------

/// MII link status for a slave interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    Up,
    Down,
}

// ---------------------------------------------------------------------------
// LACP state (simplified)
// ---------------------------------------------------------------------------

/// Simplified LACP partner info.
#[derive(Debug, Clone)]
struct LacpInfo {
    /// Partner system priority.
    partner_priority: u16,
    /// Partner system MAC (6 bytes).
    partner_mac: [u8; 6],
    /// Partner port key.
    partner_key: u16,
    /// Last LACPDU received tick.
    last_rx_tick: u64,
    /// Whether aggregation is active.
    aggregated: bool,
}

impl LacpInfo {
    const fn new() -> Self {
        Self {
            partner_priority: 0,
            partner_mac: [0; 6],
            partner_key: 0,
            last_rx_tick: 0,
            aggregated: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Slave interface
// ---------------------------------------------------------------------------

/// A slave interface within a bond.
#[derive(Debug, Clone)]
struct BondSlave {
    /// Interface name.
    name: String,
    /// Current link status.
    link: LinkStatus,
    /// MAC address.
    mac: [u8; 6],
    /// TX packets through this slave.
    tx_packets: u64,
    /// RX packets through this slave.
    rx_packets: u64,
    /// TX bytes.
    tx_bytes: u64,
    /// RX bytes.
    rx_bytes: u64,
    /// LACP info (only used in 802.3ad mode).
    lacp: LacpInfo,
}

// ---------------------------------------------------------------------------
// Bond
// ---------------------------------------------------------------------------

/// A bond (logical aggregated interface).
struct Bond {
    /// Bond name (e.g. "bond0").
    name: String,
    /// Bonding mode.
    mode: BondMode,
    /// Slave interfaces.
    slaves: Vec<BondSlave>,
    /// Index of the currently active slave (for active-backup).
    active_slave: usize,
    /// Index of the primary slave (preferred active).
    primary_slave: Option<usize>,
    /// Round-robin counter for balance-rr.
    rr_counter: u64,
    /// MII monitoring interval (in ticks).
    mii_interval: u64,
    /// Last MII poll tick.
    last_mii_tick: u64,
    /// Gratuitous ARPs sent on failover.
    grat_arps_sent: u64,
    /// Total failovers.
    failovers: u64,
}

impl Bond {
    fn new(name: String, mode: BondMode) -> Self {
        Self {
            name,
            mode,
            slaves: Vec::new(),
            active_slave: 0,
            primary_slave: None,
            rr_counter: 0,
            mii_interval: DEFAULT_MII_INTERVAL,
            last_mii_tick: 0,
            grat_arps_sent: 0,
            failovers: 0,
        }
    }

    /// Select TX slave index based on bond mode and packet hash.
    fn select_tx_slave(&mut self, hash: u32) -> Option<usize> {
        let up_count = self.slaves.iter().filter(|s| s.link == LinkStatus::Up).count();
        if up_count == 0 {
            return None;
        }
        match self.mode {
            BondMode::ActiveBackup => {
                if self.active_slave < self.slaves.len()
                    && self.slaves[self.active_slave].link == LinkStatus::Up
                {
                    Some(self.active_slave)
                } else {
                    self.slaves.iter().position(|s| s.link == LinkStatus::Up)
                }
            }
            BondMode::BalanceRR => {
                self.rr_counter += 1;
                // Find the n-th UP slave
                let target = (self.rr_counter as usize) % up_count;
                let mut count = 0usize;
                for (i, s) in self.slaves.iter().enumerate() {
                    if s.link == LinkStatus::Up {
                        if count == target {
                            return Some(i);
                        }
                        count += 1;
                    }
                }
                None
            }
            BondMode::BalanceXor | BondMode::Lacp802_3ad => {
                let target = (hash as usize) % up_count;
                let mut count = 0usize;
                for (i, s) in self.slaves.iter().enumerate() {
                    if s.link == LinkStatus::Up {
                        if count == target {
                            return Some(i);
                        }
                        count += 1;
                    }
                }
                None
            }
        }
    }

    /// Perform failover: find a new active slave.
    fn failover(&mut self) {
        // Prefer primary if it's up
        if let Some(pri) = self.primary_slave {
            if pri < self.slaves.len() && self.slaves[pri].link == LinkStatus::Up {
                if self.active_slave != pri {
                    self.active_slave = pri;
                    self.failovers += 1;
                    self.grat_arps_sent += 1;
                }
                return;
            }
        }
        // Otherwise pick first up slave
        for (i, s) in self.slaves.iter().enumerate() {
            if s.link == LinkStatus::Up && i != self.active_slave {
                self.active_slave = i;
                self.failovers += 1;
                self.grat_arps_sent += 1;
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct BondManager {
    bonds: Vec<Bond>,
}

impl BondManager {
    const fn new() -> Self {
        Self { bonds: Vec::new() }
    }
}

static BOND_MANAGER: Mutex<BondManager> = Mutex::new(BondManager::new());
static BOND_TICK: AtomicU64 = AtomicU64::new(0);
static TOTAL_TX: AtomicU64 = AtomicU64::new(0);
static TOTAL_RX: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// TX hash helpers
// ---------------------------------------------------------------------------

/// Hash on src+dst MAC addresses (layer 2).
pub fn hash_l2(src_mac: &[u8; 6], dst_mac: &[u8; 6]) -> u32 {
    let mut h: u32 = 0;
    for &b in src_mac.iter().chain(dst_mac.iter()) {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    h
}

/// Hash on src+dst IP addresses (layer 3).
pub fn hash_l3(src_ip: &[u8; 4], dst_ip: &[u8; 4]) -> u32 {
    let mut h: u32 = 0;
    for &b in src_ip.iter().chain(dst_ip.iter()) {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    h
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new bond with the given name and mode.
pub fn create_bond(name: &str, mode: BondMode) -> Result<(), &'static str> {
    let mut mgr = BOND_MANAGER.lock();
    if mgr.bonds.len() >= MAX_BONDS {
        return Err("maximum bonds reached");
    }
    if mgr.bonds.iter().any(|b| b.name == name) {
        return Err("bond name already exists");
    }
    mgr.bonds.push(Bond::new(String::from(name), mode));
    crate::serial_println!("[bonding] created {} mode={}", name, mode.name());
    Ok(())
}

/// Add a slave interface to a bond.
pub fn add_slave(bond_name: &str, iface: &str, mac: [u8; 6]) -> Result<(), &'static str> {
    let mut mgr = BOND_MANAGER.lock();
    let bond = mgr.bonds.iter_mut().find(|b| b.name == bond_name)
        .ok_or("bond not found")?;
    if bond.slaves.len() >= MAX_SLAVES {
        return Err("slave limit reached");
    }
    if bond.slaves.iter().any(|s| s.name == iface) {
        return Err("interface already enslaved");
    }
    bond.slaves.push(BondSlave {
        name: String::from(iface),
        link: LinkStatus::Up,
        mac,
        tx_packets: 0,
        rx_packets: 0,
        tx_bytes: 0,
        rx_bytes: 0,
        lacp: LacpInfo::new(),
    });
    crate::serial_println!("[bonding] added {} to {}", iface, bond_name);
    Ok(())
}

/// Remove a slave interface from a bond.
pub fn remove_slave(bond_name: &str, iface: &str) -> Result<(), &'static str> {
    let mut mgr = BOND_MANAGER.lock();
    let bond = mgr.bonds.iter_mut().find(|b| b.name == bond_name)
        .ok_or("bond not found")?;
    let pos = bond.slaves.iter().position(|s| s.name == iface)
        .ok_or("interface not enslaved")?;
    bond.slaves.remove(pos);
    // Adjust active_slave if needed
    if bond.active_slave >= bond.slaves.len() && !bond.slaves.is_empty() {
        bond.active_slave = 0;
    }
    crate::serial_println!("[bonding] removed {} from {}", iface, bond_name);
    Ok(())
}

/// Set the primary slave (preferred active in active-backup mode).
pub fn set_primary(bond_name: &str, iface: &str) -> Result<(), &'static str> {
    let mut mgr = BOND_MANAGER.lock();
    let bond = mgr.bonds.iter_mut().find(|b| b.name == bond_name)
        .ok_or("bond not found")?;
    let pos = bond.slaves.iter().position(|s| s.name == iface)
        .ok_or("interface not enslaved")?;
    bond.primary_slave = Some(pos);
    crate::serial_println!("[bonding] primary for {} set to {}", bond_name, iface);
    Ok(())
}

/// Update link status for a slave interface (MII monitoring callback).
pub fn update_link(bond_name: &str, iface: &str, status: LinkStatus) -> Result<(), &'static str> {
    let mut mgr = BOND_MANAGER.lock();
    let bond = mgr.bonds.iter_mut().find(|b| b.name == bond_name)
        .ok_or("bond not found")?;
    let slave = bond.slaves.iter_mut().find(|s| s.name == iface)
        .ok_or("interface not enslaved")?;
    let old = slave.link;
    slave.link = status;
    if old == LinkStatus::Up && status == LinkStatus::Down {
        crate::serial_println!("[bonding] {}: {} link DOWN, triggering failover", bond_name, iface);
        bond.failover();
    }
    Ok(())
}

/// Process LACPDU from partner (simplified).
pub fn process_lacpdu(
    bond_name: &str,
    iface: &str,
    partner_priority: u16,
    partner_mac: [u8; 6],
    partner_key: u16,
) -> Result<(), &'static str> {
    let mut mgr = BOND_MANAGER.lock();
    let bond = mgr.bonds.iter_mut().find(|b| b.name == bond_name)
        .ok_or("bond not found")?;
    if bond.mode != BondMode::Lacp802_3ad {
        return Err("bond is not in 802.3ad mode");
    }
    let slave = bond.slaves.iter_mut().find(|s| s.name == iface)
        .ok_or("interface not enslaved")?;
    let now = BOND_TICK.load(Ordering::Relaxed);
    slave.lacp.partner_priority = partner_priority;
    slave.lacp.partner_mac = partner_mac;
    slave.lacp.partner_key = partner_key;
    slave.lacp.last_rx_tick = now;
    slave.lacp.aggregated = true;
    Ok(())
}

/// Check LACP timeouts and de-aggregate expired partners.
pub fn lacp_check_timeouts() {
    let mut mgr = BOND_MANAGER.lock();
    let now = BOND_TICK.load(Ordering::Relaxed);
    for bond in mgr.bonds.iter_mut() {
        if bond.mode != BondMode::Lacp802_3ad {
            continue;
        }
        for slave in bond.slaves.iter_mut() {
            if slave.lacp.aggregated
                && now.saturating_sub(slave.lacp.last_rx_tick) > LACP_LONG_TIMEOUT
            {
                slave.lacp.aggregated = false;
                crate::serial_println!("[bonding] LACP timeout for {} on {}",
                    slave.name, bond.name);
            }
        }
    }
}

/// List all bonds.
pub fn list_bonds() -> String {
    let mgr = BOND_MANAGER.lock();
    if mgr.bonds.is_empty() {
        return String::from("No bonds configured.\n");
    }
    let mut out = String::from("Bond          Mode              Slaves   Active    Failovers\n");
    out.push_str("------------- ----------------  -------  --------  ---------\n");
    for bond in &mgr.bonds {
        let active_name = if bond.active_slave < bond.slaves.len() {
            bond.slaves[bond.active_slave].name.as_str()
        } else {
            "-"
        };
        out.push_str(&format!("{:<13} {:<18} {:<8} {:<9} {}\n",
            bond.name, bond.mode.name(),
            bond.slaves.len(), active_name, bond.failovers));
    }
    out
}

/// Get detailed info for a specific bond.
pub fn bond_info(name: &str) -> String {
    let mgr = BOND_MANAGER.lock();
    let bond = match mgr.bonds.iter().find(|b| b.name == name) {
        Some(b) => b,
        None => return format!("bond '{}' not found\n", name),
    };
    let mut out = format!("Bond: {}\n", bond.name);
    out.push_str(&format!("  Mode: {}\n", bond.mode.name()));
    out.push_str(&format!("  MII interval: {} ticks\n", bond.mii_interval));
    out.push_str(&format!("  Active slave: {}\n",
        if bond.active_slave < bond.slaves.len() {
            bond.slaves[bond.active_slave].name.as_str()
        } else { "none" }));
    if let Some(pri) = bond.primary_slave {
        if pri < bond.slaves.len() {
            out.push_str(&format!("  Primary: {}\n", bond.slaves[pri].name));
        }
    }
    out.push_str(&format!("  Failovers: {}\n", bond.failovers));
    out.push_str(&format!("  Gratuitous ARPs: {}\n", bond.grat_arps_sent));
    out.push_str(&format!("  Slaves ({}):\n", bond.slaves.len()));
    for s in &bond.slaves {
        out.push_str(&format!("    {} link={:?} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} tx={} rx={}\n",
            s.name, s.link,
            s.mac[0], s.mac[1], s.mac[2], s.mac[3], s.mac[4], s.mac[5],
            s.tx_packets, s.rx_packets));
        if bond.mode == BondMode::Lacp802_3ad {
            out.push_str(&format!("      LACP: aggregated={} partner_key={} partner_pri={}\n",
                s.lacp.aggregated, s.lacp.partner_key, s.lacp.partner_priority));
        }
    }
    out
}

/// Global bond statistics.
pub fn bond_stats() -> String {
    let mgr = BOND_MANAGER.lock();
    let mut out = format!("Bond statistics\n");
    out.push_str(&format!("  Total bonds: {}\n", mgr.bonds.len()));
    out.push_str(&format!("  Total TX: {}\n", TOTAL_TX.load(Ordering::Relaxed)));
    out.push_str(&format!("  Total RX: {}\n", TOTAL_RX.load(Ordering::Relaxed)));
    for bond in &mgr.bonds {
        let total_tx: u64 = bond.slaves.iter().map(|s| s.tx_packets).sum();
        let total_rx: u64 = bond.slaves.iter().map(|s| s.rx_packets).sum();
        out.push_str(&format!("  {}: tx={} rx={} failovers={}\n",
            bond.name, total_tx, total_rx, bond.failovers));
    }
    out
}

/// Update tick counter (call from timer).
pub fn tick(now: u64) {
    BOND_TICK.store(now, Ordering::Relaxed);
}

/// Initialise the bonding subsystem.
pub fn init() {
    crate::serial_println!("[bonding] link aggregation subsystem initialised");
}
