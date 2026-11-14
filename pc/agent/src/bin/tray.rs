//! `wcl-tray` — icône de notification WebcamLink : réglages de la caméra
//! virtuelle (résolution, images/s, débit, caméra iPhone, miroir), démarrage
//! automatique de l'agent et contrôle de celui-ci.
//!
//! Les réglages sont écrits dans `%ProgramData%\WebcamLink\config.ini` : la DLL
//! de webcam virtuelle le lit à l'enregistrement (le format s'applique au
//! prochain démarrage de Windows), l'agent à chaque ouverture de la caméra
//! (caméra, débit et miroir s'appliquent immédiatement).
#![windows_subsystem = "windows"]

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, COLORREF, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HINSTANCE, HWND,
    LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, GetSysColor, GetSysColorBrush, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, COLOR_WINDOW, COLOR_WINDOWTEXT, DEFAULT_CHARSET, DEFAULT_PITCH, FW_SEMIBOLD,
    HBRUSH, HDC, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateMutexW, CreateProcessW, OpenProcess, TerminateProcess, CREATE_NO_WINDOW,
    PROCESS_INFORMATION, PROCESS_TERMINATE, STARTUPINFOW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, ShellExecuteW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW,
    GetSystemMetrics, HMENU, LoadCursorW,
    LoadImageW, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, SendMessageW,
    SetForegroundWindow, SetWindowTextW, ShowWindow, TrackPopupMenu, TranslateMessage,
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWNLIST, HICON, ICON_BIG,
    ICON_SMALL, IDC_ARROW, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE, LR_SHARED,
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MESSAGEBOX_STYLE, MF_ENABLED, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, MSG, SM_CXSCREEN, SM_CYSCREEN, STM_SETICON, SW_HIDE, SW_SHOW,
    SW_SHOWNORMAL, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_HSCROLL, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WM_SETFONT, WM_SETICON, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
    WS_VSCROLL,
};
use wcl_agent::config::{self, CameraCaps, CameraPreference, Settings};
use wcl_agent::protocol::{
    IMAGE_CAPS_EXPOSURE_DURATION, IMAGE_CAPS_FOCUS_DISTANCE, IMAGE_CAPS_HDR, IMAGE_CAPS_ISO,
    IMAGE_CAPS_STABILIZATION, IMAGE_CAPS_TORCH, IMAGE_CAPS_TORCH_LEVEL, IMAGE_CAPS_WHITE_BALANCE,
    IMAGE_CAPS_ZOOM,
};

const WM_TRAY: u32 = WM_APP + 1;
/// Relance (raccourci du menu Démarrer) : afficher la fenêtre de réglages.
const WM_SHOW_SETTINGS: u32 = WM_APP + 2;
/// Relance avec `--image` : afficher la fenêtre des réglages image.
const WM_SHOW_IMAGE: u32 = WM_APP + 3;
const TRAY_ICON_ID: u32 = 1;

/// `BST_CHECKED` vit dans `Win32::UI::Controls` : la valeur est répétée ici.
const BST_CHECKED: u32 = 1;

/// Styles de contrôles `Static` (eux aussi dans `Win32::UI::Controls`).
const SS_ICON: u32 = 0x0000_0003;
const SS_ETCHEDHORZ: u32 = 0x0000_0010;

// Commandes du menu et des boutons.
const CM_SETTINGS: u16 = 1;
const CM_START_AGENT: u16 = 2;
const CM_STOP_AGENT: u16 = 3;
const CM_OPEN_LOG: u16 = 4;
const CM_QUIT: u16 = 5;
const CM_IMAGE: u16 = 6;
const CM_SAVE: u16 = 101;
const CM_CLOSE: u16 = 102;
const CM_IMAGE_RESET: u16 = 103;

// Identifiants des contrôles de la fenêtre de réglages.
const IDC_RESOLUTION: u16 = 201;
const IDC_FPS: u16 = 202;
const IDC_BITRATE: u16 = 203;
const IDC_CAMERA: u16 = 204;
const IDC_MIRROR: u16 = 205;
const IDC_AUTOSTART: u16 = 206;
const IDC_STATUS: u16 = 207;
const IDC_IMAGE_SETTINGS: u16 = 208;

// Identifiants des contrôles de la fenêtre image.
const IDC_IMG_ZOOM: u16 = 301;
const IDC_IMG_EXPOSURE_MODE: u16 = 302;
const IDC_IMG_EXPOSURE_BIAS: u16 = 303;
const IDC_IMG_ISO: u16 = 304;
const IDC_IMG_EXPOSURE_DURATION: u16 = 305;
const IDC_IMG_WB_MODE: u16 = 306;
const IDC_IMG_WB_TEMPERATURE: u16 = 307;
const IDC_IMG_WB_TINT: u16 = 308;
const IDC_IMG_FOCUS_MODE: u16 = 309;
const IDC_IMG_FOCUS_DISTANCE: u16 = 310;
const IDC_IMG_TORCH: u16 = 311;
const IDC_IMG_TORCH_LEVEL: u16 = 312;
const IDC_IMG_STABILIZATION: u16 = 313;
const IDC_IMG_HDR: u16 = 314;
const IDC_IMG_STATUS: u16 = 315;

/// Piste de réglage (`msctls_trackbar32`) : styles et messages, qui vivent dans
/// `Win32::UI::Controls` comme `BST_CHECKED`, donc répétés ici.
const TBS_AUTOTICKS: u32 = 0x0000_0001;
const TBM_SETRANGEMIN: u32 = 0x0400 + 7;
const TBM_SETRANGEMAX: u32 = 0x0400 + 8;
const TBM_SETPAGESIZE: u32 = 0x0400 + 13;
const TBM_SETTICFREQ: u32 = 0x0400 + 20;
const TBM_SETPOS: u32 = 0x0400 + 5;
const TBM_GETPOS: u32 = 0x0400 + 0;

const EXPOSURE_MODE_LABELS: [&str; 3] = ["Automatique", "Biais d'exposition", "Manuel (ISO + durée)"];
const WB_MODE_LABELS: [&str; 3] = ["Automatique", "Verrouillée", "Manuelle (température + teinte)"];
const FOCUS_MODE_LABELS: [&str; 3] = ["Continu", "Verrouillé (distance)", "Point + verrou"];
const STABILIZATION_LABELS: [&str; 3] = ["Arrêt", "Standard", "Cinématique"];

/// Paliers d'exposition proposés (µs) ; les bornes de la caméra filtrent ensuite.
const EXPOSURE_STOPS_US: [(u32, &str); 15] = [
    (125, "1/8000 s"),
    (250, "1/4000 s"),
    (500, "1/2000 s"),
    (1_000, "1/1000 s"),
    (2_000, "1/500 s"),
    (4_000, "1/250 s"),
    (8_000, "1/125 s"),
    (16_666, "1/60 s"),
    (33_333, "1/30 s"),
    (41_666, "1/24 s"),
    (66_666, "1/15 s"),
    (125_000, "1/8 s"),
    (250_000, "1/4 s"),
    (500_000, "1/2 s"),
    (1_000_000, "1 s"),
];

