//! `%ProgramData%\WebcamLink\config.ini` — réglages partagés par la DLL de
//! webcam virtuelle (`pc/vcam/src/config.cpp`), l'agent et l'icône de
//! notification.
//!
//! Format INI minimal, sans dépendance :
//!
//! ```ini
//! [video]
//! width=1920
//! height=1080
//! fps=30
//! bitrate_kbps=20000
//! [camera]
//! preference=auto
//! mirror=0
//! [image]
//! zoom_x100=100
//! exposure_mode=0
//! focus_mode=0
//! wb_mode=0
//! ```
//!
//! `caps.ini`, dans le même dossier, va dans l'autre sens : l'agent y écrit ce
//! que la caméra de l'iPhone annonce (`ImageCaps`), et la fenêtre de réglages y
//! lit les bornes de ses curseurs.

use std::fmt;
use std::io;
use std::path::PathBuf;

pub const DEFAULT_WIDTH: u32 = 1920;
pub const DEFAULT_HEIGHT: u32 = 1080;
pub const DEFAULT_FPS: u32 = 30;
pub const DEFAULT_BITRATE_KBPS: u32 = 20_000;

/// Bornes partagées avec la DLL (mêmes valeurs dans `config.cpp`).
pub const MIN_WIDTH: u32 = 160;
pub const MAX_WIDTH: u32 = 3840;
pub const MIN_HEIGHT: u32 = 120;
pub const MAX_HEIGHT: u32 = 2160;
pub const MIN_FPS: u32 = 1;
pub const MAX_FPS: u32 = 60;
pub const MIN_BITRATE_KBPS: u32 = 500;
pub const MAX_BITRATE_KBPS: u32 = 100_000;

// Bornes des réglages d'image. Celles que la caméra annonce (`ImageCaps`) sont
// plus étroites et priment : ces valeurs ne font que garantir un fichier sain.
pub const DEFAULT_ZOOM_X100: u32 = 100;
pub const MIN_ZOOM_X100: u32 = 100;
pub const MAX_ZOOM_X100: u32 = 1000;
pub const MIN_EXPOSURE_BIAS_X100: i32 = -400;
pub const MAX_EXPOSURE_BIAS_X100: i32 = 400;
pub const MIN_ISO: u32 = 0;
pub const MAX_ISO: u32 = 12_800;
pub const MIN_EXPOSURE_US: u32 = 0;
pub const MAX_EXPOSURE_US: u32 = 1_000_000;
pub const MIN_FOCUS_POINT_X10000: u32 = 0;
pub const MAX_FOCUS_POINT_X10000: u32 = 10_000;
pub const MIN_FOCUS_DISTANCE_X100: u32 = 0;
pub const MAX_FOCUS_DISTANCE_X100: u32 = 100;
pub const MIN_WB_TEMP_K: u32 = 0;
pub const MAX_WB_TEMP_K: u32 = 10_000;
pub const MIN_WB_TINT_X100: i32 = -150;
pub const MAX_WB_TINT_X100: i32 = 150;
pub const MIN_TORCH_LEVEL: u32 = 0;
pub const MAX_TORCH_LEVEL: u32 = 100;
pub const MIN_MODE: u32 = 0;
pub const MAX_EXPOSURE_MODE: u32 = 2;
pub const MAX_FOCUS_MODE: u32 = 2;
pub const MAX_WB_MODE: u32 = 2;
pub const MAX_STABILIZATION: u32 = 2;

/// Caméra de l'iPhone à utiliser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraPreference {
    #[default]
    Auto,
    BackWide,
    BackUltraWide,
    BackTele,
    Front,
}

impl CameraPreference {
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::BackWide,
        Self::BackUltraWide,
        Self::BackTele,
        Self::Front,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::BackWide => "back_wide",
            Self::BackUltraWide => "back_ultra_wide",
            Self::BackTele => "back_tele",
            Self::Front => "front",
        }
    }

    /// Libellé affiché dans la fenêtre de réglages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatique",
            Self::BackWide => "Arrière — grand angle",
            Self::BackUltraWide => "Arrière — ultra grand angle",
            Self::BackTele => "Arrière — téléobjectif",
            Self::Front => "Avant",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "back_wide" => Self::BackWide,
            "back_ultra_wide" => Self::BackUltraWide,
            "back_tele" => Self::BackTele,
            "front" => Self::Front,
            _ => Self::Auto,
        }
    }
}

