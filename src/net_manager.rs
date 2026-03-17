/// Network manager GUI for MerlionOS.
/// Provides WiFi network list, connection management,
/// IP configuration, and network diagnostics.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Connection status
// ---------------------------------------------------------------------------

/// Overall network connection state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnState {
    Connected,
    Disconnected,
    Connecting,
}

impl ConnState {
    fn as_str(&self) -> &'static str {
        match self {
            ConnState::Connected => "Connected",
            ConnState::Disconnected => "Disconnected",
            ConnState::Connecting => "Connecting",
        }
    }
}

// ---------------------------------------------------------------------------
// WiFi security types
// ---------------------------------------------------------------------------

/// WiFi security mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WifiSecurity {
    Open,
    WPA2,
    WPA3,
}

impl WifiSecurity {
    fn as_str(&self) -> &'static str {
        match self {
            WifiSecurity::Open => "Open",
            WifiSecurity::WPA2 => "WPA2",
            WifiSecurity::WPA3 => "WPA3",
        }
    }
}

// ---------------------------------------------------------------------------
// IP configuration mode
// ---------------------------------------------------------------------------

/// IP configuration method.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IpMode {
    Dhcp,
    Static,
}

impl IpMode {
    fn as_str(&self) -> &'static str {
        match self {
            IpMode::Dhcp => "DHCP",
            IpMode::Static => "Static",
        }
    }
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A scanned WiFi network entry.
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u8,          // 0-100 percentage
    pub security: WifiSecurity,
    pub channel: u8,
}

/// Saved connection profile.
pub struct ConnProfile {
    pub ssid: String,
    pub password: String,
    pub auto_connect: bool,
}

/// IP configuration for a network interface.
pub struct IpConfig {
    pub mode: IpMode,
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
}

impl IpConfig {
    fn new_dhcp() -> Self {
        Self {
            mode: IpMode::Dhcp,
            ip: [0; 4],
            mask: [255, 255, 255, 0],
            gateway: [0; 4],
            dns: [0; 4],
        }
    }

    fn format_ip(addr: &[u8; 4]) -> String {
        format!("{}.{}.{}.{}", addr[0], addr[1], addr[2], addr[3])
    }
}

/// Ethernet status.
pub struct EthernetStatus {
    pub link_up: bool,
    pub speed_mbps: u32,
    pub auto_negotiate: bool,
}

/// VPN tunnel (WireGuard).
pub struct VpnTunnel {
    pub name: String,
    pub endpoint: String,
    pub connected: bool,
}

/// Proxy settings.
pub struct ProxyConfig {
    pub http_proxy: String,
    pub socks5_proxy: String,
    pub no_proxy: Vec<String>,
}

impl ProxyConfig {
    const fn new() -> Self {
        Self {
            http_proxy: String::new(),
            socks5_proxy: String::new(),
            no_proxy: Vec::new(),
        }
    }
}

/// Diagnostic test result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiagResult {
    Pass,
    Fail,
    Skipped,
}