// Plafond matériel : 4K (capacité d'un slot de mémoire partagée
// = 3840 x 2160 x 1,5 octet). Au-delà, la DLL retomberait sur 1080p.
const RESOLUTIONS: [(u32, u32, &str); 4] = [
    (3840, 2160, "3840x2160 (4K)"),
    (1920, 1080, "1920x1080"),
    (1280, 720, "1280x720"),
    (640, 480, "640x480"),
];
const FPS_CHOICES: [u32; 2] = [30, 60];
const FPS_LABELS: [&str; 2] = ["30", "60"];
const BITRATES: [(u32, &str); 7] = [
    (5_000, "5 Mbit/s"),
    (10_000, "10 Mbit/s"),
    (20_000, "20 Mbit/s"),
    (30_000, "30 Mbit/s"),
    (40_000, "40 Mbit/s"),
    (60_000, "60 Mbit/s"),
    (100_000, "100 Mbit/s"),
];

#[derive(Default)]
struct Controls {
    resolution: isize,
    fps: isize,
    bitrate: isize,
    camera: isize,
    mirror: isize,
    autostart: isize,
    status: isize,
}

struct State {
    settings: Settings,
    tray_window: isize,
    settings_window: isize,
    image_window: isize,
    controls: Controls,
    image: ImageControls,
    icons: (isize, isize),
    fonts: Fonts,
    muted: [isize; 6],
    image_muted: Vec<isize>,
}

/// Contrôles de la fenêtre image : pistes, listes, cases et étiquettes de valeur.
#[derive(Default)]
struct ImageControls {
    zoom: isize,
    zoom_value: isize,
    exposure_mode: isize,
    exposure_bias: isize,
    exposure_bias_value: isize,
    iso: isize,
    iso_value: isize,
    exposure_duration: isize,
    wb_mode: isize,
    wb_temperature: isize,
    wb_temperature_value: isize,
    wb_tint: isize,
    wb_tint_value: isize,
    focus_mode: isize,
    focus_distance: isize,
    focus_distance_value: isize,
    torch: isize,
    torch_level: isize,
    torch_level_value: isize,
    stabilization: isize,
    hdr: isize,
    status: isize,
}

#[derive(Default)]
struct Fonts {
    ui: isize,
    title: isize,
    small: isize,
    section: isize,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn state() -> MutexGuard<'static, State> {
    STATE.get().expect("état initialisé").lock().unwrap()
}

fn hwnd(value: isize) -> HWND {
    HWND(value as *mut core::ffi::c_void)
}

unsafe fn send(handle: HWND, message: u32, wparam: usize, lparam: isize) -> LRESULT {
    SendMessageW(handle, message, Some(WPARAM(wparam)), Some(LPARAM(lparam)))
}

/// Journal de l'icône : application sans console, le fichier est la seule sortie
/// observable (`%ProgramData%\WebcamLink\tray.log`).
fn log(message: &str) {
    use std::io::Write;
    let directory = config::data_directory();
    let _ = std::fs::create_dir_all(&directory);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("tray.log"))
    else {
        return;
    };
    let now = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let _ = writeln!(
        file,
        "[{:02}:{:02}:{:02}.{:03} {}] {message}",
        now.wHour,
        now.wMinute,
        now.wSecond,
        now.wMilliseconds,
        std::process::id()
    );
}

/// Logo de l'application : `assets\webcamlink.ico` (à côté de l'exe, ou dans le
/// dépôt pendant le développement).
fn icon_path() -> Option<PathBuf> {
    let directory = exe_directory();
    [
        // Installation : assets à côté de l'exe.
        directory.join("assets").join("webcamlink.ico"),
        directory.join("webcamlink.ico"),
        // Développement : target\release → target → agent.
        directory.join("..").join("..").join("assets").join("webcamlink.ico"),
    ]
    .into_iter()
    .find(|path| path.exists())
}

/// Icône du logo, repli sur l'icône d'application générique.
fn load_icon(size: i32) -> isize {
    let path = icon_path();
    match &path {
        Some(path) => log(&format!("icône : {}", path.display())),
        None => log("icône : webcamlink.ico introuvable, icône générique utilisée"),
    }
    let loaded = path.and_then(|path| {
        let wide: Vec<u16> =
            path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
        let flags = if size > 0 { LR_LOADFROMFILE } else { LR_LOADFROMFILE | LR_DEFAULTSIZE };
        unsafe {
            LoadImageW(None, PCWSTR(wide.as_ptr()), IMAGE_ICON, size, size, flags)
                .ok()
                .map(|handle| handle.0 as isize)
        }
    });
    loaded.unwrap_or_else(|| unsafe {
        LoadImageW(None, IDI_APPLICATION, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED)
            .map(|handle| handle.0 as isize)
            .unwrap_or(0)
    })
}

fn create_font(height: i32, weight: i32) -> isize {
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            w!("Segoe UI"),
        )
        .0 as isize
    }
}

