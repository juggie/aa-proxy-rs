mod aoa;
mod bluetooth;
mod io_uring;
mod mitm;
mod usb_gadget;
mod usb_stream;

use bluer::Address;
use bluetooth::bluetooth_setup_connection;
use bluetooth::bluetooth_stop;
use clap::Parser;
use humantime::format_duration;
use io_uring::io_loop;
use simple_config_parser::Config;
use simplelog::*;
use usb_gadget::uevent_listener;
use usb_gadget::UsbGadgetState;

use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;
use std::fmt;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Builder;
use tokio::sync::Notify;
use tokio::time::Instant;

// module name for logging engine
const NAME: &str = "<i><bright-black> main: </>";

const DEFAULT_WLAN_ADDR: &str = "10.0.0.1";
const TCP_SERVER_PORT: i32 = 5288;
const TCP_DHU_PORT: i32 = 5277;

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
struct UsbId {
    vid: u16,
    pid: u16,
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

/// AndroidAuto wired/wireless proxy
#[derive(Parser, Debug)]
#[clap(version, long_about = None, about = format!(
    "🛸 aa-proxy-rs, build: {}, git: {}-{}",
    env!("BUILD_DATE"),
    env!("GIT_DATE"),
    env!("GIT_HASH")
))]
struct Args {
    /// Config file path
    #[clap(
        short,
        long,
        value_parser,
        default_value = "/etc/aa-proxy-rs/config.toml"
    )]
    config: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    advertise: bool,
    debug: bool,
    hexdump_level: HexdumpLevel,
    disable_console_debug: bool,
    legacy: bool,
    connect: Option<Address>,
    logfile: PathBuf,
    stats_interval: u16,
    udc: Option<String>,
    iface: String,
    hostapd_conf: PathBuf,
    btalias: Option<String>,
    keepalive: bool,
    timeout_secs: u16,
    bt_timeout_secs: u16,
    mitm: bool,
    dpi: Option<u16>,
    remove_tap_restriction: bool,
    video_in_motion: bool,
    disable_media_sink: bool,
    disable_tts_sink: bool,
    developer_mode: bool,
    wired: Option<UsbId>,
    dhu: bool,
    ev: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            advertise: false,
            debug: false,
            hexdump_level: HexdumpLevel::Disabled,
            disable_console_debug: false,
            legacy: false,
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
            dpi: None,
            remove_tap_restriction: false,
            video_in_motion: false,
            disable_media_sink: false,
            disable_tts_sink: false,
            developer_mode: false,
            wired: None,
            dhu: false,
            ev: false,
        }
    }
}

fn load_config(config_file: PathBuf) -> Result<AppConfig, Box<dyn std::error::Error>> {
    let file_config: AppConfig = config::Config::builder()
        .add_source(config::File::from(config_file).required(false))
        .build()?
        .try_deserialize()
        .unwrap_or_default();

    Ok(file_config)
}

#[derive(Clone)]
struct WifiConfig {
    ip_addr: String,
    port: i32,
    ssid: String,
    bssid: String,
    wpa_key: String,
}

fn init_wifi_config(iface: &str, hostapd_conf: PathBuf) -> WifiConfig {
    let mut ip_addr = String::from(DEFAULT_WLAN_ADDR);

    // Get UP interface and IP
    for ifa in netif::up().unwrap() {
        match ifa.name() {
            val if val == iface => {
                debug!("Found interface: {:?}", ifa);
                // IPv4 Address contains None scope_id, while IPv6 contains Some
                match ifa.scope_id() {
                    None => {
                        ip_addr = ifa.address().to_string();
                        break;
                    }
                    _ => (),
                }
            }
            _ => (),
        }
    }

    let bssid = mac_address::mac_address_by_name(iface)
        .unwrap()
        .unwrap()
        .to_string();

    // Create a new config from hostapd.conf
    let hostapd = Config::new().file(hostapd_conf).unwrap();

    // read SSID and WPA_KEY
    let ssid = &hostapd.get_str("ssid").unwrap();
    let wpa_key = &hostapd.get_str("wpa_passphrase").unwrap();

    WifiConfig {
        ip_addr,
        port: TCP_SERVER_PORT,
        ssid: ssid.into(),
        bssid,
        wpa_key: wpa_key.into(),
    }
}

