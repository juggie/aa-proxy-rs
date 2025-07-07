use bluer::Address;
use serde::de::{self, Deserializer, Error as DeError, Visitor};
use serde::Deserialize;
use std::fmt::{self, Display};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(clap::ValueEnum, Default, Debug, PartialEq, PartialOrd, Clone, Copy, Deserialize)]
pub enum HexdumpLevel {
    #[default]
    Disabled,
    DecryptedInput,
    RawInput,
    DecryptedOutput,
    RawOutput,
    All,
}

#[derive(Debug, Clone)]
pub struct UsbId {
    pub vid: u16,
    pub pid: u16,
}

impl std::str::FromStr for UsbId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err("Expected format VID:PID".to_string());
        }
        let vid = u16::from_str_radix(parts[0], 16).map_err(|e| e.to_string())?;
        let pid = u16::from_str_radix(parts[1], 16).map_err(|e| e.to_string())?;
        Ok(UsbId { vid, pid })
    }
}

impl<'de> Deserialize<'de> for UsbId {
    fn deserialize<D>(deserializer: D) -> Result<UsbId, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UsbIdVisitor;

        impl<'de> Visitor<'de> for UsbIdVisitor {
            type Value = UsbId;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string in the format VID:PID")
            }

            fn visit_str<E>(self, value: &str) -> Result<UsbId, E>
            where
                E: de::Error,
            {
                UsbId::from_str(value).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_str(UsbIdVisitor)
    }
}

pub fn empty_string_as_none<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: FromStr,
    T::Err: Display,
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    if s.trim().is_empty() {
        Ok(None)
    } else {
        T::from_str(&s).map(Some).map_err(DeError::custom)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub advertise: bool,
    pub debug: bool,
    pub hexdump_level: HexdumpLevel,
    pub disable_console_debug: bool,
    pub legacy: bool,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub connect: Option<Address>,
    pub logfile: PathBuf,
    pub stats_interval: u16,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub udc: Option<String>,
    pub iface: String,
    pub hostapd_conf: PathBuf,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub btalias: Option<String>,
    pub keepalive: bool,
    pub timeout_secs: u16,
    pub bt_timeout_secs: u16,
    pub mitm: bool,
    pub dpi: u16,
    pub remove_tap_restriction: bool,
    pub video_in_motion: bool,
    pub disable_media_sink: bool,
    pub disable_tts_sink: bool,
    pub developer_mode: bool,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub wired: Option<UsbId>,
    pub dhu: bool,
    pub ev: bool,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub ev_battery_logger: Option<PathBuf>,
    pub ev_battery_capacity: u64,
    pub ev_factor: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            advertise: false,
            debug: false,
            hexdump_level: HexdumpLevel::Disabled,
            disable_console_debug: false,
            legacy: true,
            connect: None,
            logfile: "/var/log/aa-proxy-rs.log".into(),
            stats_interval: 0,
            udc: None,
            iface: "wlan0".to_string(),
            hostapd_conf: "/var/run/hostapd.conf".into(),
            btalias: None,
            keepalive: false,
            timeout_secs: 10,
            bt_timeout_secs: 120,
            mitm: false,
            dpi: 0,
            remove_tap_restriction: false,
            video_in_motion: false,
            disable_media_sink: false,
            disable_tts_sink: false,
            developer_mode: false,
            wired: None,
            dhu: false,
            ev: false,
            ev_battery_logger: None,
            ev_battery_capacity: 22000,
            ev_factor: 0.075,
        }
    }
}

pub fn load_config(config_file: PathBuf) -> Result<AppConfig, Box<dyn std::error::Error>> {
    let file_config: AppConfig = config::Config::builder()
        .add_source(config::File::from(config_file).required(false))
        .build()?
        .try_deserialize()
        .unwrap_or_default();

    Ok(file_config)
}
