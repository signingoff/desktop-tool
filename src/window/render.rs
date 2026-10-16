use super::{wide, State};
use crate::{layout, market::Session, quote};
use std::ptr::{null, null_mut};
use windows_sys::{
    core::w,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
    },
};

// Reserved background color; glyphs use no antialiasing against this key.
pub(super) const TRANSPARENT_KEY: u32 = 0x00010101;

// Both placement and drawing use this same font and item padding.
fn font_size(dpi: i32) -> i32 {
    13 * dpi / 96
}
fn padding(dpi: i32) -> i32 {
    (2 * dpi / 96).max(1)
}

struct SelectedFont {
    dc: HDC,
    font: HFONT,
    previous: HGDIOBJ,
}
impl SelectedFont {
    unsafe fn new(dc: HDC, size: i32) -> Option<Self> {
        let font = CreateFontW(
            -size,
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            NONANTIALIASED_QUALITY as u32,
            0,
            w!("Microsoft YaHei UI"),
        );
        if font.is_null() {
            return None;
        }
        let previous = SelectObject(dc, font);
        if previous.is_null() || previous as isize == -1 {
            DeleteObject(font);
            return None;
        }
        Some(Self { dc, font, previous })
    }
}
impl Drop for SelectedFont {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.font);
        }
    }
}

