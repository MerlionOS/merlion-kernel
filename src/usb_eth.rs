/// USB CDC-ECM/NCM Ethernet adapter driver for MerlionOS.
///
/// Implements support for USB-to-Ethernet dongles using the Communications
/// Device Class (CDC) Ethernet Control Model (ECM) and Network Control Model
/// (NCM) as defined in the USB CDC specification. This enables networking on
/// real hardware via common USB Ethernet adapters.
///
/// # Supported chipsets (planned)
///
/// - **CDC-ECM generic** — standard class-compliant devices (most Linux-
///   compatible USB Ethernet adapters)
/// - **ASIX AX88179** — USB 3.0 Gigabit Ethernet (requires vendor-specific
///   init sequence beyond CDC-ECM)
/// - **Realtek RTL8152/RTL8153** — USB 2.0/3.0 Fast/Gigabit Ethernet
///   (vendor-specific control transfers needed alongside CDC-ECM)
///
/// Currently this driver scans for CDC-ECM class devices via xHCI-enumerated
/// USB ports and provides stub send/receive paths that integrate with the
/// kernel network stack.
///
/// # Architecture
///
/// ```text
///   netstack  ──►  usb_eth (this module)  ──►  xhci (USB transport)
/// ```
///
/// The driver registers itself with [`crate::driver`] and can serve as a NIC
/// backend option alongside e1000e and virtio-net in [`crate::netstack`].

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::{driver, net, serial_println, xhci};

// ---------------------------------------------------------------------------
// USB CDC class constants
// ---------------------------------------------------------------------------

/// USB base class: Communications and CDC Control (bDeviceClass / bInterfaceClass).
const CDC_CLASS: u8 = 0x02;

/// USB CDC subclass: Ethernet Control Model (ECM) per CDC 1.2 §3.8.
const CDC_SUBCLASS_ECM: u8 = 0x06;

/// USB CDC subclass: Network Control Model (NCM) per CDC 1.2 §3.15.
const CDC_SUBCLASS_NCM: u8 = 0x0D;

/// USB CDC Data Interface class (bInterfaceClass for the data pipe).
const CDC_DATA_CLASS: u8 = 0x0A;

/// CDC protocol: no class-specific protocol on the comm interface.
const CDC_PROTOCOL_NONE: u8 = 0x00;

/// CDC protocol: vendor-specific (used by ASIX, Realtek, etc.).
#[allow(dead_code)]
const CDC_PROTOCOL_VENDOR: u8 = 0xFF;

// ---------------------------------------------------------------------------
// CDC functional descriptor types
// ---------------------------------------------------------------------------

/// CS_INTERFACE descriptor type used by all CDC functional descriptors.
const CS_INTERFACE: u8 = 0x24;

/// CDC Header Functional Descriptor subtype.
///
/// Marks the beginning of the CDC functional descriptor chain and carries
/// the CDC specification version number (typically 1.10 or 1.20).
const CDC_FD_HEADER: u8 = 0x00;

/// CDC Union Functional Descriptor subtype.
///
/// Identifies the controlling (communication) interface and the
/// subordinate (data) interface that together form a single CDC function.
const CDC_FD_UNION: u8 = 0x06;

/// CDC Ethernet Networking Functional Descriptor subtype.
///
/// Carries the Ethernet-specific parameters: MAC address string index,
/// Ethernet statistics capabilities bitmap, maximum segment size, number
/// of multicast filters, and number of power filters.
const CDC_FD_ETHERNET: u8 = 0x0F;

// ---------------------------------------------------------------------------
// CDC Ethernet functional descriptor (parsed)
// ---------------------------------------------------------------------------

/// Parsed representation of a CDC Ethernet Networking Functional Descriptor.
///
/// This descriptor is found inside the Communication Interface's class-
/// specific descriptor chain and provides the host with essential Ethernet
/// parameters for the device.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct CdcEthernetDescriptor {
    /// Index of the string descriptor holding the MAC address (48-bit,
    /// encoded as 12 hex-ASCII characters).
    pub mac_string_index: u8,
    /// Bitmap indicating which Ethernet statistics the device collects.
    pub ethernet_statistics: u32,
    /// Maximum segment size the device can send/receive (typically 1514).
    pub max_segment_size: u16,
    /// Number of configurable multicast address filters.
    pub num_mc_filters: u16,
    /// Number of pattern (wake-up) filters.
    pub num_power_filters: u8,
}

/// Parsed representation of a CDC Header Functional Descriptor.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct CdcHeaderDescriptor {
    /// CDC specification version in BCD (e.g. 0x0110 = 1.10).
    pub bcd_cdc: u16,
}

