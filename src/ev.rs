use simplelog::*;
use std::path::PathBuf;
use tokio::fs;
use tokio::sync::mpsc::Sender;

// protobuf stuff:
include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
use crate::ev::protos::*;
use crate::ev::SensorMessageId::*;
use crate::mitm::Packet;
use crate::mitm::{ENCRYPTED, FRAME_TYPE_FIRST, FRAME_TYPE_LAST};
use protobuf::Message;

use serde::Deserialize;

pub static FORD_EV_MODEL: &[u8] = include_bytes!("protos/ford_ev_model.bin");
pub const EV_MODEL_FILE: &str = "/etc/aa-proxy-rs/ev_model.bin";

// module name for logging engine
const NAME: &str = "<i><bright-black> ev: </>";

// Just a generic Result type to ease error handling for us. Errors in multithreaded
// async contexts needs some extra restrictions
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Deserialize)]
pub struct BatteryData {
    pub battery_level: f32,
}

fn scale_percent_to_value(percent: f32, max_value: u64) -> u64 {
    let scaled = (percent as f64 / 100.0) * max_value as f64;
    scaled.round() as u64
}

/// EV sensor batch data send
pub async fn send_ev_data(
    tx: Sender<Packet>,
    level: f32,
    sensor_ch: u8,
    ev_battery_capacity: u64,
    ev_factor: f32,
) -> Result<()> {
    // obtain binary model data
    let model_path: PathBuf = PathBuf::from(EV_MODEL_FILE);
    let data = if fs::try_exists(&model_path).await? {
        // reading model from file
        fs::read(&model_path).await?
    } else {
        // default initial sample Ford data
        FORD_EV_MODEL.to_vec()
    };

    // parse
    let mut msg = SensorBatch::parse_from_bytes(&data)?;

    // apply our changes
    msg.energy_model_control[0].u1.as_mut().unwrap().u6 = 1.0;
    msg.energy_model_control[0]
        .u1
        .as_mut()
        .unwrap()
        .u2
        .as_mut()
        .unwrap()
        .u1 = 1;
    msg.energy_model_control[0]
        .u2
        .as_mut()
        .unwrap()
        .u3
        .as_mut()
        .unwrap()
        .u1 = ev_factor;

    // kwh in battery?
    msg.energy_model_control[0]
        .u1
        .as_mut()
        .unwrap()
        .u3
        .as_mut()
        .unwrap()
        .u1 = scale_percent_to_value(level, ev_battery_capacity);
    // max battery kwh?
    msg.energy_model_control[0]
        .u1
        .as_mut()
        .unwrap()
        .u4
        .as_mut()
        .unwrap()
        .u1 = ev_battery_capacity;

    // creating back binary data for sending
    let mut payload: Vec<u8> = msg.write_to_bytes()?;
    // add SENSOR header
    payload.insert(0, ((SENSOR_MESSAGE_BATCH as u16) >> 8) as u8);
    payload.insert(1, ((SENSOR_MESSAGE_BATCH as u16) & 0xff) as u8);

    let pkt = Packet {
        channel: sensor_ch,
        flags: ENCRYPTED | FRAME_TYPE_FIRST | FRAME_TYPE_LAST,
        final_length: None,
        payload: payload,
    };
    tx.send(pkt).await?;
    info!("{} injecting ENERGY_MODEL_DATA packet...", NAME);

    Ok(())
}
