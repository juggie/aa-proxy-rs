use crate::TCP_SERVER_PORT;
use bluer::adv::Advertisement;
use bluer::{
    adv::AdvertisementHandle,
    agent::{Agent, AgentHandle},
    rfcomm::{Profile, ProfileHandle, Role, Stream},
    Adapter,
};
use futures::StreamExt;
use simplelog::*;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
use protobuf::Message;
use WifiInfoResponse::AccessPointType;
use WifiInfoResponse::SecurityMode;
const HEADER_LEN: usize = 4;

// module name for logging engine
const NAME: &str = "<i><bright-black> bluetooth: </>";

// Just a generic Result type to ease error handling for us. Errors in multithreaded
// async contexts needs some extra restrictions
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const AAWG_PROFILE_UUID: &str = "4de17a00-52cb-11e6-bdf4-0800200c9a66";
const HSP_HS_UUID: &str = "00001108-0000-1000-8000-00805f9b34fb";
const BT_ALIAS: &str = "WirelessAADongle";

const WLAN_IFACE: &str = "wlan0";
const WLAN_IP_ADDR: &str = "10.0.0.1";
const WLAN_SSID: &str = "AAWirelessDongle";
const WLAN_WPA_KEY: &str = "ConnectAAWirelessDongle";

#[derive(Debug, Clone)]
#[repr(u16)]
#[allow(unused)]
enum MessageId {
    WifiStartRequest = 1,
    WifiInfoRequest = 2,
    WifiInfoResponse = 3,
    WifiVersionRequest = 4,
    WifiVersionResponse = 5,
    WifiConnectStatus = 6,
    WifiStartResponse = 7,
}

pub struct BluetoothState {
    adapter: Adapter,
    handle_ble: AdvertisementHandle,
    handle_aa: ProfileHandle,
    handle_hsp: JoinHandle<Result<ProfileHandle>>,
    handle_agent: AgentHandle,
}

pub async fn get_cpu_serial_number_suffix() -> Result<String> {
    let mut serial = String::new();
    let contents = tokio::fs::read_to_string("/sys/firmware/devicetree/base/serial-number").await?;
    // check if we read the serial number with correct length
    if contents.len() == 17 {
        serial = (&contents[10..16]).to_string();
    }
    Ok(serial)
}

async fn power_up_and_wait_for_connection() -> Result<(BluetoothState, Stream)> {
    // setting BT alias for further use
    let alias = match get_cpu_serial_number_suffix().await {
        Ok(suffix) => format!("{}-{}", BT_ALIAS, suffix),
        Err(_) => String::from(BT_ALIAS),
    };
    info!("{} 🥏 Bluetooth alias: <bold><green>{}</>", NAME, alias);

    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    info!(
        "{} 🥏 Opened bluetooth adapter <b>{}</> with address <b>{}</b>",
        NAME,
        adapter.name(),
        adapter.address().await?
    );
    adapter.set_alias(alias.clone()).await?;
    adapter.set_powered(true).await?;
    adapter.set_pairable(true).await?;

    // Perform a Bluetooth LE advertisement
    info!("{} 📣 BLE Advertisement started", NAME);
    let le_advertisement = Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        service_uuids: vec![AAWG_PROFILE_UUID.parse()?].into_iter().collect(),
        discoverable: Some(true),
        local_name: Some(alias),
        ..Default::default()
    };
    let handle_ble = adapter.advertise(le_advertisement).await?;

    // Default agent is probably needed when pairing for the first time
    let agent = Agent::default();
    let handle_agent = session.register_agent(agent).await?;

    // AA Wireless profile
    let profile = Profile {
        uuid: AAWG_PROFILE_UUID.parse()?,
        name: Some("AA Wireless".to_string()),
        channel: Some(8),
        role: Some(Role::Server),
        require_authentication: Some(false),
        require_authorization: Some(false),
        ..Default::default()
    };
    let mut handle_aa = session.register_profile(profile).await?;
    info!("{} 📱 AA Wireless Profile: registered", NAME);

    // Headset profile
    let profile = Profile {
        uuid: HSP_HS_UUID.parse()?,
        name: Some("HSP HS".to_string()),
        require_authentication: Some(false),
        require_authorization: Some(false),
        ..Default::default()
    };
    let mut handle_hsp = session.register_profile(profile).await?;
    info!("{} 🎧 Headset Profile (HSP): registered", NAME);

    info!("{} ⏳ Waiting for phone to connect via bluetooth...", NAME);

    // handling connection to headset profile in own task
    let task_hsp: JoinHandle<Result<ProfileHandle>> = tokio::spawn(async move {
        let req = handle_hsp
            .next()
            .await
            .expect("received no connect request");
        info!(
            "{} 🎧 Headset Profile (HSP): connect from: <b>{}</>",
            NAME,
            req.device()
        );
        req.accept()?;

        Ok(handle_hsp)
    });

    let req = handle_aa.next().await.expect("received no connect request");
    info!(
        "{} 📱 AA Wireless Profile: connect from: <b>{}</>",
        NAME,
        req.device()
    );
    let stream = req.accept()?;

    // generate structure with adapter and handlers for graceful shutdown later
    let state = BluetoothState {
        adapter,
        handle_ble,
        handle_aa,
        handle_hsp: task_hsp,
        handle_agent,
    };

    Ok((state, stream))
}