impl DiagResult {
    fn as_str(&self) -> &'static str {
        match self {
            DiagResult::Pass => "PASS",
            DiagResult::Fail => "FAIL",
            DiagResult::Skipped => "SKIP",
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CONNECTS: AtomicU32 = AtomicU32::new(0);
static DISCONNECTS: AtomicU32 = AtomicU32::new(0);
static SCANS: AtomicU32 = AtomicU32::new(0);
static DIAG_RUNS: AtomicU32 = AtomicU32::new(0);

struct NetManagerState {
    conn_state: ConnState,
    current_ssid: String,
    ip_config: IpConfig,
    wifi_networks: Vec<WifiNetwork>,
    profiles: Vec<ConnProfile>,
    ethernet: EthernetStatus,
    vpn_tunnels: Vec<VpnTunnel>,
    proxy: ProxyConfig,
    notifications: Vec<String>,
}

impl NetManagerState {
    const fn new() -> Self {
        Self {
            conn_state: ConnState::Disconnected,
            current_ssid: String::new(),
            ip_config: IpConfig {
                mode: IpMode::Dhcp,
                ip: [0; 4],
                mask: [255, 255, 255, 0],
                gateway: [0; 4],
                dns: [0; 4],
            },
            wifi_networks: Vec::new(),
            profiles: Vec::new(),
            ethernet: EthernetStatus {
                link_up: false,
                speed_mbps: 1000,
                auto_negotiate: true,
            },
            vpn_tunnels: Vec::new(),
            proxy: ProxyConfig::new(),
            notifications: Vec::new(),
        }
    }
}

static STATE: Mutex<NetManagerState> = Mutex::new(NetManagerState::new());

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn add_notification(st: &mut NetManagerState, msg: String) {
    if st.notifications.len() >= 64 {
        st.notifications.remove(0);
    }
    st.notifications.push(msg);
}

/// Sort wifi networks by signal strength (highest first).
fn sort_by_signal(nets: &mut Vec<WifiNetwork>) {
    nets.sort_by(|a, b| b.signal.cmp(&a.signal));
}

// ---------------------------------------------------------------------------
// WiFi operations
// ---------------------------------------------------------------------------

/// Scan for WiFi networks (simulated).
pub fn scan_wifi() -> usize {
    SCANS.fetch_add(1, Ordering::Relaxed);
    let mut st = STATE.lock();
    st.wifi_networks.clear();
    // Simulated scan results
    let nets: [(&str, u8, WifiSecurity, u8); 4] = [
        ("MerlionOS-5G", 85, WifiSecurity::WPA3, 36),
        ("SG-Public", 60, WifiSecurity::Open, 1),
        ("Office-Net", 72, WifiSecurity::WPA2, 6),
        ("IoT-Devices", 45, WifiSecurity::WPA2, 11),
    ];
    for (ssid, signal, sec, ch) in nets {
        st.wifi_networks.push(WifiNetwork {
            ssid: String::from(ssid),
            signal,
            security: sec,
            channel: ch,
        });
    }
    sort_by_signal(&mut st.wifi_networks);
    let count = st.wifi_networks.len();
    add_notification(&mut st, format!("WiFi scan complete: {} networks found", count));
    count
}

/// Connect to a WiFi network.
pub fn connect_wifi(ssid: &str, password: &str) -> bool {
    let mut st = STATE.lock();
    st.conn_state = ConnState::Connecting;
    // Check if network exists in scan results
    let found = st.wifi_networks.iter().any(|n| n.ssid == ssid);
    if !found {
        st.conn_state = ConnState::Disconnected;
        add_notification(&mut st, format!("WiFi: network '{}' not found", ssid));
        return false;
    }
    // Simulate connection
    st.conn_state = ConnState::Connected;
    st.current_ssid = String::from(ssid);
    st.ip_config = IpConfig::new_dhcp();
    st.ip_config.ip = [192, 168, 1, 100];
    st.ip_config.gateway = [192, 168, 1, 1];
    st.ip_config.dns = [8, 8, 8, 8];
    CONNECTS.fetch_add(1, Ordering::Relaxed);

    // Save profile if new
    let has_profile = st.profiles.iter().any(|p| p.ssid == ssid);
    if !has_profile {
        st.profiles.push(ConnProfile {
            ssid: String::from(ssid),
            password: String::from(password),
            auto_connect: true,
        });
    }

    add_notification(&mut st, format!("Connected to '{}'", ssid));
    true
}

/// Disconnect from current network.
pub fn disconnect() {
    let mut st = STATE.lock();
    if st.conn_state == ConnState::Connected {
        let ssid = st.current_ssid.clone();
        st.conn_state = ConnState::Disconnected;
        st.current_ssid.clear();
        st.ip_config.ip = [0; 4];
        st.ip_config.gateway = [0; 4];
        DISCONNECTS.fetch_add(1, Ordering::Relaxed);
        add_notification(&mut st, format!("Disconnected from '{}'", ssid));
    }
}

/// Reconnect to the last known network.
pub fn reconnect() -> bool {
    let ssid;
    let pass;
    {
        let st = STATE.lock();
        if let Some(prof) = st.profiles.last() {
            ssid = prof.ssid.clone();
            pass = prof.password.clone();
        } else {
            return false;
        }
    }
    connect_wifi(&ssid, &pass)
}

/// Quick connect helper — scan + connect in one call.
pub fn quick_connect(ssid: &str, password: &str) -> bool {
    scan_wifi();
    connect_wifi(ssid, password)
}

// ---------------------------------------------------------------------------
// IP configuration
// ---------------------------------------------------------------------------

/// Set static IP configuration.
pub fn set_static_ip(ip: [u8; 4], mask: [u8; 4], gw: [u8; 4], dns: [u8; 4]) {
    let mut st = STATE.lock();
    st.ip_config.mode = IpMode::Static;
    st.ip_config.ip = ip;
    st.ip_config.mask = mask;
    st.ip_config.gateway = gw;
    st.ip_config.dns = dns;
    add_notification(&mut st, format!("IP set to {}", IpConfig::format_ip(&ip)));
}

/// Switch to DHCP.
pub fn set_dhcp() {
    let mut st = STATE.lock();
    st.ip_config.mode = IpMode::Dhcp;
    add_notification(&mut st, String::from("Switched to DHCP"));
}

// ---------------------------------------------------------------------------
// VPN management
// ---------------------------------------------------------------------------

/// Add a WireGuard VPN tunnel.
pub fn add_vpn(name: &str, endpoint: &str) {
    let mut st = STATE.lock();
    st.vpn_tunnels.push(VpnTunnel {
        name: String::from(name),
        endpoint: String::from(endpoint),
        connected: false,
    });
}

/// Connect a VPN tunnel by name.
pub fn connect_vpn(name: &str) -> bool {
    let mut st = STATE.lock();
    for t in st.vpn_tunnels.iter_mut() {
        if t.name == name {
            t.connected = true;
            add_notification(&mut st, format!("VPN '{}' connected", name));
            return true;
        }
    }
    false
}

/// Disconnect a VPN tunnel by name.
pub fn disconnect_vpn(name: &str) -> bool {
    let mut st = STATE.lock();
    for t in st.vpn_tunnels.iter_mut() {
        if t.name == name {
            t.connected = false;
            add_notification(&mut st, format!("VPN '{}' disconnected", name));
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Proxy settings
// ---------------------------------------------------------------------------

/// Set HTTP proxy.
pub fn set_http_proxy(proxy: &str) {
    STATE.lock().proxy.http_proxy = String::from(proxy);
}

/// Set SOCKS5 proxy.
pub fn set_socks5_proxy(proxy: &str) {
    STATE.lock().proxy.socks5_proxy = String::from(proxy);
}

/// Add entry to no-proxy list.
pub fn add_no_proxy(host: &str) {
    STATE.lock().proxy.no_proxy.push(String::from(host));
}

// ---------------------------------------------------------------------------
// Network diagnostics
// ---------------------------------------------------------------------------

/// Run network diagnostics.
pub fn run_diagnostics() -> String {
    DIAG_RUNS.fetch_add(1, Ordering::Relaxed);
    let st = STATE.lock();
    let mut out = String::from("Network Diagnostics\n");
    out.push_str("═══════════════════\n");

    // Gateway ping test
    let gw_test = if st.conn_state == ConnState::Connected {
        DiagResult::Pass
    } else {
        DiagResult::Fail
    };
    out.push_str(&format!("  Gateway ping: {}\n", gw_test.as_str()));

    // DNS test
    let dns_test = if st.ip_config.dns != [0, 0, 0, 0] {
        DiagResult::Pass
    } else {
        DiagResult::Fail
    };
    out.push_str(&format!("  DNS resolve:  {}\n", dns_test.as_str()));

    // Internet check (HTTP probe)
    let inet_test = if st.conn_state == ConnState::Connected && st.ip_config.gateway != [0, 0, 0, 0] {
        DiagResult::Pass
    } else {
        DiagResult::Fail
    };
    out.push_str(&format!("  Internet:     {}\n", inet_test.as_str()));

    // Ethernet check
    let eth_test = if st.ethernet.link_up {
        DiagResult::Pass
    } else {
        DiagResult::Skipped
    };
    out.push_str(&format!("  Ethernet:     {}\n", eth_test.as_str()));

    out
}

// ---------------------------------------------------------------------------
// Display / info
// ---------------------------------------------------------------------------

/// Print the network manager dashboard.
fn print_dashboard(st: &NetManagerState) {
    crate::println!("Network Manager");
    crate::println!("═══════════════");
    crate::println!("  Status:  {}", st.conn_state.as_str());
    if st.conn_state == ConnState::Connected {
        crate::println!("  SSID:    {}", st.current_ssid);
        crate::println!("  IP:      {}", IpConfig::format_ip(&st.ip_config.ip));
        crate::println!("  Gateway: {}", IpConfig::format_ip(&st.ip_config.gateway));
        crate::println!("  DNS:     {}", IpConfig::format_ip(&st.ip_config.dns));
        crate::println!("  Mode:    {}", st.ip_config.mode.as_str());
    }
    crate::println!();

    // WiFi networks
    if !st.wifi_networks.is_empty() {
        crate::println!("WiFi Networks ({} found):", st.wifi_networks.len());
        crate::println!("  {:<20} {:>6} {:<6} {:>3}", "SSID", "Signal", "Sec", "Ch");
        crate::println!("  ──────────────────── ────── ────── ───");
        for net in &st.wifi_networks {
            crate::println!("  {:<20} {:>5}% {:<6} {:>3}",
                net.ssid, net.signal, net.security.as_str(), net.channel);
        }
        crate::println!();
    }

    // Ethernet
    crate::println!("Ethernet: {} ({}Mbps, auto-negotiate={})",
        if st.ethernet.link_up { "UP" } else { "DOWN" },
        st.ethernet.speed_mbps,
        if st.ethernet.auto_negotiate { "yes" } else { "no" });

    // VPN
    if !st.vpn_tunnels.is_empty() {
        crate::println!("VPN Tunnels:");
        for t in &st.vpn_tunnels {
            crate::println!("  {} -> {} [{}]", t.name, t.endpoint,
                if t.connected { "UP" } else { "DOWN" });
        }
    }

    // Proxy
    if !st.proxy.http_proxy.is_empty() || !st.proxy.socks5_proxy.is_empty() {
        crate::println!("Proxy:");
        if !st.proxy.http_proxy.is_empty() {
            crate::println!("  HTTP:   {}", st.proxy.http_proxy);
        }
        if !st.proxy.socks5_proxy.is_empty() {
            crate::println!("  SOCKS5: {}", st.proxy.socks5_proxy);
        }
        if !st.proxy.no_proxy.is_empty() {
            crate::println!("  No-proxy: {}", st.proxy.no_proxy.len());
        }
    }

    // Recent notifications
    if !st.notifications.is_empty() {
        let start = if st.notifications.len() > 5 {
            st.notifications.len() - 5
        } else {
            0
        };
        crate::println!("\nRecent events:");
        for n in &st.notifications[start..] {
            crate::println!("  * {}", n);
        }
    }
}

/// Return summary information string.
pub fn net_manager_info() -> String {
    let st = STATE.lock();
    let vpn_up = st.vpn_tunnels.iter().filter(|t| t.connected).count();
    format!(
        "NetMgr: state={} ssid={} ip={} profiles={} vpn={}/{} eth={}",
        st.conn_state.as_str(),
        if st.current_ssid.is_empty() { "(none)" } else { &st.current_ssid },
        IpConfig::format_ip(&st.ip_config.ip),
        st.profiles.len(),
        vpn_up,
        st.vpn_tunnels.len(),
        if st.ethernet.link_up { "up" } else { "down" },
    )
}

/// Return statistics string.
pub fn net_manager_stats() -> String {
    format!(
        "NetMgr Stats: connects={} disconnects={} scans={} diagnostics={}",
        CONNECTS.load(Ordering::Relaxed),
        DISCONNECTS.load(Ordering::Relaxed),
        SCANS.load(Ordering::Relaxed),
        DIAG_RUNS.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the network manager.
pub fn init() {
    let mut st = STATE.lock();
    st.ethernet.link_up = true;
    st.ethernet.speed_mbps = 1000;
    st.ethernet.auto_negotiate = true;
    // Add a default VPN tunnel
    st.vpn_tunnels.push(VpnTunnel {
        name: String::from("wg0"),
        endpoint: String::from("10.0.0.1:51820"),
        connected: false,
    });
    add_notification(&mut st, String::from("Network manager initialised"));
}

/// Show the network manager dashboard (shell command).
pub fn show() {
    scan_wifi();
    let st = STATE.lock();
    print_dashboard(&st);
}
