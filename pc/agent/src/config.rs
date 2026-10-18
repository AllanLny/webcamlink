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
//! ```

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
                _ => {}
            }
        }
        settings.clamped()
    }

    /// Contenu INI, tel que relu par la DLL et l'agent.
    pub fn to_ini(self) -> String {
        format!(
            "[video]\nwidth={}\nheight={}\nfps={}\nbitrate_kbps={}\n[camera]\npreference={}\nmirror={}\n",
            self.width,
            self.height,
            self.fps,
            self.bitrate_kbps,
            self.preference.as_str(),
            u8::from(self.mirror),
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
        };
        assert_eq!(Settings::parse(&settings.to_ini()), settings);
        assert_eq!(Settings::default(), Settings::parse(""));
    }
}