/// Parsed representation of a CDC Union Functional Descriptor.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct CdcUnionDescriptor {
    /// Interface number of the controlling (communication) interface.
    pub control_interface: u8,
    /// Interface number of the first subordinate (data) interface.
    pub subordinate_interface: u8,
}

// ---------------------------------------------------------------------------
// Device state
// ---------------------------------------------------------------------------

/// Represents a discovered USB CDC-ECM/NCM Ethernet device.
///
/// Holds the device's MAC address, negotiated segment size, and the USB
/// endpoint addresses used for bulk data transfer. The xHCI layer handles
/// the actual USB transport; this struct tracks the logical device identity.
pub struct UsbEthDevice {
    /// 48-bit IEEE 802.3 MAC address (from CDC descriptor or generated).
    pub mac_address: [u8; 6],
    /// Maximum Ethernet segment size in bytes (header + payload, no FCS).
    /// Typically 1514 for standard Ethernet.
    pub max_segment_size: u16,
    /// USB bulk IN endpoint address (device-to-host data pipe).
    pub bulk_in_endpoint: u8,
    /// USB bulk OUT endpoint address (host-to-device data pipe).
    pub bulk_out_endpoint: u8,
    /// Whether the device was successfully identified on the USB bus.
    pub detected: bool,
    /// xHCI port number where the device is attached (1-based).
    pub port: u8,
    /// CDC subclass: ECM (0x06) or NCM (0x0D).
    pub subclass: u8,
}

impl UsbEthDevice {
    /// Create a new uninitialised device placeholder.
    const fn empty() -> Self {
        Self {
            mac_address: [0u8; 6],
            max_segment_size: 1514,
            bulk_in_endpoint: 0,
            bulk_out_endpoint: 0,
            detected: false,
            port: 0,
            subclass: 0,
        }
    }
}

/// Global device state, populated during [`init`].
static mut DEVICE: UsbEthDevice = UsbEthDevice::empty();

/// Whether a USB Ethernet device has been detected and initialised.
static DETECTED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Scan for USB CDC-ECM/NCM Ethernet devices via xHCI-enumerated ports.
///
/// Iterates over all USB devices discovered by [`crate::xhci`] and checks
/// whether any match the CDC Communications class with an ECM or NCM
/// subclass. If a candidate is found the driver reads (or generates) a
/// MAC address, assigns default bulk endpoint numbers, and registers the
/// driver with the kernel framework.
///
/// This should be called after [`crate::xhci::init()`] has completed port
/// enumeration.
pub fn init() {
    if !xhci::is_detected() {
        serial_println!("[usb_eth] xHCI not available, skipping USB Ethernet scan");
        return;
    }

    let devices = xhci::connected_devices();
    if devices.is_empty() {
        serial_println!("[usb_eth] no USB devices on bus, skipping CDC-ECM scan");
        return;
    }

    serial_println!("[usb_eth] scanning {} USB device(s) for CDC-ECM/NCM Ethernet...", devices.len());

    // Walk every connected port looking for a CDC Ethernet class device.
    // In a full implementation we would issue GET_DESCRIPTOR control transfers
    // to read the device and interface descriptors. For now we log each port
    // and attempt heuristic identification by speed (high/super-speed devices
    // on non-HID ports are likely Ethernet adapters in a typical setup).
    //
    // Known USB Ethernet chipsets and their device-level class codes:
    //   - CDC-ECM compliant:  class=0x02 subclass=0x06  (standard)
    //   - CDC-NCM compliant:  class=0x02 subclass=0x0D  (USB 3.0 Ethernet)
    //   - ASIX AX88179:       class=0xFF (vendor)  — needs vendor init
    //   - Realtek RTL8152:    class=0xFF (vendor)  — needs vendor init
    //   - Realtek RTL8153:    class=0xFF (vendor)  — needs vendor init

    let mut found = false;
    for &(port, speed) in &devices {
        serial_println!(
            "[usb_eth]   port {}: speed={} ({})",
            port, speed, xhci::speed_name(speed)
        );

        // High-speed (480 Mb/s) and super-speed (5 Gb/s) devices are
        // plausible USB Ethernet candidates. Low/full-speed are almost
        // certainly HID devices (keyboards, mice).
        if speed >= xhci::USB_SPEED_HIGH && !found {
            serial_println!(
                "[usb_eth]   port {}: candidate CDC-ECM Ethernet (speed qualifies)",
                port
            );

            // TODO: Issue USB control transfer GET_DESCRIPTOR(DEVICE) to
            // read bDeviceClass/bDeviceSubClass and confirm CDC-ECM/NCM.
            // TODO: Parse configuration descriptor to locate CDC functional
            // descriptors (Header, Union, Ethernet Networking) and extract
            // bulk IN/OUT endpoint addresses.

            let mac = read_mac();
            serial_println!(
                "[usb_eth]   MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
            );

            unsafe {
                DEVICE = UsbEthDevice {
                    mac_address: mac,
                    max_segment_size: 1514,
                    // Default CDC-ECM bulk endpoints (to be read from descriptors)
                    bulk_in_endpoint: 0x81,  // EP1 IN
                    bulk_out_endpoint: 0x02, // EP2 OUT
                    detected: true,
                    port,
                    subclass: CDC_SUBCLASS_ECM,
                };
            }

            found = true;
            DETECTED.store(true, Ordering::SeqCst);

            // Propagate MAC to the global network state so higher layers
            // (ARP, DHCP) use the correct source address.
            {
                let mut ns = net::NET.lock();
                ns.mac = net::MacAddr(mac);
            }

            serial_println!(
                "[usb_eth] USB Ethernet device configured on port {} (ECM, MSS=1514)",
                port
            );
        }
    }

    if found {
        driver::register("usb-cdc-eth", driver::DriverKind::Serial);
        serial_println!("[usb_eth] USB CDC Ethernet driver registered");
    } else {
        serial_println!("[usb_eth] no CDC-ECM/NCM Ethernet device found");
    }
}