impl fmt::Display for CameraPreference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Réglages partagés.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub preference: CameraPreference,
    pub mirror: bool,
    pub zoom_x100: u32,
    pub exposure_mode: u8,
    pub exposure_bias_ev_x100: i16,
    pub iso: u16,
    pub exposure_duration_us: u32,
    pub focus_mode: u8,
    pub focus_x_x10000: u16,
    pub focus_y_x10000: u16,
    pub focus_distance_x100: u16,
    pub wb_mode: u8,
    pub wb_temp_k: u16,
    pub wb_tint_x100: i16,
    pub torch: bool,
    pub torch_level: u8,
    pub stabilization: u8,
    pub hdr: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            fps: DEFAULT_FPS,
            bitrate_kbps: DEFAULT_BITRATE_KBPS,
            preference: CameraPreference::Auto,
            mirror: false,
            zoom_x100: DEFAULT_ZOOM_X100,
            exposure_mode: 0,
            exposure_bias_ev_x100: 0,
            iso: 0,
            exposure_duration_us: 0,
            focus_mode: 0,
            focus_x_x10000: 5000,
            focus_y_x10000: 5000,
            focus_distance_x100: 100,
            wb_mode: 0,
            wb_temp_k: 0,
            wb_tint_x100: 0,
            torch: false,
            torch_level: 100,
            stabilization: 0,
            hdr: false,
        }
    }
}

impl Settings {
    /// Ramène les valeurs dans les bornes partagées avec la DLL.
    pub fn clamped(self) -> Self {
        Self {
            width: self.width.clamp(MIN_WIDTH, MAX_WIDTH),
            height: self.height.clamp(MIN_HEIGHT, MAX_HEIGHT),
            fps: self.fps.clamp(MIN_FPS, MAX_FPS),
            bitrate_kbps: self.bitrate_kbps.clamp(MIN_BITRATE_KBPS, MAX_BITRATE_KBPS),
            preference: self.preference,
            mirror: self.mirror,
            zoom_x100: self.zoom_x100.clamp(MIN_ZOOM_X100, MAX_ZOOM_X100),
            exposure_mode: self.exposure_mode.clamp(MIN_MODE as u8, MAX_EXPOSURE_MODE as u8),
            exposure_bias_ev_x100: self
                .exposure_bias_ev_x100
                .clamp(MIN_EXPOSURE_BIAS_X100 as i16, MAX_EXPOSURE_BIAS_X100 as i16),
            iso: self.iso.clamp(MIN_ISO as u16, MAX_ISO as u16),
            exposure_duration_us: self.exposure_duration_us.clamp(MIN_EXPOSURE_US, MAX_EXPOSURE_US),
            focus_mode: self.focus_mode.clamp(MIN_MODE as u8, MAX_FOCUS_MODE as u8),
            focus_x_x10000: self.focus_x_x10000.clamp(
                MIN_FOCUS_POINT_X10000 as u16,
                MAX_FOCUS_POINT_X10000 as u16,
            ),
            focus_y_x10000: self.focus_y_x10000.clamp(
                MIN_FOCUS_POINT_X10000 as u16,
                MAX_FOCUS_POINT_X10000 as u16,
            ),
            focus_distance_x100: self.focus_distance_x100.clamp(
                MIN_FOCUS_DISTANCE_X100 as u16,
                MAX_FOCUS_DISTANCE_X100 as u16,
            ),
            wb_mode: self.wb_mode.clamp(MIN_MODE as u8, MAX_WB_MODE as u8),
            wb_temp_k: self.wb_temp_k.clamp(MIN_WB_TEMP_K as u16, MAX_WB_TEMP_K as u16),
            wb_tint_x100: self
                .wb_tint_x100
                .clamp(MIN_WB_TINT_X100 as i16, MAX_WB_TINT_X100 as i16),
            torch: self.torch,
            torch_level: self.torch_level.clamp(MIN_TORCH_LEVEL as u8, MAX_TORCH_LEVEL as u8),
            stabilization: self.stabilization.clamp(MIN_MODE as u8, MAX_STABILIZATION as u8),
            hdr: self.hdr,
        }
    }

