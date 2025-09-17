use aa_proxy_rs::bluetooth::bluetooth_setup_connection;
use aa_proxy_rs::bluetooth::bluetooth_stop;
use aa_proxy_rs::config::SharedConfig;
use aa_proxy_rs::config::SharedConfigJson;
use aa_proxy_rs::config::WifiConfig;
use aa_proxy_rs::config::{Action, AppConfig};
use aa_proxy_rs::config::{DEFAULT_WLAN_ADDR, TCP_SERVER_PORT};
use aa_proxy_rs::io_uring::io_loop;
use aa_proxy_rs::led::{LedColor, LedManager, LedMode};
use aa_proxy_rs::mitm::Packet;
use aa_proxy_rs::usb_gadget::uevent_listener;
use aa_proxy_rs::usb_gadget::UsbGadgetState;
use aa_proxy_rs::web;
use clap::Parser;
use humantime::format_duration;
use simple_config_parser::Config;
use simplelog::*;
use std::os::unix::fs::PermissionsExt;

use std::fs;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Builder;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::time::Instant;

use std::net::SocketAddr;
use tokio::sync::RwLock;

// Just a generic Result type to ease error handling for us. Errors in multithreaded
// async contexts needs some extra restrictions
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

// module name for logging engine
const NAME: &str = "<i><bright-black> main: </>";
const HOSTAPD_CONF_IN: &str = "/etc/hostapd.conf.in";
const HOSTAPD_CONF_OUT: &str = "/var/run/hostapd.conf";
const UMTPRD_CONF_IN: &str = "/etc/umtprd/umtprd.conf.in";
const UMTPRD_CONF_OUT: &str = "/var/run/umtprd.conf";
const GADGET_INIT_IN: &str = "/etc/S92usb_gadget.in";
const GADGET_INIT_OUT: &str = "/var/run/S92usb_gadget";
const REBOOT_CMD: &str = "/sbin/reboot";

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

    /// Generate system config and exit
    #[clap(short, long)]
    generate_system_config: bool,
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
        .expect(&format!("mac_address_by_name for {:?}", iface))
        .expect(&format!("No MAC address found for interface: {:?}", iface))
        .to_string();

    // Create a new config from hostapd.conf
    let hostapd = Config::new().file(&hostapd_conf).expect(&format!(
        "Unable to open hostapd config: {:?}",
        hostapd_conf
    ));

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

async fn enable_usb_if_present(usb: &mut Option<UsbGadgetState>, accessory_started: Arc<Notify>) {
    if let Some(ref mut usb) = usb {
        usb.enable_default_and_wait_for_accessory(accessory_started)
            .await;
    }
}