unsafe fn apply_font(handle: isize, font: isize) {
    if handle != 0 && font != 0 {
        send(hwnd(handle), WM_SETFONT, font as usize, 1);
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Déjà lancé ? Le second lancement (raccourci du menu Démarrer) demande à
    // l'instance active d'ouvrir ses réglages, puis s'arrête.
    unsafe {
        if let Ok(existing) = FindWindowW(w!("WebcamLinkTray"), PCWSTR::null()) {
            // Le relancement ouvre ce qu'il demande : les réglages image si on
            // les a demandés, sinon la fenêtre de réglages.
            let message = if std::env::args().any(|argument| argument == "--image") {
                WM_SHOW_IMAGE
            } else {
                WM_SHOW_SETTINGS
            };
            let _ = PostMessageW(Some(existing), message, WPARAM(0), LPARAM(0));
            return Ok(());
        }
        // Une seule icône à la fois : le mutex nommé fait office de verrou.
        let _guard = CreateMutexW(None, false, w!("Global\\WebcamLinkTray"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        let initial = State {
            settings: Settings::load(),
            tray_window: 0,
            settings_window: 0,
            image_window: 0,
            controls: Controls::default(),
            image: ImageControls::default(),
            icons: (load_icon(32), load_icon(0)),
            image_muted: Vec::new(),
            fonts: Fonts {
                ui: create_font(-12, 400),
                title: create_font(-22, FW_SEMIBOLD.0 as i32),
                small: create_font(-11, 400),
                section: create_font(-11, FW_SEMIBOLD.0 as i32),
            },
            muted: [0; 6],
        };
        let _ = STATE.set(Mutex::new(initial));

        let module = GetModuleHandleW(None)?;
        let instance = HINSTANCE(module.0);
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(tray_proc),
            hInstance: instance,
            lpszClassName: w!("WebcamLinkTray"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        });
        let window = CreateWindowExW(
            Default::default(),
            w!("WebcamLinkTray"),
            w!("WebcamLink"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )?;
        state().tray_window = window.0 as isize;
        add_tray_icon(window)?;
        log("icône WebcamLink ajoutée");

        // `--settings` ou `--image` ouvrent directement une fenêtre (raccourci
        // du menu Démarrer, ou test).
        if std::env::args().any(|argument| argument == "--settings") {
            if let Err(error) = show_settings() {
                log(&format!("réglages indisponibles : {error:#}"));
            }
        }
        if std::env::args().any(|argument| argument == "--image") {
            if let Err(error) = show_image_window() {
                log(&format!("réglages image indisponibles : {error:#}"));
            }
        }

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        remove_tray_icon(window);
    }

    Ok(())
}

fn tray_data(window: HWND) -> Result<NOTIFYICONDATAW> {
    let icon = state().icons.1;
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: HICON(icon as *mut core::ffi::c_void),
        ..Default::default()
    };
    let tooltip: Vec<u16> = "WebcamLink".encode_utf16().collect();
    data.szTip[..tooltip.len()].copy_from_slice(&tooltip);
    Ok(data)
}

unsafe fn add_tray_icon(window: HWND) -> Result<()> {
    let data = tray_data(window)?;
    if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
        return Err(anyhow!("Shell_NotifyIconW(NIM_ADD) a échoué"));
    }
    Ok(())
}

unsafe fn remove_tray_icon(window: HWND) {
    if let Ok(data) = tray_data(window) {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

unsafe extern "system" fn tray_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TRAY => {
            // Identifiant d'icône dans le mot haut, message souris dans le mot
            // bas : comparer sans masquer ne reconnaît aucun clic.
            let event = (lparam.0 as usize & 0xFFFF) as u32;
            if event == WM_RBUTTONUP {
                log("icône : clic droit");
                show_menu(window);
            } else if event == WM_LBUTTONUP {
                log("icône : clic gauche");
                if let Err(error) = show_settings() {
                    log(&format!("réglages indisponibles : {error:#}"));
                    message_box(&format!("Réglages indisponibles : {error:#}"), MB_ICONERROR);
                }
            }
            LRESULT(0)
        }
        WM_SHOW_SETTINGS => {
            log("relance : ouverture des réglages");
            if let Err(error) = show_settings() {
                log(&format!("réglages indisponibles : {error:#}"));
                message_box(&format!("Réglages indisponibles : {error:#}"), MB_ICONERROR);
            }
            LRESULT(0)
        }
        WM_SHOW_IMAGE => {
            log("relance : ouverture des réglages image");
            if let Err(error) = show_image_window() {
                log(&format!("réglages image indisponibles : {error:#}"));
                message_box(&format!("Réglages image indisponibles : {error:#}"), MB_ICONERROR);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_tray_icon(window);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe fn show_menu(window: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let running = agent_pid().is_some();
    let start_flags = if running { MF_STRING | MF_GRAYED } else { MF_STRING | MF_ENABLED };
    let stop_flags = if running { MF_STRING | MF_ENABLED } else { MF_STRING | MF_GRAYED };

    let _ = AppendMenuW(menu, MF_STRING, CM_SETTINGS as usize, w!("Réglages…"));
    let _ = AppendMenuW(menu, MF_STRING, CM_IMAGE as usize, w!("Réglages image…"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, start_flags, CM_START_AGENT as usize, w!("Démarrer l'agent"));
    let _ = AppendMenuW(menu, stop_flags, CM_STOP_AGENT as usize, w!("Arrêter l'agent"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, CM_OPEN_LOG as usize, w!("Ouvrir le journal"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, CM_QUIT as usize, w!("Quitter"));

    let mut point = POINT::default();
    let _ = GetCursorPos(&mut point);
    let _ = SetForegroundWindow(window);
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        point.x,
        point.y,
        Some(0),
        window,
        None,
    );
    let _ = DestroyMenu(menu);
    // Réactive la fenêtre : sinon le menu reste « collé » au prochain clic.
    let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));

    if command.0 != 0 {
        dispatch_command(command.0 as u16);
    }
}

fn dispatch_command(command: u16) {
    log(&format!("commande {command}"));
    match command {
        CM_SETTINGS => {
            if let Err(error) = unsafe { show_settings() } {
                message_box(&format!("Réglages indisponibles : {error:#}"), MB_ICONERROR);
            }
        }
        CM_IMAGE => unsafe {
            if let Err(error) = show_image_window() {
                message_box(&format!("Réglages image indisponibles : {error:#}"), MB_ICONERROR);
            }
        },
        CM_START_AGENT => {
            if let Err(error) = start_agent() {
                message_box(&format!("Démarrage de l'agent impossible : {error:#}"), MB_ICONERROR);
            }
        }
        CM_STOP_AGENT => {
            if let Err(error) = stop_agent() {
                message_box(&format!("Arrêt de l'agent impossible : {error:#}"), MB_ICONERROR);
            }
        }
        CM_OPEN_LOG => open_log(),
        CM_QUIT => unsafe {
            let window = state().tray_window;
            remove_tray_icon(hwnd(window));
            PostQuitMessage(0);
        },
        CM_SAVE => unsafe {
            if let Err(error) = save_settings() {
                message_box(&format!("Enregistrement impossible : {error:#}"), MB_ICONERROR);
            }
        },
        CM_CLOSE => unsafe {
            let window = state().settings_window;
            if window != 0 {
                let _ = ShowWindow(hwnd(window), SW_HIDE);
            }
        },
        IDC_IMAGE_SETTINGS => unsafe {
            if let Err(error) = show_image_window() {
                message_box(&format!("Réglages image indisponibles : {error:#}"), MB_ICONERROR);
            }
        },
        _ => {}
    }
}

fn message_box(text: &str, icon: MESSAGEBOX_STYLE) {
    let text = HSTRING::from(text);
    let caption = HSTRING::from("WebcamLink");
    unsafe {
        MessageBoxW(None, &text, &caption, MB_OK | icon);
    }
}

unsafe fn show_settings() -> Result<()> {
    let existing = state().settings_window;
    if existing == 0
        || !windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd(existing))).as_bool()
    {
        create_settings_window()?;
    }
    let settings = Settings::load();
    state().settings = settings;
    sync_controls(&settings);
    let window = hwnd(state().settings_window);
    let _ = ShowWindow(window, SW_SHOW);
    let _ = SetForegroundWindow(window);
    log(&format!("fenêtre de réglages affichée ({window:?})"));
    Ok(())
}

unsafe fn create_settings_window() -> Result<()> {
    let module = GetModuleHandleW(None)?;
    let instance = HINSTANCE(module.0);
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(settings_proc),
        hInstance: instance,
        lpszClassName: w!("WebcamLinkSettings"),
        hCursor: LoadCursorW(None, IDC_ARROW)?,
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut core::ffi::c_void),
        ..Default::default()
    });

    // 520x500 de client, centrée sur l'écran.
    let mut frame = RECT { left: 0, top: 0, right: 520, bottom: 500 };
    let _ = AdjustWindowRect(&mut frame, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU, false);
    let width = frame.right - frame.left;
    let height = frame.bottom - frame.top;
    let window = CreateWindowExW(
        Default::default(),
        w!("WebcamLinkSettings"),
        w!("WebcamLink — réglages"),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
        (GetSystemMetrics(SM_CYSCREEN) - height) / 2,
        width,
        height,
        None,
        None,
        Some(instance),
        None,
    )?;

    let (icons, fonts) = {
        let guard = state();
        (
            guard.icons,
            Fonts {
                ui: guard.fonts.ui,
                title: guard.fonts.title,
                small: guard.fonts.small,
                section: guard.fonts.section,
            },
        )
    };
    send(window, WM_SETICON, ICON_BIG as usize, icons.0);
    send(window, WM_SETICON, ICON_SMALL as usize, icons.1);

    let label = WS_CHILD | WS_VISIBLE;
    let combo = WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32);
    let button = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32);
    let button_default = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32);
    let check = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32);
    let icon_static = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_ICON);
    let separator = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_ETCHEDHORZ);

    let mut controls = Controls::default();
    let mut muted = [0isize; 6];

    // En-tête : logo, nom, sous-titre, puis filet de séparation.
    let header = control(w!("Static"), "", icon_static, 0, 20, 18, 40, 40, window, 0);
    if icons.0 != 0 {
        send(hwnd(header), STM_SETICON, icons.0 as usize, 0);
    }
    control(w!("Static"), "WebcamLink", label, fonts.title, 72, 12, 300, 34, window, 0);
    muted[0] = control(
        w!("Static"),
        "Caméra iPhone virtuelle",
        label,
        fonts.small,
        74,
        46,
        300,
        20,
        window,
        0,
    );
    control(w!("Static"), "", separator, 0, 20, 78, 480, 1, window, 0);

    // Vidéo
    muted[1] = control(w!("Static"), "VIDÉO", label, fonts.section, 20, 94, 200, 18, window, 0);
    control(w!("Static"), "Résolution", label, fonts.ui, 20, 122, 160, 22, window, 0);
    controls.resolution =
        control(w!("ComboBox"), "", combo, fonts.ui, 190, 118, 310, 220, window, IDC_RESOLUTION);
    control(w!("Static"), "Images/s", label, fonts.ui, 20, 156, 160, 22, window, 0);
    controls.fps = control(w!("ComboBox"), "", combo, fonts.ui, 190, 152, 310, 220, window, IDC_FPS);
    control(w!("Static"), "Débit vidéo", label, fonts.ui, 20, 190, 160, 22, window, 0);
    controls.bitrate =
        control(w!("ComboBox"), "", combo, fonts.ui, 190, 186, 310, 220, window, IDC_BITRATE);

    // Caméra
    muted[2] = control(w!("Static"), "CAMÉRA", label, fonts.section, 20, 226, 200, 18, window, 0);
    control(w!("Static"), "Caméra iPhone", label, fonts.ui, 20, 254, 160, 22, window, 0);
    controls.camera =
        control(w!("ComboBox"), "", combo, fonts.ui, 190, 250, 310, 220, window, IDC_CAMERA);
    controls.mirror = control(
        w!("Button"),
        "Miroir (image retournée)",
        check,
        fonts.ui,
        190,
        282,
        180,
        24,
        window,
        IDC_MIRROR,
    );
    control(
        w!("Button"),
        "Réglages image…",
        button,
        fonts.ui,
        380,
        278,
        120,
        30,
        window,
        IDC_IMAGE_SETTINGS,
    );

    // Agent
    muted[3] = control(w!("Static"), "AGENT", label, fonts.section, 20, 320, 200, 18, window, 0);
    controls.autostart = control(
        w!("Button"),
        "Démarrer avec Windows",
        check,
        fonts.ui,
        20,
        346,
        310,
        24,
        window,
        IDC_AUTOSTART,
    );
    controls.status =
        control(w!("Static"), "", label, fonts.small, 20, 378, 480, 20, window, IDC_STATUS);
    muted[4] = controls.status;

    muted[5] = control(
        w!("Static"),
        "La résolution et les images/s s'appliquent au prochain démarrage de Windows (4K : 30 fps maximum).\nCaméra, débit et miroir s'appliquent dès la prochaine ouverture de la caméra.",
        label,
        fonts.small,
        20,
        406,
        480,
        44,
        window,
        0,
    );
    control(w!("Button"), "Enregistrer", button_default, fonts.ui, 250, 456, 150, 32, window, CM_SAVE);
    control(w!("Button"), "Fermer", button, fonts.ui, 410, 456, 90, 32, window, CM_CLOSE);

    fill_combo(controls.resolution, &RESOLUTIONS.map(|(_, _, name)| name));
    fill_combo(controls.fps, &FPS_LABELS);
    fill_combo(controls.bitrate, &BITRATES.map(|(_, name)| name));
    fill_combo(controls.camera, &CameraPreference::ALL.map(|preference| preference.label()));

    let mut guard = state();
    guard.settings_window = window.0 as isize;
    guard.controls = controls;
    guard.muted = muted;
    log(&format!("fenêtre de réglages créée ({window:?})"));
    Ok(())
}