    /// Lit le contenu INI. Les clés absentes ou invalides gardent les défauts.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
                section = name.trim().to_ascii_lowercase();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match (section.as_str(), key.trim().to_ascii_lowercase().as_str()) {
                ("video", "width") => settings.width = parse_u32(value, settings.width),
                ("video", "height") => settings.height = parse_u32(value, settings.height),
                ("video", "fps") => settings.fps = parse_u32(value, settings.fps),
                ("video", "bitrate_kbps") => {
                    settings.bitrate_kbps = parse_u32(value, settings.bitrate_kbps)
                }
                ("camera", "preference") => settings.preference = CameraPreference::parse(value),
                ("camera", "mirror") => settings.mirror = parse_bool(value),
                ("image", "zoom_x100") => settings.zoom_x100 = parse_u32(value, settings.zoom_x100),
                ("image", "exposure_mode") => {
                    settings.exposure_mode = parse_u32(value, u32::from(settings.exposure_mode)) as u8
                }
                ("image", "exposure_bias_ev_x100") => {
                    settings.exposure_bias_ev_x100 =
                        parse_i32(value, i32::from(settings.exposure_bias_ev_x100)) as i16
                }
                ("image", "iso") => settings.iso = parse_u32(value, u32::from(settings.iso)) as u16,
                ("image", "exposure_us") => {
                    settings.exposure_duration_us = parse_u32(value, settings.exposure_duration_us)
                }
                ("image", "focus_mode") => {
                    settings.focus_mode = parse_u32(value, u32::from(settings.focus_mode)) as u8
                }
                ("image", "focus_x_x10000") => {
                    settings.focus_x_x10000 =
                        parse_u32(value, u32::from(settings.focus_x_x10000)) as u16
                }
                ("image", "focus_y_x10000") => {
                    settings.focus_y_x10000 =
                        parse_u32(value, u32::from(settings.focus_y_x10000)) as u16
                }
                ("image", "focus_distance_x100") => {
                    settings.focus_distance_x100 =
                        parse_u32(value, u32::from(settings.focus_distance_x100)) as u16
                }
                ("image", "wb_mode") => {
                    settings.wb_mode = parse_u32(value, u32::from(settings.wb_mode)) as u8
                }
                ("image", "wb_temp_k") => {
                    settings.wb_temp_k = parse_u32(value, u32::from(settings.wb_temp_k)) as u16
                }
                ("image", "wb_tint_x100") => {
                    settings.wb_tint_x100 =
                        parse_i32(value, i32::from(settings.wb_tint_x100)) as i16
                }
                ("image", "torch") => settings.torch = parse_bool(value),
                ("image", "torch_level") => {
                    settings.torch_level = parse_u32(value, u32::from(settings.torch_level)) as u8
                }
                ("image", "stabilization") => {
                    settings.stabilization = parse_u32(value, u32::from(settings.stabilization)) as u8
                }
                ("image", "hdr") => settings.hdr = parse_bool(value),
                _ => {}
            }
        }
        settings.clamped()
    }

    /// Contenu INI, tel que relu par la DLL et l'agent.
    pub fn to_ini(self) -> String {
        let settings = self.clamped();
        format!(
            "[video]\n\
             width={}\n\
             height={}\n\
             fps={}\n\
             bitrate_kbps={}\n\
             [camera]\n\
             preference={}\n\
             mirror={}\n\
             [image]\n\
             zoom_x100={}\n\
             exposure_mode={}\n\
             exposure_bias_ev_x100={}\n\
             iso={}\n\
             exposure_us={}\n\
             focus_mode={}\n\
             focus_x_x10000={}\n\
             focus_y_x10000={}\n\
             focus_distance_x100={}\n\
             wb_mode={}\n\
             wb_temp_k={}\n\
             wb_tint_x100={}\n\
             torch={}\n\
             torch_level={}\n\
             stabilization={}\n\
             hdr={}\n",
            settings.width,
            settings.height,
            settings.fps,
            settings.bitrate_kbps,
            settings.preference.as_str(),
            u8::from(settings.mirror),
            settings.zoom_x100,
            settings.exposure_mode,
            settings.exposure_bias_ev_x100,
            settings.iso,
            settings.exposure_duration_us,
            settings.focus_mode,
            settings.focus_x_x10000,
            settings.focus_y_x10000,
            settings.focus_distance_x100,
            settings.wb_mode,
            settings.wb_temp_k,
            settings.wb_tint_x100,
            u8::from(settings.torch),
            settings.torch_level,
            settings.stabilization,
            u8::from(settings.hdr),
        )
    }

    /// Réglages courants ; défauts si le fichier est absent ou illisible.
    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Écrit le fichier (le dossier est créé au besoin).
    pub fn save(self) -> io::Result<()> {
        let path = config_path();
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        std::fs::write(path, self.clamped().to_ini())
    }
}