async fn action_handler(config: &mut SharedConfig) {
    // check pending action
    let action = config.read().await.action_requested.clone();
    if let Some(action) = action {
        // check if we need to reboot
        if action == Action::Reboot {
            config.write().await.action_requested = None;
            info!("{} 🔁 Rebooting now!", NAME);
            let _ = Command::new(REBOOT_CMD).spawn();
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }
}

async fn tokio_main(
    config: SharedConfig,
    config_json: SharedConfigJson,
    need_restart: Arc<Notify>,
    tcp_start: Arc<Notify>,
    config_file: PathBuf,
    tx: Arc<Mutex<Option<Sender<Packet>>>>,
    sensor_channel: Arc<Mutex<Option<u8>>>,
    led_support: bool,
) -> Result<()> {
    let accessory_started = Arc::new(Notify::new());
    let accessory_started_cloned = accessory_started.clone();

    // LED support
    let mut led_manager = if led_support {
        Some(LedManager::new(100))
    } else {
        None
    };

    let cfg = config.read().await.clone();
    if let Some(ref bindaddr) = cfg.webserver {
        // preparing AppState and starting webserver
        let state = web::AppState {
            config: config.clone(),
            config_json: config_json.clone(),
            config_file: config_file.into(),
            tx,
            sensor_channel,
        };
        let app = web::app(state.into());

        match bindaddr.parse::<SocketAddr>() {
            Ok(addr) => {
                let server = hyper::Server::bind(&addr).serve(app.into_make_service());

                // run webserver in separate task
                tokio::spawn(async move {
                    if let Err(e) = server.await {
                        error!("{} webserver starting error: {}", NAME, e);
                    }
                });

                info!("{} webserver running at http://{addr}/", NAME);
            }
            Err(e) => {
                error!("{} webserver address/port parse: {}", NAME, e);
            }
        }
    }

    let wifi_conf = {
        if !cfg.wired.is_some() {
            Some(init_wifi_config(&cfg.iface, cfg.hostapd_conf.clone()))
        } else {
            None
        }
    };
    let mut usb = None;
    if !cfg.dhu {
        if cfg.legacy {
            // start uevent listener in own task
            std::thread::spawn(|| uevent_listener(accessory_started_cloned));
        }
        usb = Some(UsbGadgetState::new(cfg.legacy, cfg.udc.clone()));
    }

    // spawn a background task for reboot detection
    let mut config_cloned = config.clone();
    let _ = tokio::spawn(async move {
        loop {
            action_handler(&mut config_cloned).await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });

    let change_usb_order = cfg.change_usb_order;
    loop {
        if let Some(ref mut leds) = led_manager {
            leds.set_led(LedColor::Green, LedMode::Heartbeat).await;
        }
        if let Some(ref mut usb) = usb {
            if let Err(e) = usb.init() {
                error!("{} 🔌 USB init error: {}", NAME, e);
            }
        }

        let mut bt_stop = None;

        if change_usb_order {
            enable_usb_if_present(&mut usb, accessory_started.clone()).await;
        }

        if let Some(ref wifi_conf) = wifi_conf {
            loop {
                // read and clone the effective config in advance to avoid holding the lock
                let cfg = config.read().await.clone();
                let stopped = cfg.action_requested == Some(Action::Stop);

                match bluetooth_setup_connection(
                    cfg.advertise,
                    cfg.dongle_mode,
                    cfg.btalias,
                    cfg.connect,
                    wifi_conf.clone(),
                    tcp_start.clone(),
                    Duration::from_secs(cfg.bt_timeout_secs.into()),
                    stopped,
                )
                .await
                {
                    Ok(state) => {
                        // we're ready, gracefully shutdown bluetooth in task
                        bt_stop = Some(tokio::spawn(async move { bluetooth_stop(state).await }));
                        // user connected manually, clear stopped action flag
                        if stopped {
                            config.write().await.action_requested = None;
                        }
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

        if !change_usb_order {
            enable_usb_if_present(&mut usb, accessory_started.clone()).await;
        }

        if let Some(bt_stop) = bt_stop {
            // wait for bluetooth stop properly
            let _ = bt_stop.await;
        }

        // inform via LED about successful connection
        if let Some(ref mut leds) = led_manager {
            leds.set_led(LedColor::Blue, LedMode::On).await;
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

/// Returns the SBC model string (currently supports only Raspberry Pi)
pub fn get_sbc_model() -> Result<String> {
    Ok(fs::read_to_string("/sys/firmware/devicetree/base/model")?
        .trim_end_matches(char::from(0))
        .trim()
        .to_string())
}

/// Returns the full device serial number from Device Tree
pub fn get_serial_number() -> Result<String> {
    Ok(
        fs::read_to_string("/sys/firmware/devicetree/base/serial-number")?
            .trim_end_matches(char::from(0))
            .trim()
            .to_string(),
    )
}

fn render_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut output = template.to_string();
    for (key, value) in vars {
        let placeholder = format!("{{{{{}}}}}", key);
        output = output.replace(&placeholder, value);
    }
    output
}

fn generate_hostapd_conf(config: AppConfig) -> std::io::Result<()> {
    info!(
        "{} 🗃️ Generating config from input template: <bold><green>{}</>",
        NAME, HOSTAPD_CONF_IN
    );

    // Technically for IEEE802.11g we have to use g but AFAIK b is fine.
    let hostapd_mode = if config.band == 5.0 || config.band == 6.0 {
        "a"
    } else {
        "b"
    };

    let template = fs::read_to_string(HOSTAPD_CONF_IN)?;

    // Eventually: For 6 GHz, we will need more options like opclass.
    let rendered = render_template(
        &template,
        &[
            ("HW_MODE", hostapd_mode),
            ("BE_MODE", if config.wifi_version >= 7 { "1" } else { "0" }),
            ("AX_MODE", if config.wifi_version >= 6 { "1" } else { "0" }),
            ("AC_MODE", if config.wifi_version >= 5 { "1" } else { "0" }),
            ("N_MODE", if config.wifi_version >= 4 { "1" } else { "0" }),
            ("COUNTRY_CODE", &config.country_code),
            ("CHANNEL", &config.channel.to_string()),
            ("SSID", &config.ssid),
            ("WPA_PASSPHRASE", &config.wpa_passphrase),
        ],
    );

    info!(
        "{} 💾 Saving generated file as: <bold><green>{}</>",
        NAME, HOSTAPD_CONF_OUT
    );
    fs::write(HOSTAPD_CONF_OUT, rendered)
}

fn generate_usb_strings(input: &str, output: &str) -> std::io::Result<()> {
    info!(
        "{} 🗃️ Generating config from input template: <bold><green>{}</>",
        NAME, input
    );

    let template = fs::read_to_string(input)?;

    let rendered = render_template(
        &template,
        &[
            (
                "MODEL",
                &get_sbc_model().map_or(String::new(), |model| format!(" ({})", model)),
            ),
            (
                "SERIAL",
                &get_serial_number().unwrap_or("0123456".to_string()),
            ),
            (
                "FIRMWARE_VER",
                &format!(
                    "{}, git: {}-{}",
                    env!("BUILD_DATE"),
                    env!("GIT_DATE"),
                    env!("GIT_HASH")
                ),
            ),
        ],
    );

    info!(
        "{} 💾 Saving generated file as: <bold><green>{}</>",
        NAME, output
    );
    fs::write(output, rendered)
}

fn main() -> Result<()> {
    let started = Instant::now();

    // CLI arguments
    let args = Args::parse();

    // parse config
    let config = match AppConfig::load(args.config.clone()) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!(
                "Failed to start aa-proxy-rs due to invalid configuration in: {}.  Error:\n{}",
                args.config.display(),
                e
            );
            std::process::exit(1);
        }
    };
    let config_json = AppConfig::load_config_json().expect("Invalid embedded config.json");

    logging_init(config.debug, config.disable_console_debug, &config.logfile);
    info!(
        "🛸 <b><blue>aa-proxy-rs</> is starting, build: {}, git: {}-{}",
        env!("BUILD_DATE"),
        env!("GIT_DATE"),
        env!("GIT_HASH")
    );

    // generate system configs from template and exit
    if args.generate_system_config {
        generate_hostapd_conf(config).expect("error generating config from template");

        generate_usb_strings(UMTPRD_CONF_IN, UMTPRD_CONF_OUT)
            .expect("error generating config from template");

        generate_usb_strings(GADGET_INIT_IN, GADGET_INIT_OUT)
            .expect("error generating config from template");
        // make a script executable
        info!(
            "{} 🚀 Making script executable: <bold><green>{}</>",
            NAME, GADGET_INIT_OUT
        );
        let mut perms = fs::metadata(GADGET_INIT_OUT)?.permissions();
        perms.set_mode(0o755); // rwxr-xr-x
        fs::set_permissions(GADGET_INIT_OUT, perms)?;

        return Ok(());
    }

    // show SBC model
    let mut led_support = false;
    if let Ok(model) = get_sbc_model() {
        info!("{} 📟 host device: <bold><blue>{}</>", NAME, model);
        if model == "AAWireless 2B" {
            led_support = true;
        }
    }

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

    // notify for syncing threads
    let need_restart = Arc::new(Notify::new());
    let need_restart_cloned = need_restart.clone();
    let tcp_start = Arc::new(Notify::new());
    let tcp_start_cloned = tcp_start.clone();
    let config = Arc::new(RwLock::new(config));
    let config_json = Arc::new(RwLock::new(config_json));
    let config_cloned = config.clone();
    let tx = Arc::new(Mutex::new(None));
    let tx_cloned = tx.clone();
    let sensor_channel = Arc::new(Mutex::new(None));
    let sensor_channel_cloned = sensor_channel.clone();

    // build and spawn main tokio runtime
    let runtime = Builder::new_multi_thread().enable_all().build().unwrap();
    runtime.spawn(async move {
        tokio_main(
            config_cloned,
            config_json.clone(),
            need_restart,
            tcp_start,
            args.config.clone(),
            tx_cloned,
            sensor_channel_cloned,
            led_support,
        )
        .await
    });

    // start tokio_uring runtime simultaneously
    let _ = tokio_uring::start(io_loop(
        need_restart_cloned,
        tcp_start_cloned,
        config,
        tx,
        sensor_channel,
    ));

    info!(
        "🚩 aa-proxy-rs terminated, running time: {}",
        format_duration(started.elapsed()).to_string()
    );

    Ok(())
}