unsafe fn control(
    class: PCWSTR,
    text: &str,
    style: WINDOW_STYLE,
    font: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: HWND,
    id: u16,
) -> isize {
    let text = HSTRING::from(text);
    let handle = CreateWindowExW(
        Default::default(),
        class,
        &text,
        style,
        x,
        y,
        width,
        height,
        Some(parent),
        Some(HMENU(id as isize as *mut core::ffi::c_void)),
        None,
        None,
    );
    let handle = handle.map(|handle| handle.0 as isize).unwrap_or(0);
    apply_font(handle, font);
    handle
}

unsafe fn fill_combo<Item: AsRef<str>>(handle: isize, items: &[Item]) {
    let combo = hwnd(handle);
    for item in items {
        let text = HSTRING::from(item.as_ref());
        send(combo, CB_ADDSTRING, 0, text.as_ptr() as isize);
    }
}

unsafe fn sync_controls(settings: &Settings) {
    let controls = {
        let guard = state();
        Controls {
            resolution: guard.controls.resolution,
            fps: guard.controls.fps,
            bitrate: guard.controls.bitrate,
            camera: guard.controls.camera,
            mirror: guard.controls.mirror,
            autostart: guard.controls.autostart,
            status: guard.controls.status,
        }
    };

    let resolution = RESOLUTIONS
        .iter()
        .position(|(width, height, _)| *width == settings.width && *height == settings.height)
        .unwrap_or(0);
    let fps = FPS_CHOICES.iter().position(|value| *value == settings.fps).unwrap_or(0);
    let bitrate =
        BITRATES.iter().position(|(value, _)| *value == settings.bitrate_kbps).unwrap_or(2);
    let camera = CameraPreference::ALL
        .iter()
        .position(|preference| *preference == settings.preference)
        .unwrap_or(0);

    send(hwnd(controls.resolution), CB_SETCURSEL, resolution, 0);
    send(hwnd(controls.fps), CB_SETCURSEL, fps, 0);
    send(hwnd(controls.bitrate), CB_SETCURSEL, bitrate, 0);
    send(hwnd(controls.camera), CB_SETCURSEL, camera, 0);
    check(hwnd(controls.mirror), settings.mirror);
    check(hwnd(controls.autostart), autostart_enabled());
    update_agent_status(controls.status);
}

