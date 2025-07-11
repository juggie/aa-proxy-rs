use bluer::Address;
use serde::de::{self, Deserializer, Error as DeError, Visitor};
use serde::{Deserialize, Serialize};
use simplelog::*;
use std::{
    fmt::{self, Display},
    fs, io,
    path::PathBuf,
    process::{Command, Stdio},
    str::FromStr,
    sync::Arc,
};
use tokio::sync::RwLock;
use toml_edit::{value, DocumentMut};

// module name for logging engine
const NAME: &str = "<i><bright-black> config: </>";

pub type SharedConfig = Arc<RwLock<AppConfig>>;

#[derive(
    clap::ValueEnum, Default, Debug, PartialEq, PartialOrd, Clone, Copy, Deserialize, Serialize,
)]
pub enum HexdumpLevel {
    #[default]
    Disabled,
    DecryptedInput,
    RawInput,
    DecryptedOutput,
    RawOutput,
    All,
}

#[derive(Debug, Clone, Serialize)]
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

impl fmt::Display for UsbId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}:{:x}", self.vid, self.pid)
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

fn webserver_default_bind() -> Option<String> {
    Some("0.0.0.0:80".into())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    #[serde(
        default = "webserver_default_bind",
        deserialize_with = "empty_string_as_none"
    )]
    pub webserver: Option<String>,
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

    #[serde(skip)]
    pub restart_requested: bool,
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
            webserver: webserver_default_bind(),
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
            restart_requested: false,
        }
    }
}

/// Remount `/` as readonly (`lock = true`) or read-write (`lock = false`)
fn remount_root(lock: bool) -> io::Result<()> {
    let mode = if lock { "remount,ro" } else { "remount,rw" };

    let status = Command::new("mount")
        .args(&["-o", mode, "/"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        info!(
            "{} Remount as {} successful",
            NAME,
            if lock { "read-only" } else { "read-write" }
        );
    } else {
        error!(
            "{} Remount as {} failed: {:?}",
            NAME,
            if lock { "read-only" } else { "read-write" },
            status
        );
    }

    Ok(())
}

impl AppConfig {
    pub fn load(config_file: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        use ::config::File;
        let file_config: AppConfig = ::config::Config::builder()
            .add_source(File::from(config_file).required(false))
            .build()?
            .try_deserialize()
            .unwrap_or_default();

        Ok(file_config)
    }

    pub fn save(&self, config_file: PathBuf) {
        debug!("Saving config: {:?}", self);
        let raw = fs::read_to_string(&config_file).unwrap_or_default();
        let mut doc = raw.parse::<DocumentMut>().unwrap_or_else(|_| {
            // if the file doesn't exists or there is parse error, create a new one
            DocumentMut::new()
        });

        doc["advertise"] = value(self.advertise);
        doc["debug"] = value(self.debug);
        doc["hexdump_level"] = value(format!("{:?}", self.hexdump_level));
        doc["disable_console_debug"] = value(self.disable_console_debug);
        doc["legacy"] = value(self.legacy);
        doc["connect"] = match &self.connect {
            Some(c) => value(c.to_string()),
            None => value(""),
        };
        doc["logfile"] = value(self.logfile.display().to_string());
        doc["stats_interval"] = value(self.stats_interval as i64);
        if let Some(udc) = &self.udc {
            doc["udc"] = value(udc);
        }
        doc["iface"] = value(&self.iface);
        doc["hostapd_conf"] = value(self.hostapd_conf.display().to_string());
        if let Some(alias) = &self.btalias {
            doc["btalias"] = value(alias);
        }
        doc["keepalive"] = value(self.keepalive);
        doc["timeout_secs"] = value(self.timeout_secs as i64);
        if let Some(webserver) = &self.webserver {
            doc["webserver"] = value(webserver);
        }
        doc["bt_timeout_secs"] = value(self.bt_timeout_secs as i64);
        doc["mitm"] = value(self.mitm);
        doc["dpi"] = value(self.dpi as i64);
        doc["remove_tap_restriction"] = value(self.remove_tap_restriction);
        doc["video_in_motion"] = value(self.video_in_motion);
        doc["disable_media_sink"] = value(self.disable_media_sink);
        doc["disable_tts_sink"] = value(self.disable_tts_sink);
        doc["developer_mode"] = value(self.developer_mode);
        doc["wired"] = value(
            self.wired
                .as_ref()
                .map_or("".to_string(), |w| w.to_string()),
        );
        doc["dhu"] = value(self.dhu);
        doc["ev"] = value(self.ev);
        if let Some(path) = &self.ev_battery_logger {
            doc["ev_battery_logger"] = value(path.display().to_string());
        }
        doc["ev_battery_capacity"] = value(self.ev_battery_capacity as i64);
        doc["ev_factor"] = value(self.ev_factor as f64);

        let _ = remount_root(false);
        info!(
            "{} Saving new configuration to file: {}",
            NAME,
            config_file.display()
        );
        let _ = fs::write(config_file, doc.to_string());
        let _ = remount_root(true);
    }
}
