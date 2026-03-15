/// BMP image parser, renderer, and screenshot creator for MerlionOS.
///
/// Supports parsing Windows BMP files (BITMAPINFOHEADER, 24-bit and 32-bit),
/// rendering decoded images to the framebuffer console, and capturing the
/// current framebuffer contents as a BMP file suitable for saving to disk.
///
/// BMP stores pixels in BGR byte order; this module handles the BGR <-> RGB
/// conversion required by the framebuffer (which uses 0xRRGGBB u32 pixels).

use alloc::vec;
use alloc::vec::Vec;
use crate::fbconsole;

// ---------------------------------------------------------------------------
// BMP file structures (little-endian, packed, matching the on-disk layout)
// ---------------------------------------------------------------------------

/// BMP file header — first 14 bytes of every .bmp file.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BmpFileHeader {
    /// Magic bytes: must be 0x42 0x4D ('BM').
    pub signature: [u8; 2],
    /// Total file size in bytes.
    pub file_size: u32,
    /// Reserved (application specific).
    pub reserved1: u16,
    /// Reserved (application specific).
    pub reserved2: u16,
    /// Byte offset from the beginning of the file to the pixel data.
    pub data_offset: u32,
}

/// BMP info header (BITMAPINFOHEADER) — 40 bytes following the file header.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct BmpInfoHeader {
    /// Size of this header (must be 40 for BITMAPINFOHEADER).
    pub header_size: u32,
    /// Image width in pixels.
    pub width: i32,
    /// Image height in pixels (positive = bottom-up, negative = top-down).
    pub height: i32,
    /// Number of color planes (must be 1).
    pub planes: u16,
    /// Bits per pixel (supported: 24 or 32).
    pub bpp: u16,
    /// Compression method (0 = BI_RGB, uncompressed).
    pub compression: u32,
    /// Size of the raw pixel data (may be 0 for BI_RGB).
    pub image_size: u32,
    /// Horizontal resolution in pixels per metre.
    pub x_ppm: i32,
    /// Vertical resolution in pixels per metre.
    pub y_ppm: i32,
    /// Number of palette colors (0 = default).
    pub colors_used: u32,
    /// Number of important colors (0 = all).
    pub colors_important: u32,
}

// ---------------------------------------------------------------------------
// Decoded image representation
// ---------------------------------------------------------------------------

/// A decoded BMP image stored as 32-bit RGBA pixels in top-down row order.
///
/// Each pixel is packed as `0x00RRGGBB` (alpha channel is unused and set to
/// zero), matching the format expected by [`fbconsole`].
pub struct BmpImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Bits per pixel of the source BMP (24 or 32).
    pub bpp: u16,
    /// Pixel data in 0x00RRGGBB format, row-major, top-down.
    pub pixels: Vec<u32>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur while parsing a BMP file.
#[derive(Debug)]
pub enum BmpError {
    /// The data slice is too short to contain valid headers.
    TooShort,
    /// The magic signature is not 'BM'.
    InvalidSignature,
    /// The info header size is not 40 (unsupported variant).
    UnsupportedHeader,
    /// Bits-per-pixel is neither 24 nor 32.
    UnsupportedBpp,
    /// Compression is not BI_RGB (uncompressed).
    CompressedNotSupported,
    /// The pixel data region extends beyond the supplied buffer.
    DataOutOfBounds,
}

// ---------------------------------------------------------------------------
// BGR <-> RGB helpers
// ---------------------------------------------------------------------------

/// Convert a BGR-ordered u32 (`0x00BBGGRR`) to RGB (`0x00RRGGBB`).
#[inline]
pub fn bgr_to_rgb(bgr: u32) -> u32 {
    let b = (bgr >> 16) & 0xFF;
    let g = (bgr >> 8) & 0xFF;
    let r = bgr & 0xFF;
    (r << 16) | (g << 8) | b
}