unsafe fn check(handle: HWND, checked: bool) {
    let value = if checked { BST_CHECKED as usize } else { 0 };
    send(handle, BM_SETCHECK, value, 0);
}

unsafe fn is_checked(handle: isize) -> bool {
    send(hwnd(handle), BM_GETCHECK, 0, 0).0 as u32 == BST_CHECKED
}

unsafe fn combo_index(handle: isize) -> usize {
    send(hwnd(handle), CB_GETCURSEL, 0, 0).0.max(0) as usize
}

unsafe fn update_agent_status(status: isize) {
    if status == 0 {
        return;
    }
    let text = if agent_pid().is_some() {
        "Agent : en cours d'exécution"
    } else {
        "Agent : arrêté"
    };
    let _ = SetWindowTextW(hwnd(status), &HSTRING::from(text));
}

unsafe fn save_settings() -> Result<()> {
    let (settings, autostart) = {
        // On repart du fichier pour conserver les réglages image, que cette
        // fenêtre ne touche pas.
        let mut current = Settings::load();
        let autostart = {
            let guard = state();
            let controls = &guard.controls;
            let resolution = RESOLUTIONS[combo_index(controls.resolution).min(RESOLUTIONS.len() - 1)];
            let fps = FPS_CHOICES[combo_index(controls.fps).min(FPS_CHOICES.len() - 1)];
            let bitrate = BITRATES[combo_index(controls.bitrate).min(BITRATES.len() - 1)];
            let camera = CameraPreference::ALL
                [combo_index(controls.camera).min(CameraPreference::ALL.len() - 1)];
            current.width = resolution.0;
            current.height = resolution.1;
            current.fps = fps;
            current.bitrate_kbps = bitrate.0;
            current.preference = camera;
            current.mirror = is_checked(controls.mirror);
            is_checked(controls.autostart)
        };
        (current.clamped(), autostart)
    };

    settings.save()?;
    apply_autostart(autostart)?;
    state().settings = settings;
    sync_controls(&settings);
    message_box(
        &format!(
            "Réglages enregistrés dans {}.\n\nRésolution et images/s : au prochain démarrage de Windows.\nCaméra, débit et miroir : à la prochaine ouverture de la caméra.",
            config::config_path().display()
        ),
        MB_ICONINFORMATION,
    );
    Ok(())
}