fn logging_init(debug: bool, disable_console_debug: bool, log_path: &PathBuf) {
    let conf = ConfigBuilder::new()
        .set_time_format("%F, %H:%M:%S%.3f".to_string())
        .set_write_log_enable_colors(true)
        .build();

    let mut loggers = vec![];

    let requested_level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    let console_logger: Box<dyn SharedLogger> = TermLogger::new(
        {
            if disable_console_debug {
                LevelFilter::Info
            } else {
                requested_level
            }
        },
        conf.clone(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    );
    loggers.push(console_logger);

    let mut logfile_error: Option<String> = None;
    let logfile = OpenOptions::new().create(true).append(true).open(&log_path);
    match logfile {
        Ok(logfile) => {
            loggers.push(WriteLogger::new(requested_level, conf, logfile));
        }
        Err(e) => {
            logfile_error = Some(format!(
                "Error creating/opening log file: {:?}: {:?}",
                log_path, e
            ));
        }
    }

    CombinedLogger::init(loggers).expect("Cannot initialize logging subsystem");
    if logfile_error.is_some() {
        error!("{} {}", NAME, logfile_error.unwrap());
        warn!("{} Will do console logging only...", NAME);
    }
}

async fn tokio_main(config: AppConfig, need_restart: Arc<Notify>, tcp_start: Arc<Notify>) {
    let accessory_started = Arc::new(Notify::new());
    let accessory_started_cloned = accessory_started.clone();

    let wifi_conf = {
        if !config.wired.is_some() {
            Some(init_wifi_config(&config.iface, config.hostapd_conf))
        } else {
            None
        }
    };
    let mut usb = None;
    if !config.dhu {
        if config.legacy {
            // start uevent listener in own task
            std::thread::spawn(|| uevent_listener(accessory_started_cloned));
        }
        usb = Some(UsbGadgetState::new(config.legacy, config.udc));
    }
    loop {
        if let Some(ref mut usb) = usb {
            if let Err(e) = usb.init() {
                error!("{} 🔌 USB init error: {}", NAME, e);
            }
        }

        let mut bt_stop = None;
        if let Some(ref wifi_conf) = wifi_conf {
            loop {
                match bluetooth_setup_connection(
                    config.advertise,
                    config.btalias.clone(),
                    config.connect,
                    wifi_conf.clone(),
                    tcp_start.clone(),
                    config.keepalive,
                    Duration::from_secs(config.bt_timeout_secs.into()),
                )
                .await
                {
                    Ok(state) => {
                        // we're ready, gracefully shutdown bluetooth in task
                        bt_stop = Some(tokio::spawn(async move { bluetooth_stop(state).await }));
                        break;
                    }
                    Err(e) => {
                        error!("{} Bluetooth error: {}", NAME, e);
                        info!("{} Trying to recover...", NAME);
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }

        if let Some(ref mut usb) = usb {
            usb.enable_default_and_wait_for_accessory(accessory_started.clone())
                .await;
        }

        if let Some(bt_stop) = bt_stop {
            // wait for bluetooth stop properly
            let _ = bt_stop.await;
        }

        // wait for restart
        need_restart.notified().await;

        // TODO: make proper main loop with cancelation
        info!(
            "{} 📵 TCP/USB connection closed or not started, trying again...",
            NAME
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

fn main() {
    let started = Instant::now();

    // CLI arguments
    let args = Args::parse();

    // parse config
    let config = load_config(args.config.clone()).unwrap();

    logging_init(config.debug, config.disable_console_debug, &config.logfile);
    info!(
        "🛸 <b><blue>aa-proxy-rs</> is starting, build: {}, git: {}-{}",
        env!("BUILD_DATE"),
        env!("GIT_DATE"),
        env!("GIT_HASH")
    );

    // check and display config
    if args.config.exists() {
        info!(
            "{} ⚙️ config loaded from file: {}",
            NAME,
            args.config.display()
        );
    } else {
        warn!(
            "{} ⚙️ config file: {} doesn't exist, defaults used",
            NAME,
            args.config.display()
        );
    }
    debug!("{} ⚙️ startup configuration: {:#?}", NAME, config);

    let stats_interval = {
        if config.stats_interval == 0 {
            None
        } else {
            Some(Duration::from_secs(config.stats_interval.into()))
        }
    };
    let read_timeout = Duration::from_secs(config.timeout_secs.into());

    if let Some(ref wired) = config.wired {
        info!(
            "{} 🔌 enabled wired USB connection with {:04X?}",
            NAME, wired
        );
    }
    info!(
        "{} 📜 Log file path: <b><green>{}</>",
        NAME,
        config.logfile.display()
    );
    info!(
        "{} ⚙️ Showing transfer statistics: <b><blue>{}</>",
        NAME,
        match stats_interval {
            Some(d) => format_duration(d).to_string(),
            None => "disabled".to_string(),
        }
    );

    // notify for syncing threads
    let need_restart = Arc::new(Notify::new());
    let need_restart_cloned = need_restart.clone();
    let tcp_start = Arc::new(Notify::new());
    let tcp_start_cloned = tcp_start.clone();
    let mitm = config.mitm;
    let dpi = config.dpi;
    let developer_mode = config.developer_mode;
    let disable_media_sink = config.disable_media_sink;
    let disable_tts_sink = config.disable_tts_sink;
    let remove_tap_restriction = config.remove_tap_restriction;
    let video_in_motion = config.video_in_motion;
    let hex_requested = config.hexdump_level;
    let wired = config.wired.clone();
    let dhu = config.dhu;

    // build and spawn main tokio runtime
    let runtime = Builder::new_multi_thread().enable_all().build().unwrap();
    runtime.spawn(async move { tokio_main(config, need_restart, tcp_start).await });

    // start tokio_uring runtime simultaneously
    let _ = tokio_uring::start(io_loop(
        stats_interval,
        need_restart_cloned,
        tcp_start_cloned,
        read_timeout,
        mitm,
        dpi,
        developer_mode,
        disable_media_sink,
        disable_tts_sink,
        remove_tap_restriction,
        video_in_motion,
        hex_requested,
        wired,
        dhu,
    ));

    info!(
        "🚩 aa-proxy-rs terminated, running time: {}",
        format_duration(started.elapsed()).to_string()
    );
}
