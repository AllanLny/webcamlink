//! Minimal Win32 preview window: GDI blit of BGRA frames plus a stats band.
//!
//! Enough for P1 (measurement and debugging on the PC); the tray application
//! owns the real UI later.

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetStockObject,
    InvalidateRect, SelectObject, SetBkMode, SetTextColor, StretchDIBits, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DEFAULT_GUI_FONT, DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_TOP, HDC,
    PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetClientRect, IsWindow, LoadCursorW, PeekMessageW, PostQuitMessage, RegisterClassW, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW,
    WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WM_QUIT, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
};

struct PreviewState {
    /// Raw HWND value: kept as an integer so the state stays `Send`
    /// (the window is only ever touched from the thread that created it).
    hwnd: isize,
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    stats: String,
    frames: u64,
}

impl PreviewState {
    fn window(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }
}

static PREVIEW: OnceLock<Arc<Mutex<PreviewState>>> = OnceLock::new();

pub struct PreviewWindow {
    state: Arc<Mutex<PreviewState>>,
}

impl PreviewWindow {
    pub fn create(title: &str, width: u32, height: u32) -> Result<Self> {
        let state = Arc::new(Mutex::new(PreviewState {
            hwnd: 0,
            bgra: Vec::new(),
            width: width.max(16),
            height: height.max(16),
            stats: "en attente du flux…".to_string(),
            frames: 0,
        }));
        let _ = PREVIEW.set(state.clone());

        unsafe {
            let module = GetModuleHandleW(None).context("GetModuleHandleW")?;
            let instance = HINSTANCE(module.0);
            let class_name = w!("WebcamLinkPreview");

            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            };
            RegisterClassW(&class);

            let mut rect = RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 };
            let _ = AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, false);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                &HSTRING::from(title),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                None,
                None,
                Some(instance),
                None,
            )?;

            state.lock().unwrap().hwnd = hwnd.0 as isize;
            let _ = ShowWindow(hwnd, SW_SHOW);
        }

        Ok(Self { state })
    }

    /// Hand a new BGRA frame to the window (cheap; the blit happens in WM_PAINT).
    pub fn present(&self, bgra: Vec<u8>, width: u32, height: u32) {
        if let Ok(mut state) = self.state.lock() {
            state.bgra = bgra;
            state.width = width;
            state.height = height;
            state.frames += 1;
            let hwnd = state.window();
            if !hwnd.is_invalid() {
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
        }
    }

    pub fn set_stats(&self, text: String) {
        if let Ok(mut state) = self.state.lock() {
            state.stats = text;
        }
    }

    pub fn frames(&self) -> u64 {
        self.state.lock().map(|state| state.frames).unwrap_or(0)
    }

    /// Pump pending messages; returns `false` once the window is gone.
    pub fn pump(&self) -> bool {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    return false;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let hwnd = self.state.lock().map(|state| state.window()).unwrap_or_default();
            IsWindow(Some(hwnd)).as_bool()
        }
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        if let Ok(state) = self.state.lock() {
            let hwnd = state.window();
            if !hwnd.is_invalid() {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let device = BeginPaint(hwnd, &mut paint);
            paint_window(device);
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn paint_window(device: HDC) {
    let Some(shared) = PREVIEW.get() else { return };
    let Ok(state) = shared.lock() else { return };

    let mut client = RECT::default();
    let _ = GetClientRect(state.window(), &mut client);
    let client_width = (client.right - client.left).max(1);
    let client_height = (client.bottom - client.top).max(1);

    let black = CreateSolidBrush(COLORREF(0));
    let background = RECT { left: 0, top: 0, right: client_width, bottom: client_height };
    FillRect(device, &background, black);
    let _ = DeleteObject(black.into());

    if !state.bgra.is_empty() && state.width > 0 && state.height > 0 {
        let scale = (client_width as f64 / f64::from(state.width))
            .min(client_height as f64 / f64::from(state.height));
        let draw_width = (f64::from(state.width) * scale).round() as i32;
        let draw_height = (f64::from(state.height) * scale).round() as i32;
        let x = (client_width - draw_width) / 2;
        let y = (client_height - draw_height) / 2;

        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: state.width as i32,
                biHeight: -(state.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        StretchDIBits(
            device,
            x,
            y,
            draw_width,
            draw_height,
            0,
            0,
            state.width as i32,
            state.height as i32,
            Some(state.bgra.as_ptr() as *const core::ffi::c_void),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }

    let font = GetStockObject(DEFAULT_GUI_FONT);
    let previous = SelectObject(device, font);
    SetBkMode(device, TRANSPARENT);
    SetTextColor(device, COLORREF(0x00F0_F0F0));

    let band = RECT { left: 8, top: 8, right: 8 + 660, bottom: 8 + 92 };
    let dark = CreateSolidBrush(COLORREF(0x0020_2020));
    FillRect(device, &band, dark);
    let _ = DeleteObject(dark.into());

    let mut text: Vec<u16> = state.stats.encode_utf16().collect();
    let mut text_rect =
        RECT { left: 16, top: 12, right: client_width - 8, bottom: client_height - 8 };
    DrawTextW(device, &mut text, &mut text_rect, DT_LEFT | DT_TOP | DT_NOPREFIX);

    let _ = SelectObject(device, previous);
}
