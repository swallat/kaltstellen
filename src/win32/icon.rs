//! M2: the icon of an executable as pixels, for the main window.
//!
//! The icon is read with `SHDefExtractIconW`, the shell's default
//! extractor: it maps the file as data (`LOAD_LIBRARY_AS_DATAFILE`) and
//! reads its icon resource. No code of the file runs and no shell extension
//! is asked. The pixels come back as RGBA rows from the top; turning them
//! into an image for the page is safe code in [`crate::gui::icons`].

use std::path::Path;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC,
};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::UI::Shell::SHDefExtractIconW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::PCWSTR;

use super::{wide, wide_path};

/// Largest size asked for (a 20 px icon at 400 %, or zoomed in); the page
/// asks for its own size times the DPI scale, rounded up to a size icons
/// usually have. Bounds the memory of one icon to 36 KiB.
pub const MAX_SIZE: u32 = 96;
/// `GetDriveTypeW`: a fixed local disk.
const DRIVE_FIXED: u32 = 3;

/// An icon as RGBA, `width * height * 4` bytes, rows from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Owns an icon handle.
struct Icon(HICON);

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: owned and destroyed once.
        let _ = unsafe { DestroyIcon(self.0) };
    }
}

/// Owns a bitmap handle (may be null).
struct Bitmap(HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: owned and deleted once.
            let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
        }
    }
}

/// The screen DC, released on drop.
struct ScreenDc(windows::Win32::Graphics::Gdi::HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: from GetDC(None), released once.
        unsafe { ReleaseDC(None, self.0) };
    }
}

/// Whether `path` is `X:\…` on a fixed local disk. Icons are read on the
/// window thread: a network share (`\\server\…`, a mapped drive) could hang
/// the window until a time-out and would send credentials to that server,
/// and device paths (`\\?\`, `\\.\`) are not files to read icons from.
pub fn on_a_local_disk(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    let drive_letter = bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    if !drive_letter {
        return false;
    }
    let root = wide(&format!("{}:\\", char::from(bytes[0])));
    // SAFETY: `root` is NUL-terminated and outlives the call.
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED }
}

/// The first icon of `exe` at `size` pixels (the nearest size the file has,
/// scaled by the system). `None` if the file has no icon or cannot be read;
/// that is never an error for the caller.
pub fn of_file(exe: &Path, size: u32) -> Option<Pixels> {
    let size = size.clamp(16, MAX_SIZE);
    if !on_a_local_disk(exe) {
        return None;
    }
    let name = wide_path(exe);
    let mut handle = HICON::default();
    // SAFETY: `name` is NUL-terminated and outlives the call; one out
    // pointer for the large icon, none for the small one.
    let found = unsafe {
        SHDefExtractIconW(
            PCWSTR(name.as_ptr()),
            0,
            0,
            Some(&mut handle),
            None,
            size & 0xFFFF,
        )
    };
    // S_FALSE: the file has no icon.
    if found.is_err() || found.0 == 1 || handle.is_invalid() {
        return None;
    }
    pixels(&Icon(handle))
}

fn pixels(icon: &Icon) -> Option<Pixels> {
    let mut info = ICONINFO::default();
    // SAFETY: a valid icon and out pointer; the bitmaps become ours.
    unsafe { GetIconInfo(icon.0, &mut info) }.ok()?;
    let color = Bitmap(info.hbmColor);
    let mask = Bitmap(info.hbmMask);
    // A monochrome icon (no colour bitmap, AND and XOR masks in one
    // bitmap of double height) is rare in programs today: the page shows
    // its placeholder, which is never an error.
    if color.0.is_invalid() {
        return None;
    }
    let mut bitmap = BITMAP::default();
    // SAFETY: `bitmap` is writable for its size.
    let read = unsafe {
        GetObjectW(
            HGDIOBJ(color.0.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&raw mut bitmap).cast()),
        )
    };
    if read == 0 || bitmap.bmWidth <= 0 || bitmap.bmHeight <= 0 {
        return None;
    }
    let (width, height) = (bitmap.bmWidth as u32, bitmap.bmHeight as u32);
    if width > MAX_SIZE || height > MAX_SIZE {
        return None;
    }
    // SAFETY: the screen DC; released by the guard.
    let dc = ScreenDc(unsafe { GetDC(None) });
    let mut bgra = bits(&dc, &color, width, height)?;
    // Icons without an alpha channel take their shape from the mask.
    if bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        let shape = bits(&dc, &mask, width, height)?;
        for (pixel, m) in bgra
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(shape.as_chunks::<4>().0)
        {
            // Mask black (0) is opaque, white is transparent.
            pixel[3] = if m[0] == 0 { 255 } else { 0 };
        }
    }
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(Pixels {
        width,
        height,
        rgba: bgra,
    })
}

/// `bitmap` as 32-bit BGRA rows from the top.
fn bits(dc: &ScreenDc, bitmap: &Bitmap, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            // Negative: rows from the top.
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    let mut buffer = vec![0u8; (width * height * 4) as usize];
    // SAFETY: `buffer` holds `height` rows of `width` 32-bit pixels, as the
    // header describes; the bitmap is not selected into a DC.
    let lines = unsafe {
        GetDIBits(
            dc.0,
            bitmap.0,
            0,
            height,
            Some(buffer.as_mut_ptr().cast()),
            &mut header,
            DIB_RGB_COLORS,
        )
    };
    (lines == height as i32).then_some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m2_icon_of_a_system_program_and_none_without_one() {
        let windir = std::env::var_os("SystemRoot").expect("SystemRoot");
        let notepad = Path::new(&windir).join("System32").join("notepad.exe");
        for size in [16, 32, 48] {
            let icon = of_file(&notepad, size).expect("notepad has an icon");
            assert_eq!((icon.width, icon.height), (size, size));
            assert_eq!(icon.rgba.len(), (size * size * 4) as usize);
            assert!(icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
            assert!(icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 0));
        }
        // Out of range sizes are clamped.
        assert_eq!(of_file(&notepad, 1).unwrap().width, 16);
        assert_eq!(of_file(&notepad, 512).unwrap().width, MAX_SIZE);
        // Only files on a fixed local disk: no shares, no device paths.
        assert!(on_a_local_disk(&notepad));
        for elsewhere in [
            r"\\server\share\a.exe",
            r"\\?\C:\a.exe",
            r"\\.\pipe\x",
            "a.exe",
        ] {
            assert!(!on_a_local_disk(Path::new(elsewhere)), "{elsewhere}");
            assert!(of_file(Path::new(elsewhere), 32).is_none());
        }
        // No such file, and a file without an icon resource.
        assert!(of_file(Path::new(r"C:\no\such\program.exe"), 32).is_none());
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.exe");
        std::fs::write(&junk, b"MZ not really a program").unwrap();
        assert!(of_file(&junk, 32).is_none());
    }
}