// ---------------------------------------------------------------------------
// Transmit
// ---------------------------------------------------------------------------

/// Send an Ethernet frame through the USB CDC-ECM bulk OUT endpoint.
///
/// The caller provides a complete Ethernet frame (destination MAC through
/// payload; no FCS — the device appends it). The frame is handed to the
/// xHCI layer for a bulk OUT transfer on the configured endpoint.
///
/// # Errors
///
/// Returns `Err` if the device has not been detected, the frame exceeds
/// the maximum segment size, or the xHCI transfer could not be initiated.
///
/// # Current status
///
/// This is a stub implementation. It validates the frame, logs the
/// attempt, and will call into xHCI bulk transfer once that API is
/// available.
pub fn send_frame(frame: &[u8]) -> Result<(), &'static str> {
    if !DETECTED.load(Ordering::SeqCst) {
        return Err("usb_eth: device not detected");
    }

    if frame.is_empty() {
        return Err("usb_eth: empty frame");
    }

    let max_seg = unsafe { DEVICE.max_segment_size } as usize;
    if frame.len() > max_seg {
        return Err("usb_eth: frame exceeds max segment size");
    }

    let _ep_out = unsafe { DEVICE.bulk_out_endpoint };

    // TODO: Build a USB bulk OUT transfer TRB targeting `_ep_out` on the
    //       device's xHCI slot, copy `frame` into a DMA-accessible buffer,
    //       ring the endpoint doorbell, and poll for completion.
    //
    // Pseudocode:
    //   let slot = xhci::slot_for_port(DEVICE.port);
    //   xhci::bulk_out(slot, _ep_out, frame)?;

    serial_println!(
        "[usb_eth] TX: {} bytes via bulk OUT EP {:#04x} (stub)",
        frame.len(),
        _ep_out
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Receive
// ---------------------------------------------------------------------------

/// Poll the USB CDC-ECM bulk IN endpoint for a received Ethernet frame.
///
/// If a complete frame is pending in the xHCI transfer ring for the bulk
/// IN endpoint, it is copied out and returned. Otherwise returns `None`.
///
/// # Current status
///
/// This is a stub implementation. It checks the device state and will
/// poll the xHCI bulk IN transfer ring once that API is available.
pub fn recv_frame() -> Option<Vec<u8>> {
    if !DETECTED.load(Ordering::SeqCst) {
        return None;
    }

    let _ep_in = unsafe { DEVICE.bulk_in_endpoint };

    // TODO: Poll the xHCI transfer ring for bulk IN completions on `_ep_in`.
    //       If a completed TRB is found, copy the received data into a Vec
    //       and return it.
    //
    // Pseudocode:
    //   let slot = xhci::slot_for_port(DEVICE.port);
    //   if let Some(data) = xhci::bulk_in_poll(slot, _ep_in) {
    //       return Some(data);
    //   }

    None
}

// ---------------------------------------------------------------------------
// MAC address
// ---------------------------------------------------------------------------

/// Obtain the device's MAC address.
///
/// In a full implementation this reads the CDC Ethernet Networking
/// Functional Descriptor's `iMACAddress` string index and issues a
/// GET_DESCRIPTOR(STRING) control transfer to retrieve the 12 hex-ASCII
/// character MAC address.
///
/// As a fallback (or when descriptors are not yet parsed), a locally-
/// administered MAC address is generated from the kernel's tick counter
/// to ensure uniqueness across boots.
pub fn read_mac() -> [u8; 6] {
    // TODO: Issue GET_DESCRIPTOR(STRING, mac_string_index) via xHCI
    //       control transfer and parse the 12-character hex string.

    // Generate a locally-administered, unicast MAC address.
    // Byte 0 bit 1 = 1 (locally administered), bit 0 = 0 (unicast).
    // Use a simple hash of the TSC/tick counter for the variable octets.
    let seed = crate::timer::ticks() as u64;
    let hash = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);

    [
        0x02, // locally administered, unicast
        0x4D, // 'M' — MerlionOS
        ((hash >> 8) & 0xFF) as u8,
        ((hash >> 16) & 0xFF) as u8,
        ((hash >> 24) & 0xFF) as u8,
        ((hash >> 32) & 0xFF) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// Return a human-readable status string for the USB Ethernet driver.
///
/// Includes MAC address, segment size, endpoint assignments, and the xHCI
/// port where the device is attached.
pub fn info() -> String {
    if !DETECTED.load(Ordering::SeqCst) {
        return String::from("usb-eth: not detected");
    }

    let dev = unsafe { &*(&raw const DEVICE) };
    let sub = match dev.subclass {
        CDC_SUBCLASS_ECM => "ECM",
        CDC_SUBCLASS_NCM => "NCM",
        _ => "unknown",
    };

    format!(
        "usb-eth: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}  \
         type=CDC-{}  MSS={}  port={}  EP_IN={:#04x}  EP_OUT={:#04x}",
        dev.mac_address[0], dev.mac_address[1], dev.mac_address[2],
        dev.mac_address[3], dev.mac_address[4], dev.mac_address[5],
        sub, dev.max_segment_size, dev.port,
        dev.bulk_in_endpoint, dev.bulk_out_endpoint,
    )
}

/// Return whether a USB CDC Ethernet device has been detected.
pub fn is_detected() -> bool {
    DETECTED.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Descriptor parsing helpers (for future use)
// ---------------------------------------------------------------------------

/// Parse a CDC Header Functional Descriptor from raw bytes.
///
/// Expected layout (5 bytes):
///   [bLength, bDescriptorType(0x24), bDescriptorSubtype(0x00), bcdCDC_lo, bcdCDC_hi]
#[allow(dead_code)]
fn parse_header_descriptor(data: &[u8]) -> Option<CdcHeaderDescriptor> {
    if data.len() < 5 || data[1] != CS_INTERFACE || data[2] != CDC_FD_HEADER {
        return None;
    }
    Some(CdcHeaderDescriptor {
        bcd_cdc: u16::from_le_bytes([data[3], data[4]]),
    })
}

/// Parse a CDC Union Functional Descriptor from raw bytes.
///
/// Expected layout (5+ bytes):
///   [bLength, bDescriptorType(0x24), bDescriptorSubtype(0x06),
///    bControlInterface, bSubordinateInterface0, ...]
#[allow(dead_code)]
fn parse_union_descriptor(data: &[u8]) -> Option<CdcUnionDescriptor> {
    if data.len() < 5 || data[1] != CS_INTERFACE || data[2] != CDC_FD_UNION {
        return None;
    }
    Some(CdcUnionDescriptor {
        control_interface: data[3],
        subordinate_interface: data[4],
    })
}

/// Parse a CDC Ethernet Networking Functional Descriptor from raw bytes.
///
/// Expected layout (13 bytes):
///   [bLength, bDescriptorType(0x24), bDescriptorSubtype(0x0F),
///    iMACAddress, bmEthernetStatistics(4 bytes),
///    wMaxSegmentSize(2 bytes), wNumberMCFilters(2 bytes),
///    bNumberPowerFilters]
#[allow(dead_code)]
fn parse_ethernet_descriptor(data: &[u8]) -> Option<CdcEthernetDescriptor> {
    if data.len() < 13 || data[1] != CS_INTERFACE || data[2] != CDC_FD_ETHERNET {
        return None;
    }
    Some(CdcEthernetDescriptor {
        mac_string_index: data[3],
        ethernet_statistics: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
        max_segment_size: u16::from_le_bytes([data[8], data[9]]),
        num_mc_filters: u16::from_le_bytes([data[10], data[11]]),
        num_power_filters: data[12],
    })
}