async fn send_message(stream: &mut Stream, id: MessageId, message: impl Message) -> Result<usize> {
    let mut packet: Vec<u8> = vec![];
    let mut data = message.write_to_bytes()?;

    // create header: 2 bytes message length + 2 bytes MessageID
    packet.write_u16(data.len() as u16).await?;
    packet.write_u16(id.clone() as u16).await?;

    // append data and send
    packet.append(&mut data);

    info!("{} 📨 Sending <yellow>{:?}</> frame to phone...", NAME, id);

    Ok(stream.write(&packet).await?)
}

async fn read_message(stream: &mut Stream, id: MessageId) -> Result<usize> {
    let mut buf = vec![0; HEADER_LEN];
    let n = stream.read_exact(&mut buf).await?;
    debug!("received {} bytes: {:X?}", n, buf);

    let len: usize = u16::from_be_bytes(buf[0..=1].try_into()?).into();
    let message_id = u16::from_be_bytes(buf[2..=3].try_into()?);
    debug!("MessageID = {}, len = {}", message_id, len);

    if message_id != id.clone() as u16 {
        return Err(format!(
            "Received data has invalid MessageID: got: {:?}, expected: {:?}",
            message_id, id,
        )
        .into());
    }
    info!("{} 📨 Received <yellow>{:?}</> frame from phone", NAME, id);

    // read and discard the remaining bytes
    if len > 0 {
        let mut buf = vec![0; len];
        stream.read_exact(&mut buf).await?;
    }

    Ok(HEADER_LEN + len)
}

pub async fn bluetooth_stop(state: BluetoothState) -> Result<()> {
    info!("{} 📣 Removing BLE advertisement", NAME);
    drop(state.handle_ble);
    info!("{} 🥷 Unregistering default agent", NAME);
    drop(state.handle_agent);
    info!("{} 📱 Removing AA profile", NAME);
    drop(state.handle_aa);
    info!("{} 🎧 Removing HSP profile", NAME);
    drop(state.handle_hsp.await??);

    state.adapter.set_powered(false).await?;
    info!("{} 💤 Bluetooth adapter powered off", NAME);

    Ok(())
}

pub async fn bluetooth_setup_connection() -> Result<BluetoothState> {
    use WifiInfoResponse::WifiInfoResponse;
    use WifiStartRequest::WifiStartRequest;

    let (state, mut stream) = power_up_and_wait_for_connection().await?;

    info!("{} 📲 Sending parameters via bluetooth to phone...", NAME);
    let mut start_req = WifiStartRequest::new();
    start_req.set_ip_address(String::from(WLAN_IP_ADDR));
    start_req.set_port(TCP_SERVER_PORT);
    send_message(&mut stream, MessageId::WifiStartRequest, start_req).await?;
    read_message(&mut stream, MessageId::WifiInfoRequest).await?;

    let mut info = WifiInfoResponse::new();
    info.set_ssid(String::from(WLAN_SSID));
    info.set_key(String::from(WLAN_WPA_KEY));
    let bssid = mac_address::mac_address_by_name(WLAN_IFACE)
        .unwrap()
        .unwrap()
        .to_string();
    info.set_bssid(bssid);
    info.set_security_mode(SecurityMode::WPA2_PERSONAL);
    info.set_access_point_type(AccessPointType::DYNAMIC);
    send_message(&mut stream, MessageId::WifiInfoResponse, info).await?;
    read_message(&mut stream, MessageId::WifiStartResponse).await?;
    read_message(&mut stream, MessageId::WifiConnectStatus).await?;
    let _ = stream.shutdown().await?;

    info!("{} 🚀 Bluetooth launch sequence completed", NAME);

    Ok(state)
}