unsafe extern "system" fn settings_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CTLCOLORSTATIC => {
            // Sous-titres, sections, statut et note en gris ; libellés en noir.
            let device = HDC(wparam.0 as *mut core::ffi::c_void);
            let subtle = state().muted.contains(&(lparam.0 as isize));
            let _ = SetBkMode(device, TRANSPARENT);
            let color = if subtle {
                COLORREF(0x0080_8080)
            } else {
                COLORREF(GetSysColor(COLOR_WINDOWTEXT))
            };
            let _ = SetTextColor(device, color);
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        WM_COMMAND => {
            let command = (wparam.0 & 0xFFFF) as u16;
            let notification = ((wparam.0 >> 16) & 0xFFFF) as u16;
            if notification == BN_CLICKED as u16 || command == CM_SAVE || command == CM_CLOSE {
                dispatch_command(command);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = ShowWindow(window, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

// MARK: - Fenêtre « Réglages image »

/// Pas vertical d'une ligne de la fenêtre image.
const IMAGE_ROW: i32 = 36;

/// Dernier enregistrement déclenché par un curseur (les déplacements arrivent
/// par rafales : on borne les écritures à ~8 par seconde).
static LAST_IMAGE_SAVE: OnceLock<Mutex<Instant>> = OnceLock::new();

fn image_save_due() -> bool {
    let cell = LAST_IMAGE_SAVE.get_or_init(|| Mutex::new(Instant::now() - Duration::from_secs(1)));
    let Ok(mut last) = cell.lock() else {
        return true;
    };
    if last.elapsed() < Duration::from_millis(120) {
        return false;
    }
    *last = Instant::now();
    true
}

unsafe fn trackbar_position(handle: isize) -> i32 {
    send(hwnd(handle), TBM_GETPOS, 0, 0).0 as i32
}

unsafe fn set_trackbar(handle: isize, minimum: i32, maximum: i32, position: i32) {
    let low = minimum.min(maximum);
    let high = minimum.max(maximum);
    send(hwnd(handle), TBM_SETRANGEMIN, 1, low as isize);
    send(hwnd(handle), TBM_SETRANGEMAX, 1, high as isize);
    send(hwnd(handle), TBM_SETPAGESIZE, 0, ((high - low) / 10).max(1) as isize);
    // Sans fréquence explicite, une graduation par unité : la piste disparaît
    // sous les traits.
    send(hwnd(handle), TBM_SETTICFREQ, ((high - low) / 10).max(1) as usize, 0);
    send(hwnd(handle), TBM_SETPOS, 1, position.clamp(low, high) as isize);
}

/// Libellé + piste + étiquette de valeur, sur une ligne.
unsafe fn slider_row(
    window: HWND,
    text: &str,
    y: i32,
    font: isize,
    small: isize,
    id: u16,
    muted: &mut Vec<isize>,
) -> (isize, isize) {
    control(w!("Static"), text, WS_CHILD | WS_VISIBLE, font, 20, y + 4, 172, 22, window, 0);
    let track = control(
        w!("msctls_trackbar32"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(TBS_AUTOTICKS),
        0,
        196,
        y + 2,
        256,
        26,
        window,
        id,
    );
    let value =
        control(w!("Static"), "—", WS_CHILD | WS_VISIBLE, small, 460, y + 4, 80, 22, window, 0);
    muted.push(value);
    (track, value)
}

/// Libellé + liste déroulante, sur une ligne.
unsafe fn combo_row(
    window: HWND,
    text: &str,
    y: i32,
    font: isize,
    id: u16,
    items: &[&str],
) -> isize {
    control(w!("Static"), text, WS_CHILD | WS_VISIBLE, font, 20, y + 4, 172, 22, window, 0);
    let combo = control(
        w!("ComboBox"),
        "",
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
        font,
        196,
        y,
        344,
        260,
        window,
        id,
    );
    fill_combo(combo, items);
    combo
}

unsafe fn create_image_window() -> Result<()> {
    let module = GetModuleHandleW(None)?;
    let instance = HINSTANCE(module.0);
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(image_proc),
        hInstance: instance,
        lpszClassName: w!("WebcamLinkImageSettings"),
        hCursor: LoadCursorW(None, IDC_ARROW)?,
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut core::ffi::c_void),
        ..Default::default()
    });

    let mut frame = RECT { left: 0, top: 0, right: 560, bottom: 616 };
    let _ = AdjustWindowRect(&mut frame, WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU, false);
    let width = frame.right - frame.left;
    let height = frame.bottom - frame.top;
    let window = CreateWindowExW(
        Default::default(),
        w!("WebcamLinkImageSettings"),
        w!("WebcamLink — réglages image"),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
        (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
        (GetSystemMetrics(SM_CYSCREEN) - height) / 3,
        width,
        height,
        None,
        None,
        Some(instance),
        None,
    )?;

    let (icons, fonts) = {
        let guard = state();
        (
            guard.icons,
            Fonts {
                ui: guard.fonts.ui,
                title: guard.fonts.title,
                small: guard.fonts.small,
                section: guard.fonts.section,
            },
        )
    };
    send(window, WM_SETICON, ICON_BIG as usize, icons.0);
    send(window, WM_SETICON, ICON_SMALL as usize, icons.1);

    let label = WS_CHILD | WS_VISIBLE;
    let check = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_AUTOCHECKBOX as u32);
    let button = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32);
    let button_default = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32);
    let separator = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_ETCHEDHORZ);

    let mut controls = ImageControls::default();
    let mut muted: Vec<isize> = Vec::new();

    let header = control(
        w!("Static"),
        "",
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_ICON),
        0,
        20,
        12,
        32,
        32,
        window,
        0,
    );
    if icons.1 != 0 {
        send(hwnd(header), STM_SETICON, icons.1 as usize, 0);
    }
    control(w!("Static"), "Réglages image", label, fonts.title, 62, 6, 320, 30, window, 0);
    controls.status = control(w!("Static"), "", label, fonts.small, 64, 38, 476, 36, window, IDC_IMG_STATUS);
    muted.push(controls.status);
    control(w!("Static"), "", separator, 0, 20, 84, 520, 1, window, 0);

    let mut y = 96;
    controls.exposure_mode = combo_row(
        window,
        "Exposition — mode",
        y,
        fonts.ui,
        IDC_IMG_EXPOSURE_MODE,
        &EXPOSURE_MODE_LABELS,
    );
    y += IMAGE_ROW;
    (controls.exposure_bias, controls.exposure_bias_value) = slider_row(
        window,
        "Exposition — biais (EV)",
        y,
        fonts.ui,
        fonts.small,
        IDC_IMG_EXPOSURE_BIAS,
        &mut muted,
    );
    y += IMAGE_ROW;
    (controls.iso, controls.iso_value) =
        slider_row(window, "Exposition — ISO", y, fonts.ui, fonts.small, IDC_IMG_ISO, &mut muted);
    y += IMAGE_ROW;
    controls.exposure_duration = combo_row(
        window,
        "Exposition — durée",
        y,
        fonts.ui,
        IDC_IMG_EXPOSURE_DURATION,
        &EXPOSURE_STOPS_US.map(|(_, name)| name),
    );
    y += IMAGE_ROW;
    controls.wb_mode =
        combo_row(window, "Balance — mode", y, fonts.ui, IDC_IMG_WB_MODE, &WB_MODE_LABELS);
    y += IMAGE_ROW;
    (controls.wb_temperature, controls.wb_temperature_value) = slider_row(
        window,
        "Balance — température (K)",
        y,
        fonts.ui,
        fonts.small,
        IDC_IMG_WB_TEMPERATURE,
        &mut muted,
    );
    y += IMAGE_ROW;
    (controls.wb_tint, controls.wb_tint_value) = slider_row(
        window,
        "Balance — teinte",
        y,
        fonts.ui,
        fonts.small,
        IDC_IMG_WB_TINT,
        &mut muted,
    );
    y += IMAGE_ROW;
    controls.focus_mode =
        combo_row(window, "Mise au point — mode", y, fonts.ui, IDC_IMG_FOCUS_MODE, &FOCUS_MODE_LABELS);
    y += IMAGE_ROW;
    (controls.focus_distance, controls.focus_distance_value) = slider_row(
        window,
        "Mise au point — distance",
        y,
        fonts.ui,
        fonts.small,
        IDC_IMG_FOCUS_DISTANCE,
        &mut muted,
    );
    y += IMAGE_ROW;
    (controls.zoom, controls.zoom_value) =
        slider_row(window, "Zoom", y, fonts.ui, fonts.small, IDC_IMG_ZOOM, &mut muted);
    y += IMAGE_ROW;
    controls.torch =
        control(w!("Button"), "Torche", check, fonts.ui, 20, y + 2, 150, 24, window, IDC_IMG_TORCH);
    controls.hdr =
        control(w!("Button"), "HDR", check, fonts.ui, 196, y + 2, 150, 24, window, IDC_IMG_HDR);
    y += IMAGE_ROW;
    (controls.torch_level, controls.torch_level_value) = slider_row(
        window,
        "Torche — niveau",
        y,
        fonts.ui,
        fonts.small,
        IDC_IMG_TORCH_LEVEL,
        &mut muted,
    );
    y += IMAGE_ROW;
    controls.stabilization = combo_row(
        window,
        "Stabilisation",
        y,
        fonts.ui,
        IDC_IMG_STABILIZATION,
        &STABILIZATION_LABELS,
    );
    y += IMAGE_ROW + 8;
    control(w!("Button"), "Tout auto", button, fonts.ui, 20, y, 130, 30, window, CM_IMAGE_RESET);
    control(w!("Button"), "Fermer", button_default, fonts.ui, 430, y, 110, 30, window, CM_CLOSE);

    let mut guard = state();
    guard.image_window = window.0 as isize;
    guard.image = controls;
    guard.image_muted = muted;
    log(&format!("fenêtre image créée ({window:?})"));
    Ok(())
}