/// Convert an RGB-ordered u32 (`0x00RRGGBB`) to BGR (`0x00BBGGRR`).
#[inline]
pub fn rgb_to_bgr(rgb: u32) -> u32 {
    // The transformation is symmetric.
    bgr_to_rgb(rgb)
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a BMP file from a raw byte slice.
///
/// Returns a [`BmpImage`] with pixels in `0x00RRGGBB` format on success.
/// Only uncompressed 24-bit and 32-bit BMP files are supported.
pub fn parse_bmp(data: &[u8]) -> Result<BmpImage, BmpError> {
    // Validate minimum size for both headers (14 + 40 = 54 bytes).
    if data.len() < 54 {
        return Err(BmpError::TooShort);
    }

    // Safety: the data is long enough and the structs are packed, so
    // unaligned reads are acceptable on x86_64.
    let file_header = unsafe { &*(data.as_ptr() as *const BmpFileHeader) };
    let info_header = unsafe { &*(data.as_ptr().add(14) as *const BmpInfoHeader) };

    // Validate signature.
    if file_header.signature != [b'B', b'M'] {
        return Err(BmpError::InvalidSignature);
    }

    // We only support BITMAPINFOHEADER (40 bytes).
    if info_header.header_size != 40 {
        return Err(BmpError::UnsupportedHeader);
    }

    let bpp = info_header.bpp;
    if bpp != 24 && bpp != 32 {
        return Err(BmpError::UnsupportedBpp);
    }

    if info_header.compression != 0 {
        return Err(BmpError::CompressedNotSupported);
    }

    let width = info_header.width.unsigned_abs();
    let height = info_header.height.unsigned_abs();
    let top_down = info_header.height < 0;

    let bytes_per_pixel = (bpp / 8) as usize;
    // BMP rows are padded to 4-byte boundaries.
    let row_size = (width as usize * bytes_per_pixel + 3) & !3;

    let data_offset = file_header.data_offset as usize;
    let required = data_offset + row_size * height as usize;
    if data.len() < required {
        return Err(BmpError::DataOutOfBounds);
    }

    let pixel_count = (width as usize) * (height as usize);
    let mut pixels = vec![0u32; pixel_count];

    for y in 0..height as usize {
        // BMP default is bottom-up; compute source row accordingly.
        let src_y = if top_down { y } else { (height as usize) - 1 - y };
        let row_offset = data_offset + src_y * row_size;

        for x in 0..width as usize {
            let px_off = row_offset + x * bytes_per_pixel;
            // BMP stores pixels as B, G, R [, A].
            let b = data[px_off] as u32;
            let g = data[px_off + 1] as u32;
            let r = data[px_off + 2] as u32;
            // Convert to 0x00RRGGBB.
            pixels[y * width as usize + x] = (r << 16) | (g << 8) | b;
        }
    }

    Ok(BmpImage {
        width,
        height,
        bpp,
        pixels,
    })
}

// ---------------------------------------------------------------------------
// Framebuffer rendering
// ---------------------------------------------------------------------------

/// Render a [`BmpImage`] to the framebuffer console at position (`x`, `y`).
///
/// Pixels that fall outside the framebuffer bounds are silently clipped.
/// The image is written directly using [`fbconsole::FbConsole::put_pixel`]
/// semantics (32-bit `0x00RRGGBB` words).
pub fn render_to_framebuffer(image: &BmpImage, x: u32, y: u32) {
    let console = fbconsole::CONSOLE.lock();
    let fb = match console.fb() {
        Some(f) => f,
        None => return,
    };

    let fb_w = fb.width as u32;
    let fb_h = fb.height as u32;
    let stride = fb.stride as usize;
    let bpp = fb.bpp as usize;

    for iy in 0..image.height {
        let screen_y = y + iy;
        if screen_y >= fb_h {
            break;
        }
        for ix in 0..image.width {
            let screen_x = x + ix;
            if screen_x >= fb_w {
                break;
            }
            let color = image.pixels[(iy * image.width + ix) as usize];
            let offset = screen_y as usize * stride + screen_x as usize * bpp;
            unsafe {
                let ptr = (fb.addr as *mut u8).add(offset);
                (ptr as *mut u32).write_volatile(color);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BMP creation (for screenshots)
// ---------------------------------------------------------------------------

/// Create a BMP file in memory from raw 0x00RRGGBB pixel data.
///
/// The returned `Vec<u8>` is a complete, valid BMP file (24-bit, bottom-up,
/// uncompressed) that can be written directly to disk.
pub fn create_bmp(width: u32, height: u32, pixels: &[u32]) -> Vec<u8> {
    let row_size = (width as usize * 3 + 3) & !3; // 24-bit, 4-byte aligned
    let pixel_data_size = row_size * height as usize;
    let file_size = 54 + pixel_data_size;

    let mut buf = vec![0u8; file_size];

    // --- File header (14 bytes) ---
    buf[0] = b'B';
    buf[1] = b'M';
    write_u32_le(&mut buf, 2, file_size as u32);
    // reserved fields stay 0
    write_u32_le(&mut buf, 10, 54); // data offset

    // --- Info header (40 bytes) ---
    write_u32_le(&mut buf, 14, 40); // header size
    write_i32_le(&mut buf, 18, width as i32);
    write_i32_le(&mut buf, 22, height as i32); // positive = bottom-up
    write_u16_le(&mut buf, 26, 1); // planes
    write_u16_le(&mut buf, 28, 24); // bpp
    // compression, image_size, ppm, colors — all 0

    // --- Pixel data (bottom-up, BGR) ---
    for y in 0..height as usize {
        let src_y = (height as usize) - 1 - y; // bottom-up flip
        let row_off = 54 + y * row_size;
        for x in 0..width as usize {
            let rgb = pixels[src_y * width as usize + x];
            let r = ((rgb >> 16) & 0xFF) as u8;
            let g = ((rgb >> 8) & 0xFF) as u8;
            let b = (rgb & 0xFF) as u8;
            let px = row_off + x * 3;
            buf[px] = b;
            buf[px + 1] = g;
            buf[px + 2] = r;
        }
        // Padding bytes are already 0.
    }

    buf
}

// ---------------------------------------------------------------------------
// Screenshot
// ---------------------------------------------------------------------------

/// Capture the current framebuffer contents as a BMP file.
///
/// Reads every pixel from the active [`fbconsole`] framebuffer and encodes
/// it into a 24-bit BMP image. Returns `None` if no framebuffer is active.
pub fn screenshot() -> Option<Vec<u8>> {
    let console = fbconsole::CONSOLE.lock();
    let fb = match console.fb() {
        Some(f) => f,
        None => return None,
    };

    let width = fb.width;
    let height = fb.height;
    let stride = fb.stride as usize;
    let bpp = fb.bpp as usize;

    let pixel_count = width as usize * height as usize;
    let mut pixels = vec![0u32; pixel_count];

    for y in 0..height as usize {
        for x in 0..width as usize {
            let offset = y * stride + x * bpp;
            let color = unsafe {
                let ptr = (fb.addr as *const u8).add(offset);
                (ptr as *const u32).read_volatile()
            };
            // Framebuffer stores 0x00RRGGBB — keep as-is.
            pixels[y * width as usize + x] = color & 0x00FFFFFF;
        }
    }

    // Release the console lock before the (potentially large) encode step.
    drop(console);

    Some(create_bmp(width, height, &pixels))
}

// ---------------------------------------------------------------------------
// Little-endian write helpers
// ---------------------------------------------------------------------------

/// Write a u32 in little-endian to `buf` at `offset`.
#[inline]
fn write_u32_le(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

/// Write an i32 in little-endian to `buf` at `offset`.
#[inline]
fn write_i32_le(buf: &mut [u8], offset: usize, val: i32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

/// Write a u16 in little-endian to `buf` at `offset`.
#[inline]
fn write_u16_le(buf: &mut [u8], offset: usize, val: u16) {
    buf[offset..offset + 2].copy_from_slice(&val.to_le_bytes());
}