fn parse_u32(value: &str, fallback: u32) -> u32 {
    value.trim().parse().unwrap_or(fallback)
}

fn parse_i32(value: &str, fallback: i32) -> i32 {
    value.trim().parse().unwrap_or(fallback)
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "oui" | "yes")
}

/// `%ProgramData%\WebcamLink`.
pub fn data_directory() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
        .join("WebcamLink")
}

/// `%ProgramData%\WebcamLink\config.ini`.
pub fn config_path() -> PathBuf {
    data_directory().join("config.ini")
}

/// `%ProgramData%\WebcamLink\vcam.log` (journal de la DLL / du Frame Server).
pub fn log_path() -> PathBuf {
    data_directory().join("vcam.log")
}

/// `%ProgramData%\WebcamLink\caps.ini` — ce que la caméra de l'iPhone annonce
/// (`ImageCaps`). L'agent l'écrit, la fenêtre de réglages le lit : c'est le seul
/// canal agent → icône, dans le sens inverse de `config.ini`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CameraCaps {
    pub camera_label: String,
    pub flags: u32,
    pub zoom_max_x100: u32,
    pub iso_min: u32,
    pub iso_max: u32,
    pub exposure_min_us: u32,
    pub exposure_max_us: u32,
    pub wb_min_k: u32,
    pub wb_max_k: u32,
    pub wb_tint_min_x100: i32,
    pub wb_tint_max_x100: i32,
    pub stabilization_modes: u32,
    pub last_error: String,
}

impl CameraCaps {
    pub fn supports(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// Ce que l'agent a écrit ; tout à zéro si le fichier est absent (aucun
    /// iPhone ne s'est encore annoncé).
    pub fn load() -> Self {
        std::fs::read_to_string(caps_path())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = caps_path();
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        std::fs::write(path, self.to_ini())
    }

    pub fn parse(text: &str) -> Self {
        let mut caps = Self::default();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
                section = name.trim().to_ascii_lowercase();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match (section.as_str(), key.trim().to_ascii_lowercase().as_str()) {
                ("camera", "label") => caps.camera_label = value.to_string(),
                ("caps", "flags") => caps.flags = parse_u32(value, 0),
                ("caps", "zoom_max_x100") => caps.zoom_max_x100 = parse_u32(value, 0),
                ("caps", "iso_min") => caps.iso_min = parse_u32(value, 0),
                ("caps", "iso_max") => caps.iso_max = parse_u32(value, 0),
                ("caps", "exposure_min_us") => caps.exposure_min_us = parse_u32(value, 0),
                ("caps", "exposure_max_us") => caps.exposure_max_us = parse_u32(value, 0),
                ("caps", "wb_min_k") => caps.wb_min_k = parse_u32(value, 0),
                ("caps", "wb_max_k") => caps.wb_max_k = parse_u32(value, 0),
                ("caps", "wb_tint_min_x100") => caps.wb_tint_min_x100 = parse_i32(value, 0),
                ("caps", "wb_tint_max_x100") => caps.wb_tint_max_x100 = parse_i32(value, 0),
                ("caps", "stabilization_modes") => caps.stabilization_modes = parse_u32(value, 0),
                ("status", "last_error") => caps.last_error = value.to_string(),
                _ => {}
            }
        }
        caps
    }

    pub fn to_ini(&self) -> String {
        format!(
            "[camera]\n\
             label={}\n\
             [caps]\n\
             flags={}\n\
             zoom_max_x100={}\n\
             iso_min={}\n\
             iso_max={}\n\
             exposure_min_us={}\n\
             exposure_max_us={}\n\
             wb_min_k={}\n\
             wb_max_k={}\n\
             wb_tint_min_x100={}\n\
             wb_tint_max_x100={}\n\
             stabilization_modes={}\n\
             [status]\n\
             last_error={}\n",
            self.camera_label,
            self.flags,
            self.zoom_max_x100,
            self.iso_min,
            self.iso_max,
            self.exposure_min_us,
            self.exposure_max_us,
            self.wb_min_k,
            self.wb_max_k,
            self.wb_tint_min_x100,
            self.wb_tint_max_x100,
            self.stabilization_modes,
            self.last_error,
        )
    }
}

