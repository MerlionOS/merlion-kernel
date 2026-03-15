/// CMOS Real-Time Clock (RTC) driver.
/// Reads date and time from the MC146818 RTC via I/O ports 0x70/0x71.

use x86_64::instructions::port::Port;

#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl core::fmt::Display for DateTime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// Read the current date and time from the CMOS RTC.
pub fn read() -> DateTime {
    // Wait for any RTC update to finish
    while is_update_in_progress() {}

    let mut second = read_cmos(0x00);
    let mut minute = read_cmos(0x02);
    let mut hour = read_cmos(0x04);
    let mut day = read_cmos(0x07);
    let mut month = read_cmos(0x08);
    let mut year = read_cmos(0x09);

    let register_b = read_cmos(0x0B);

    // Convert BCD to binary if needed (bit 2 of register B = 0 means BCD)
    if register_b & 0x04 == 0 {
        second = bcd_to_bin(second);
        minute = bcd_to_bin(minute);
        hour = bcd_to_bin(hour & 0x7F) | (hour & 0x80); // preserve AM/PM bit
        day = bcd_to_bin(day);
        month = bcd_to_bin(month);
        year = bcd_to_bin(year);
    }

    // Convert 12-hour to 24-hour if needed (bit 1 of register B = 0 means 12h)
    if register_b & 0x02 == 0 && hour & 0x80 != 0 {
        hour = ((hour & 0x7F) + 12) % 24;
    }

    // Assume 21st century
    let full_year = 2000u16 + year as u16;

    DateTime {
        year: full_year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

fn is_update_in_progress() -> bool {
    read_cmos(0x0A) & 0x80 != 0
}

fn read_cmos(register: u8) -> u8 {
    unsafe {
        let mut addr = Port::<u8>::new(0x70);
        let mut data = Port::<u8>::new(0x71);
        addr.write(register);
        data.read()
    }
}

fn bcd_to_bin(bcd: u8) -> u8 {
    (bcd & 0x0F) + ((bcd >> 4) * 10)
}
