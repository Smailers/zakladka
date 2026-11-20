//! Значок трея (P 1105–1126): пиксели из `core::color::icon_pixels`.

use windows::Win32::Graphics::Gdi::CreateBitmap;
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

use crate::types::Rgb;
use crate::win::api::Api;
use crate::win::capture::{Dib, GdiObject, ScreenDc};

impl Api {
    /// HICON как число; освобождает вызывающий (DestroyIcon).
    pub fn make_icon(&self, size: i32, tint: Rgb) -> Option<isize> {
        if size <= 0 {
            return None;
        }
        let pixels = crate::core::color::icon_pixels(size as u32, tint);
        icon_from_pixels(size, &pixels)
    }
}

/// Значок `size × size` из пикселей BGRA (с альфой) сверху вниз. Растры освобождаются на всех
/// ветках: CreateIconIndirect делает свои копии.
fn icon_from_pixels(size: i32, pixels: &[u8]) -> Option<isize> {
    let byte_len = Dib::byte_len(size, size)?;
    let color = {
        let screen = ScreenDc::get()?;
        Dib::new(screen.0, size, size)?
    }; // DC экрана отпущен сразу, как в Python
    // SAFETY: в растр пишется не больше его длины `size × size × 4`.
    unsafe {
        std::ptr::copy_nonoverlapping(
            pixels.as_ptr(),
            color.bits as *mut u8,
            pixels.len().min(byte_len),
        );
    }
    // Монохромная маска из нулей: прозрачность задаёт альфа цветного растра.
    // Строки монохромного растра выровнены по 16 битам.
    let row_bytes = (size as usize).div_ceil(16) * 2;
    let mask_bits = vec![0u8; size as usize * row_bytes];
    // SAFETY: буфер маски нужной длины живёт до конца вызова.
    let mask = unsafe { CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast())) };
    if mask.is_invalid() {
        return None;
    }
    let mask_guard = GdiObject(mask.into());
    let info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color.bitmap,
    };
    // SAFETY: оба растра действительны; значок получает их копии.
    let icon = unsafe { CreateIconIndirect(&info) }.ok()?;
    drop(mask_guard);
    drop(color);
    Some(icon.0 as isize)
}

#[cfg(test)]
mod tests {
    use super::icon_from_pixels;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    /// Пиксели ленты-закладки, как `icon_pixels` в Python (P 590–603): подстановка, пока
    /// `core::color::icon_pixels` — заготовка части 1.
    fn ribbon(size: i32) -> Vec<u8> {
        let n = size as usize;
        let mut data = vec![0u8; n * n * 4];
        for y in 0..n {
            for x in 0..n {
                let (u, v) = ((x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64);
                let mut inside = (0.25..=0.75).contains(&u) && (0.06..=0.94).contains(&v);
                if inside && v > 0.66 && (u - 0.5).abs() < (v - 0.66) * 0.9 {
                    inside = false;
                }
                if inside {
                    let i = (y * n + x) * 4;
                    data[i..i + 4].copy_from_slice(&[0x00, 0x7A, 0xFF, 0xFF]);
                }
            }
        }
        data
    }

    #[test]
    fn icon_from_substitute_pixels_without_gdi_leaks() {
        let gdi = || unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
        let pixels = ribbon(32);
        // Прогрев: первый вызов может завести постоянные объекты системы.
        let first = icon_from_pixels(32, &pixels).expect("значок");
        unsafe { DestroyIcon(HICON(first as *mut _)).expect("DestroyIcon") };
        let before = gdi();
        for _ in 0..200 {
            let icon = icon_from_pixels(32, &pixels).expect("значок");
            unsafe { DestroyIcon(HICON(icon as *mut _)).expect("DestroyIcon") };
        }
        let after = gdi();
        // Утечка давала бы +400 (маска и цветной растр на каждый значок); допуск — на
        // параллельные тесты этого же процесса.
        assert!(after <= before + 10, "GDI-объектов было {before}, стало {after}");
        assert!(icon_from_pixels(0, &pixels).is_none());
        // Короткий буфер пикселей не читается за его концом.
        let short = icon_from_pixels(16, &pixels[..100]).expect("значок");
        unsafe { DestroyIcon(HICON(short as *mut _)).expect("DestroyIcon") };
    }
}