/// Bornes, positions et activation des contrôles d'après les réglages courants
/// et ce que la caméra a annoncé (`caps.ini`, vide tant qu'aucun iPhone ne s'est
/// présenté : dans ce cas on garde les bornes du fichier et tout reste actif).
unsafe fn sync_image_controls(settings: &Settings, caps: &CameraCaps) {
    let known = caps.flags != 0;
    let supports = |flag: u16| !known || caps.flags & u32::from(flag) != 0;

    let zoom_max =
        if caps.zoom_max_x100 > 0 { caps.zoom_max_x100 as i32 } else { config::MAX_ZOOM_X100 as i32 };
    let iso_min = if caps.iso_max > 0 { caps.iso_min as i32 } else { config::MIN_ISO as i32 };
    let iso_max = if caps.iso_max > 0 { caps.iso_max as i32 } else { config::MAX_ISO as i32 };
    let wb_min = if caps.wb_max_k > 0 { caps.wb_min_k as i32 } else { config::MIN_WB_TEMP_K as i32 };
    let wb_max = if caps.wb_max_k > 0 { caps.wb_max_k as i32 } else { config::MAX_WB_TEMP_K as i32 };
    let tint_min = if caps.wb_tint_max_x100 != 0 {
        caps.wb_tint_min_x100
    } else {
        config::MIN_WB_TINT_X100
    };
    let tint_max = if caps.wb_tint_max_x100 != 0 {
        caps.wb_tint_max_x100
    } else {
        config::MAX_WB_TINT_X100
    };

    let guard = state();
    let controls = &guard.image;

    set_trackbar(controls.zoom, 100, zoom_max, settings.zoom_x100 as i32);
    set_trackbar(
        controls.exposure_bias,
        config::MIN_EXPOSURE_BIAS_X100,
        config::MAX_EXPOSURE_BIAS_X100,
        i32::from(settings.exposure_bias_ev_x100),
    );
    set_trackbar(controls.iso, iso_min, iso_max, i32::from(settings.iso));
    set_trackbar(controls.wb_temperature, wb_min, wb_max, i32::from(settings.wb_temp_k));
    set_trackbar(controls.wb_tint, tint_min, tint_max, i32::from(settings.wb_tint_x100));
    set_trackbar(controls.focus_distance, 0, 100, i32::from(settings.focus_distance_x100));
    set_trackbar(controls.torch_level, 0, 100, i32::from(settings.torch_level));

    send(hwnd(controls.exposure_mode), CB_SETCURSEL, usize::from(settings.exposure_mode).min(2), 0);
    send(hwnd(controls.wb_mode), CB_SETCURSEL, usize::from(settings.wb_mode).min(2), 0);
    send(hwnd(controls.focus_mode), CB_SETCURSEL, usize::from(settings.focus_mode).min(2), 0);
    send(hwnd(controls.stabilization), CB_SETCURSEL, usize::from(settings.stabilization).min(2), 0);
    let duration = EXPOSURE_STOPS_US
        .iter()
        .position(|(value, _)| *value == settings.exposure_duration_us)
        .unwrap_or(9);
    send(hwnd(controls.exposure_duration), CB_SETCURSEL, duration, 0);
    check(hwnd(controls.torch), settings.torch);
    check(hwnd(controls.hdr), settings.hdr);

    // Grisé : ce que la caméra ne sait pas faire, et ce que le mode courant rend
    // sans effet.
    let exposure_mode = combo_index(controls.exposure_mode);
    let wb_mode = combo_index(controls.wb_mode);
    let focus_mode = combo_index(controls.focus_mode);
    let manual_exposure = exposure_mode == 2;
    let _ = EnableWindow(hwnd(controls.zoom), supports(IMAGE_CAPS_ZOOM));
    let _ = EnableWindow(hwnd(controls.exposure_bias), !manual_exposure);
    let _ = EnableWindow(hwnd(controls.iso), manual_exposure && supports(IMAGE_CAPS_ISO));
    let _ = EnableWindow(
        hwnd(controls.exposure_duration),
        manual_exposure && supports(IMAGE_CAPS_EXPOSURE_DURATION),
    );
    let _ = EnableWindow(hwnd(controls.wb_temperature), wb_mode == 2 && supports(IMAGE_CAPS_WHITE_BALANCE));
    let _ = EnableWindow(hwnd(controls.wb_tint), wb_mode == 2 && supports(IMAGE_CAPS_WHITE_BALANCE));
    let _ = EnableWindow(
        hwnd(controls.focus_distance),
        focus_mode == 1 && supports(IMAGE_CAPS_FOCUS_DISTANCE),
    );
    let _ = EnableWindow(hwnd(controls.torch), supports(IMAGE_CAPS_TORCH));
    let _ = EnableWindow(
        hwnd(controls.torch_level),
        is_checked(controls.torch) && supports(IMAGE_CAPS_TORCH_LEVEL),
    );
    let _ = EnableWindow(hwnd(controls.hdr), supports(IMAGE_CAPS_HDR));
    let _ = EnableWindow(
        hwnd(controls.stabilization),
        !known || (supports(IMAGE_CAPS_STABILIZATION) && caps.stabilization_modes != 0),
    );

    set_value_labels(controls);

    let summary = if known {
        format!(
            "Caméra « {} » · zoom max {}.{:02}× · ISO {}–{} · durée {}–{} µs · balance {}–{} K",
            if caps.camera_label.is_empty() { "iPhone" } else { caps.camera_label.as_str() },
            caps.zoom_max_x100 / 100,
            caps.zoom_max_x100 % 100,
            caps.iso_min,
            caps.iso_max,
            caps.exposure_min_us,
            caps.exposure_max_us,
            caps.wb_min_k,
            caps.wb_max_k,
        )
    } else {
        "La caméra n'a pas encore annoncé ses plages (ouvre la caméra une fois) : les valeurs \
         restent bornées par l'application, l'iPhone ajustera si besoin."
            .to_string()
    };
    let status = if caps.last_error.is_empty() {
        summary
    } else {
        format!("{summary}\nDernière erreur : {}", caps.last_error)
    };
    let _ = SetWindowTextW(hwnd(controls.status), &HSTRING::from(status));
}

/// Étiquettes de valeur des pistes (l'appelant tient le verrou de l'état).
unsafe fn set_value_labels(controls: &ImageControls) {
    let zoom = trackbar_position(controls.zoom);
    let _ = SetWindowTextW(
        hwnd(controls.zoom_value),
        &HSTRING::from(format!("{}.{:02}×", zoom / 100, (zoom % 100).abs())),
    );
    let bias = trackbar_position(controls.exposure_bias);
    let _ = SetWindowTextW(
        hwnd(controls.exposure_bias_value),
        &HSTRING::from(format!("{:+.2} EV", f64::from(bias) / 100.0)),
    );
    let _ = SetWindowTextW(
        hwnd(controls.iso_value),
        &HSTRING::from(format!("{}", trackbar_position(controls.iso))),
    );
    let _ = SetWindowTextW(
        hwnd(controls.wb_temperature_value),
        &HSTRING::from(format!("{} K", trackbar_position(controls.wb_temperature))),
    );
    let tint = trackbar_position(controls.wb_tint);
    let _ = SetWindowTextW(
        hwnd(controls.wb_tint_value),
        &HSTRING::from(format!("{:+.2}", f64::from(tint) / 100.0)),
    );
    let _ = SetWindowTextW(
        hwnd(controls.focus_distance_value),
        &HSTRING::from(format!("{} %", trackbar_position(controls.focus_distance))),
    );
    let _ = SetWindowTextW(
        hwnd(controls.torch_level_value),
        &HSTRING::from(format!("{} %", trackbar_position(controls.torch_level))),
    );
}

unsafe fn refresh_image_labels() {
    let guard = state();
    set_value_labels(&guard.image);
}

/// Enregistre les réglages image. `force` pour les listes et les cases (un
/// changement ponctuel), sinon les curseurs sont limités à ~8 écritures/s.
unsafe fn save_image_settings(force: bool) {
    if !force && !image_save_due() {
        return;
    }

    // On repart du fichier pour ne pas écraser les clés vidéo/caméra.
    let mut settings = Settings::load();
    {
        let guard = state();
        let controls = &guard.image;
        settings.zoom_x100 = trackbar_position(controls.zoom).max(0) as u32;
        settings.exposure_mode = combo_index(controls.exposure_mode).min(2) as u8;
        settings.exposure_bias_ev_x100 = trackbar_position(controls.exposure_bias) as i16;
        settings.iso = trackbar_position(controls.iso).max(0) as u16;
        settings.exposure_duration_us = EXPOSURE_STOPS_US
            [combo_index(controls.exposure_duration).min(EXPOSURE_STOPS_US.len() - 1)]
        .0;
        settings.wb_mode = combo_index(controls.wb_mode).min(2) as u8;
        settings.wb_temp_k = trackbar_position(controls.wb_temperature).max(0) as u16;
        settings.wb_tint_x100 = trackbar_position(controls.wb_tint) as i16;
        settings.focus_mode = combo_index(controls.focus_mode).min(2) as u8;
        settings.focus_distance_x100 = trackbar_position(controls.focus_distance).max(0) as u16;
        settings.torch = is_checked(controls.torch);
        settings.torch_level = trackbar_position(controls.torch_level).max(0) as u8;
        settings.stabilization = combo_index(controls.stabilization).min(2) as u8;
        settings.hdr = is_checked(controls.hdr);
    }

    match settings.save() {
        Ok(()) => state().settings = settings,
        Err(error) => log(&format!("réglages image non enregistrés: {error}")),
    }
}