unsafe fn text_width(dc: HDC, text: &str, size: i32) -> Option<i32> {
    let _font = SelectedFont::new(dc, size)?;
    let text = wide(text);
    let mut rect: RECT = std::mem::zeroed();
    let measured = DrawTextW(
        dc,
        text.as_ptr(),
        text.len() as i32 - 1,
        &mut rect,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    (measured > 0 && rect.right > 0).then_some(rect.right)
}

pub(super) unsafe fn measure_widths(state: &State, session: Session, dpi: i32) -> Option<Vec<i32>> {
    let dc = CreateCompatibleDC(null_mut());
    if dc.is_null() {
        return None;
    }
    let count = quote::indices(session.market).len();
    let widths = layout::page(state.page, count, count)
        .into_iter()
        .map(|i| {
            let (text, _) = quote_line(state, session, i);
            text_width(dc, &text, font_size(dpi)).map(|width| width + 2 * padding(dpi))
        })
        .collect();
    DeleteDC(dc);
    widths
}

unsafe fn draw(dc: HDC, text: &str, size: i32, rect: &mut RECT, color: u32) {
    let Some(_font) = SelectedFont::new(dc, size) else {
        return;
    };
    SetTextColor(dc, color);
    let text = wide(text);
    DrawTextW(
        dc,
        text.as_ptr(),
        text.len() as i32 - 1,
        rect,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
}
pub(super) unsafe fn paint(hwnd: HWND, state: &State) {
    let mut ps = std::mem::zeroed();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut rect: RECT = std::mem::zeroed();
    if !dc.is_null() && GetClientRect(hwnd, &mut rect) != 0 && rect.right > 0 && rect.bottom > 0 {
        let buffer = CreateCompatibleDC(dc);
        let bitmap = CreateCompatibleBitmap(dc, rect.right, rect.bottom);
        if !buffer.is_null() && !bitmap.is_null() {
            let previous = SelectObject(buffer, bitmap);
            if !previous.is_null() && previous as isize != -1 {
                render(hwnd, buffer, state);
                // Publish the complete frame, never the intermediate cleared background.
                if BitBlt(dc, 0, 0, rect.right, rect.bottom, buffer, 0, 0, SRCCOPY) == 0 {
                    InvalidateRect(hwnd, null(), 0);
                }
                SelectObject(buffer, previous);
            } else {
                InvalidateRect(hwnd, null(), 0);
            }
        } else {
            InvalidateRect(hwnd, null(), 0);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        if !buffer.is_null() {
            DeleteDC(buffer);
        }
    }
    EndPaint(hwnd, &ps);
}
fn quote_line(state: &State, session: Session, i: usize) -> (String, u32) {
    format_quote(
        quote::indices(session.market)[i].name,
        state.quotes.get(i),
        state.error.as_deref(),
        crate::palette::WHITE_BACKGROUND,
    )
}

fn format_quote(
    name: &str,
    quote: Option<&Result<quote::Quote, String>>,
    error: Option<&str>,
    palette: crate::palette::Palette,
) -> (String, u32) {
    match quote.and_then(|q| q.as_ref().ok()) {
        Some(q) => {
            let color = if error.is_some() {
                palette.muted
            } else if q.percent > 0.0 {
                palette.up
            } else if q.percent < 0.0 {
                palette.down
            } else {
                palette.neutral
            };
            (
                format!(
                    "{} {:.2} {:+.2}%{}",
                    name,
                    q.price,
                    q.percent,
                    if error.is_some() { " 旧" } else { "" }
                ),
                color,
            )
        }
        None => {
            let message = error
                .or_else(|| quote.and_then(|q| q.as_ref().err().map(String::as_str)))
                .unwrap_or("等待当天行情");
            (format!("{name} {message}"), palette.muted)
        }
    }
}
pub(super) unsafe fn render(hwnd: HWND, dc: HDC, state: &State) {
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let saved = SaveDC(dc);
    SetDCBrushColor(dc, TRANSPARENT_KEY);
    FillRect(dc, &rect, GetStockObject(DC_BRUSH) as HBRUSH);
    let dpi = GetDpiForWindow(hwnd).max(96) as i32;
    let height = (24 * dpi / 96).min(rect.bottom);
    let top = (rect.bottom - height) / 2;
    let radius = 8 * dpi / 96;
    let background = CreateRoundRectRgn(0, top, rect.right, top + height, radius, radius);
    if !background.is_null() {
        FillRgn(dc, background, GetStockObject(WHITE_BRUSH) as HBRUSH);
        DeleteObject(background);
    } else {
        // Preserve readable text if GDI cannot allocate the rounded region.
        FillRect(dc, &rect, GetStockObject(WHITE_BRUSH) as HBRUSH);
    }
    SetBkColor(dc, 0x00FFFFFF);
    SetBkMode(dc, TRANSPARENT as i32);
    if let Some(session) = state.session {
        let list = quote::indices(session.market);
        let visible = state.visible.max(1).min(list.len());
        let mut left = 0;
        // Match the measured widths used by placement; leave only a small gap.
        let font_size = font_size(dpi);
        for i in layout::page(state.page, visible, list.len()) {
            let (text, color) = quote_line(state, session, i);
            let padding = padding(dpi);
            let Some(width) = text_width(dc, &text, font_size) else {
                break;
            };
            let right = left + width + 2 * padding;

            draw(
                dc,
                &text,
                font_size,
                &mut RECT {
                    left: left + padding,
                    top: 0,
                    right: right - padding,
                    bottom: rect.bottom,
                },
                color,
            );
            left = right;
        }
    }
    if saved != 0 {
        RestoreDC(dc, saved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measured_text_grows_with_content_and_dpi() {
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            assert!(!dc.is_null());
            let short = text_width(dc, "上证 1234.56 +1.23%", 13);
            let long = text_width(dc, "上证 12345.67 +12.34% 旧", 13);
            let scaled = text_width(dc, "上证 1234.56 +1.23%", 26);
            DeleteDC(dc);
            assert!(short.unwrap() > 0);
            assert!(long.unwrap() > short.unwrap());
            assert!(scaled.unwrap() > short.unwrap());
        }
    }

    #[test]
    fn quote_format_preserves_direction_stale_marker_and_error_priority() {
        let palette = crate::palette::WHITE_BACKGROUND;
        let quote = Ok(quote::Quote {
            price: 3000.0,
            percent: -1.25,
            time: chrono::NaiveDate::from_ymd_opt(2026, 10, 16)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap(),
        });
        assert_eq!(
            format_quote("上证", Some(&quote), None, palette),
            ("上证 3000.00 -1.25%".into(), palette.down)
        );
        assert_eq!(
            format_quote("上证", Some(&quote), Some("offline"), palette),
            ("上证 3000.00 -1.25% 旧".into(), palette.muted)
        );
        let invalid = Err("invalid".into());
        assert_eq!(
            format_quote("上证", Some(&invalid), Some("offline"), palette).0,
            "上证 offline"
        );
        assert_eq!(
            format_quote("上证", Some(&invalid), None, palette).0,
            "上证 invalid"
        );
        assert_eq!(
            format_quote("上证", None, None, palette).0,
            "上证 等待当天行情"
        );
    }
}
