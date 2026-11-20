//! Снимок экрана (P 891–922). Здесь же обёртки GDI с освобождением в `Drop` (ими пользуется
//! icon.rs): GDI-объекты освобождаются на всех ветках, в том числе при раннем `return`.

use std::ffi::c_void;

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC,
    ReleaseDC, SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HALFTONE, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};

use crate::types::Rect;
use crate::win::api::Api;

/// DC всего экрана (`GetDC(NULL)`), `ReleaseDC` в `Drop`.
pub(super) struct ScreenDc(pub(super) HDC);

impl ScreenDc {
    pub(super) fn get() -> Option<ScreenDc> {
        // SAFETY: DC экрана; освобождается в Drop.
        let dc = unsafe { GetDC(HWND::default()) };
        (!dc.is_invalid()).then_some(ScreenDc(dc))
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: DC получен GetDC(NULL) и освобождается один раз.
        unsafe {
            ReleaseDC(HWND::default(), self.0);
        }
    }
}

/// DC в памяти, `DeleteDC` в `Drop`.
struct MemoryDc(HDC);

impl MemoryDc {
    fn compatible(dc: HDC) -> Option<MemoryDc> {
        // SAFETY: создание DC; удаляется в Drop.
        let memory = unsafe { CreateCompatibleDC(dc) };
        (!memory.is_invalid()).then_some(MemoryDc(memory))
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        // SAFETY: собственный DC, удаляется один раз.
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

/// 32-битный DIB сверху вниз и указатель на его пиксели, `DeleteObject` в `Drop`.
pub(super) struct Dib {
    pub(super) bitmap: HBITMAP,
    pub(super) bits: *mut c_void,
}

impl Dib {
    pub(super) fn new(dc: HDC, width: i32, height: i32) -> Option<Dib> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // сверху вниз
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: `info` и `bits` — собственные; растр удаляется в Drop.
        let bitmap = unsafe {
            CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0)
        }
        .ok()?;
        let dib = Dib { bitmap, bits };
        (!bits.is_null()).then_some(dib) // иначе Drop удалит растр
    }

    /// Длина пикселей в байтах (4 на пиксель).
    pub(super) fn byte_len(width: i32, height: i32) -> Option<usize> {
        if width <= 0 || height <= 0 {
            return None;
        }
        (width as usize).checked_mul(height as usize)?.checked_mul(4)
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: собственный растр; к этому моменту он уже не выбран ни в один DC.
        unsafe {
            let _ = DeleteObject(self.bitmap);
        }
    }
}

/// Любой GDI-объект, `DeleteObject` в `Drop`.
pub(super) struct GdiObject(pub(super) HGDIOBJ);

impl Drop for GdiObject {
    fn drop(&mut self) {
        // SAFETY: собственный объект, удаляется один раз.
        unsafe {
            let _ = DeleteObject(self.0);
        }
    }
}

/// Объект, выбранный в DC; в `Drop` возвращается прежний (до удаления объекта и DC).
struct Selected {
    dc: HDC,
    previous: HGDIOBJ,
}

impl Selected {
    fn new(dc: HDC, object: HGDIOBJ) -> Option<Selected> {
        // SAFETY: собственные DC и объект.
        let previous = unsafe { SelectObject(dc, object) };
        (!previous.is_invalid()).then_some(Selected { dc, previous })
    }
}

impl Drop for Selected {
    fn drop(&mut self) {
        // SAFETY: возврат объекта, который стоял в собственном DC.
        unsafe {
            SelectObject(self.dc, self.previous);
        }
    }
}

/// Пиксели BGRA сверху вниз, `width × height × 4` байт.
#[derive(Clone)]
pub struct Bitmap {
    pub width: i32,
    pub height: i32,
    pub bgra: Vec<u8>,
}

impl Api {
    /// SRCCOPY без CAPTUREBLT: слоёные окна не попадают. `size` — масштабировать до (w, h).
    pub fn capture(&self, rect: Rect, size: Option<(i32, i32)>) -> Option<Bitmap> {
        let width = rect.r.checked_sub(rect.l)?;
        let height = rect.b.checked_sub(rect.t)?;
        if width <= 0 || height <= 0 {
            return None;
        }
        let (out_width, out_height) = size.unwrap_or((width, height));
        let byte_len = Dib::byte_len(out_width, out_height)?;
        // Порядок Drop обратный объявлению: вернуть прежний объект в DC, удалить растр,
        // удалить DC в памяти, отпустить DC экрана — как finally в Python.
        let screen = ScreenDc::get()?;
        let memory = MemoryDc::compatible(screen.0)?;
        let dib = Dib::new(screen.0, out_width, out_height)?;
        let _selected = Selected::new(memory.0, dib.bitmap.into())?;
        // SAFETY: все DC и растр собственные и живы до конца функции; пиксели читаются после
        // GdiFlush в пределах растра `out_width × out_height × 4`.
        unsafe {
            let copied = if size.is_some() {
                SetStretchBltMode(memory.0, HALFTONE);
                StretchBlt(
                    memory.0, 0, 0, out_width, out_height, screen.0, rect.l, rect.t, width,
                    height, SRCCOPY,
                )
                .as_bool()
            } else {
                BitBlt(memory.0, 0, 0, width, height, screen.0, rect.l, rect.t, SRCCOPY).is_ok()
            };
            if !copied {
                return None;
            }
            let _ = GdiFlush();
            let bgra = std::slice::from_raw_parts(dib.bits as *const u8, byte_len).to_vec();
            Some(Bitmap { width: out_width, height: out_height, bgra })
        }
    }
}