unsafe fn reset_image_settings() {
    let mut settings = Settings::load();
    let defaults = Settings::default();
    settings.zoom_x100 = defaults.zoom_x100;
    settings.exposure_mode = defaults.exposure_mode;
    settings.exposure_bias_ev_x100 = defaults.exposure_bias_ev_x100;
    settings.iso = defaults.iso;
    settings.exposure_duration_us = defaults.exposure_duration_us;
    settings.wb_mode = defaults.wb_mode;
    settings.wb_temp_k = defaults.wb_temp_k;
    settings.wb_tint_x100 = defaults.wb_tint_x100;
    settings.focus_mode = defaults.focus_mode;
    settings.focus_distance_x100 = defaults.focus_distance_x100;
    settings.torch = defaults.torch;
    settings.torch_level = defaults.torch_level;
    settings.stabilization = defaults.stabilization;
    settings.hdr = defaults.hdr;
    match settings.save() {
        Ok(()) => {
            state().settings = settings;
            sync_image_controls(&settings, &CameraCaps::load());
            log("réglages image remis à l'automatique");
        }
        Err(error) => log(&format!("réinitialisation impossible: {error}")),
    }
}

unsafe fn show_image_window() -> Result<()> {
    let existing = state().image_window;
    if existing == 0
        || !windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd(existing))).as_bool()
    {
        create_image_window()?;
    }
    let settings = Settings::load();
    let caps = CameraCaps::load();
    state().settings = settings;
    sync_image_controls(&settings, &caps);
    let window = hwnd(state().image_window);
    let _ = ShowWindow(window, SW_SHOW);
    let _ = SetForegroundWindow(window);
    log(&format!("fenêtre image affichée ({window:?})"));
    Ok(())
}

unsafe extern "system" fn image_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CTLCOLORSTATIC => {
            let device = HDC(wparam.0 as *mut core::ffi::c_void);
            let subtle = state().image_muted.contains(&(lparam.0 as isize));
            let _ = SetBkMode(device, TRANSPARENT);
            let color = if subtle {
                COLORREF(0x0080_8080)
            } else {
                COLORREF(GetSysColor(COLOR_WINDOWTEXT))
            };
            let _ = SetTextColor(device, color);
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        WM_HSCROLL => {
            // Les curseurs arrivent par rafales : les étiquettes suivent tout de
            // suite, le fichier est écrit à cadence bornée.
            refresh_image_labels();
            save_image_settings(false);
            LRESULT(0)
        }
        WM_COMMAND => {
            let command = (wparam.0 & 0xFFFF) as u16;
            let notification = ((wparam.0 >> 16) & 0xFFFF) as u16;
            if notification == CBN_SELCHANGE as u16 {
                // Le mode choisi change ce qui est actif : enregistrer puis
                // resynchroniser (le fichier reflète déjà le nouveau mode).
                save_image_settings(true);
                sync_image_controls(&Settings::load(), &CameraCaps::load());
            } else if notification == BN_CLICKED as u16 {
                if command == CM_IMAGE_RESET {
                    reset_image_settings();
                } else {
                    save_image_settings(true);
                    sync_image_controls(&Settings::load(), &CameraCaps::load());
                }
            } else if command == CM_CLOSE {
                let _ = ShowWindow(window, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = ShowWindow(window, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

fn exe_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_default()
}

fn agent_exe() -> PathBuf {
    exe_directory().join("wcl-agent.exe")
}

fn agent_pid() -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                if process_name(&entry).eq_ignore_ascii_case("wcl-agent.exe") {
                    found = Some(entry.th32ProcessID);
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

fn process_name(entry: &PROCESSENTRY32W) -> String {
    let length = entry.szExeFile.iter().position(|value| *value == 0).unwrap_or(0);
    String::from_utf16_lossy(&entry.szExeFile[..length])
}

fn start_agent() -> Result<()> {
    if agent_pid().is_some() {
        return Ok(());
    }
    let exe = agent_exe();
    if !exe.exists() {
        return Err(anyhow!("{} introuvable (à côté de wcl-tray.exe)", exe.display()));
    }
    unsafe {
        let startup =
            STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut process = PROCESS_INFORMATION::default();
        let mut command_line: Vec<u16> =
            format!("\"{}\"", exe.display()).encode_utf16().chain(std::iter::once(0)).collect();
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW,
            None,
            None,
            &startup,
            &mut process,
        )?;
        let _ = CloseHandle(process.hProcess);
        let _ = CloseHandle(process.hThread);
    }
    Ok(())
}

fn stop_agent() -> Result<()> {
    if let Some(pid) = agent_pid() {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, false, pid)?;
            TerminateProcess(handle, 1)?;
            let _ = CloseHandle(handle);
        }
    }
    Ok(())
}

fn open_log() {
    let target = HSTRING::from(config::log_path().to_string_lossy().to_string());
    unsafe {
        ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL);
    }
}

fn run_key() -> PCWSTR {
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
}

fn autostart_enabled() -> bool {
    let mut buffer = [0u16; 1024];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            run_key(),
            w!("WebcamLinkAgent"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        ) == ERROR_SUCCESS
    }
}

fn apply_autostart(enabled: bool) -> Result<()> {
    let agent = agent_exe();
    let tray = std::env::current_exe().context("chemin de wcl-tray.exe")?;
    unsafe {
        let mut key = HKEY::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            run_key(),
            Some(0),
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if status != ERROR_SUCCESS {
            return Err(anyhow!("RegCreateKeyExW: {status:?}"));
        }
        if enabled {
            set_run_value(key, w!("WebcamLinkAgent"), &agent)?;
            set_run_value(key, w!("WebcamLinkTray"), &tray)?;
        } else {
            let _ = RegDeleteValueW(key, w!("WebcamLinkAgent"));
            let _ = RegDeleteValueW(key, w!("WebcamLinkTray"));
        }
        let _ = RegCloseKey(key);
    }
    Ok(())
}

unsafe fn set_run_value(key: HKEY, name: PCWSTR, path: &PathBuf) -> Result<()> {
    let value = format!("\"{}\"", path.display());
    let wide: Vec<u16> = value.encode_utf16().collect();
    let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
    let status = RegSetValueExW(key, name, Some(0), REG_SZ, Some(bytes));
    if status != ERROR_SUCCESS {
        return Err(anyhow!("RegSetValueExW({value}) : {status:?}"));
    }
    Ok(())
}