/// `%ProgramData%\WebcamLink\caps.ini`.
pub fn caps_path() -> PathBuf {
    data_directory().join("caps.ini")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_file() {
        let text = "\
; commentaire
[video]
width=1280
height=720
fps=60
bitrate_kbps=30000
[camera]
preference=front
mirror=1
";
        let settings = Settings::parse(text);
        assert_eq!(settings.width, 1280);
        assert_eq!(settings.height, 720);
        assert_eq!(settings.fps, 60);
        assert_eq!(settings.bitrate_kbps, 30_000);
        assert_eq!(settings.preference, CameraPreference::Front);
        assert!(settings.mirror);
    }

    #[test]
    fn keeps_defaults_for_missing_or_invalid_keys() {
        let settings = Settings::parse("[video]\nwidth=abc\nheight=\nfps=nope\n");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn clamps_out_of_range_values() {
        let settings = Settings::parse("[video]\nwidth=99999\nheight=1\nfps=999\nbitrate_kbps=1\n");
        assert_eq!(settings.width, MAX_WIDTH);
        assert_eq!(settings.height, MIN_HEIGHT);
        assert_eq!(settings.fps, MAX_FPS);
        assert_eq!(settings.bitrate_kbps, MIN_BITRATE_KBPS);
    }

    #[test]
    fn round_trips_through_ini() {
        let settings = Settings {
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_kbps: 10_000,
            preference: CameraPreference::BackUltraWide,
            mirror: true,
            ..Settings::default()
        };
        assert_eq!(Settings::parse(&settings.to_ini()), settings);
        assert_eq!(Settings::default(), Settings::parse(""));
    }

    #[test]
    fn parses_and_clamps_image_settings() {
        let text = "\
[image]
zoom_x100=250
exposure_mode=2
exposure_bias_ev_x100=-75
iso=800
exposure_us=16666
focus_mode=1
focus_x_x10000=2500
focus_distance_x100=30
wb_mode=2
wb_temp_k=5600
wb_tint_x100=-20
torch=1
torch_level=80
stabilization=2
hdr=1
";
        let settings = Settings::parse(text);
        assert_eq!(settings.zoom_x100, 250);
        assert_eq!(settings.exposure_mode, 2);
        assert_eq!(settings.exposure_bias_ev_x100, -75);
        assert_eq!(settings.iso, 800);
        assert_eq!(settings.exposure_duration_us, 16_666);
        assert_eq!(settings.focus_mode, 1);
        assert_eq!(settings.focus_distance_x100, 30);
        assert_eq!(settings.wb_temp_k, 5600);
        assert_eq!(settings.wb_tint_x100, -20);
        assert!(settings.torch);
        assert_eq!(settings.torch_level, 80);
        assert_eq!(settings.stabilization, 2);
        assert!(settings.hdr);
        assert_eq!(settings, Settings::parse(&settings.to_ini()));

        let clamped = Settings::parse(
            "[image]\nzoom_x100=99\nexposure_mode=9\niso=99999\nwb_tint_x100=-9999\n",
        );
        assert_eq!(clamped.zoom_x100, MIN_ZOOM_X100);
        assert_eq!(clamped.exposure_mode, MAX_EXPOSURE_MODE as u8);
        assert_eq!(clamped.iso, MAX_ISO as u16);
        assert_eq!(clamped.wb_tint_x100, MIN_WB_TINT_X100 as i16);
    }

    #[test]
    fn caps_round_trip() {
        let caps = CameraCaps {
            camera_label: "Back Camera".to_string(),
            flags: 0x0005,
            zoom_max_x100: 800,
            iso_min: 25,
            iso_max: 3200,
            exposure_min_us: 25,
            exposure_max_us: 1_000_000,
            wb_min_k: 2000,
            wb_max_k: 9000,
            wb_tint_min_x100: -150,
            wb_tint_max_x100: 150,
            stabilization_modes: 0x03,
            last_error: "exposition manuelle non supportée".to_string(),
        };
        assert_eq!(CameraCaps::parse(&caps.to_ini()), caps);
        assert!(caps.supports(0x0001));
        assert!(!caps.supports(0x0002));
        assert_eq!(CameraCaps::parse(""), CameraCaps::default());
    }
}
