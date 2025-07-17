use simplelog::*;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;

// protobuf stuff:
include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
use crate::ev::protos::*;
use crate::ev::SensorMessageId::*;
use crate::mitm::Packet;
use crate::mitm::{ENCRYPTED, FRAME_TYPE_FIRST, FRAME_TYPE_LAST};
use protobuf::Message;

use serde::Deserialize;
use warp::Filter;

pub static FORD_EV_MODEL: &[u8] = include_bytes!("protos/ford_ev_model.bin");
pub const EV_MODEL_FILE: &str = "/etc/aa-proxy-rs/ev_model.bin";

// module name for logging engine
const NAME: &str = "<i><bright-black> ev: </>";

// Just a generic Result type to ease error handling for us. Errors in multithreaded
// async contexts needs some extra restrictions
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Deserialize)]
pub struct BatteryData {
    battery_level: f32,
}

// reset server context
#[derive(Clone)]
pub struct RestContext {
    pub sensor_channel: Option<u8>,
    pub ev_battery_capacity: u64,
    pub ev_factor: f32,
}

pub async fn rest_server(tx: Sender<Packet>, ctx: Arc<Mutex<RestContext>>) -> Result<()> {
    let battery_route = warp::post()
        .and(warp::path("battery"))
        .and(warp::body::json())
        .and(warp::any().map({
            let ctx = ctx.clone();
            move || ctx.clone()
        }))
        .and_then(move |data: BatteryData, ctx: Arc<Mutex<RestContext>>| {
            let tx = tx.clone();
            async move {
                if data.battery_level < 0.0 || data.battery_level > 100.0 {
                    let msg = format!(
                        "battery_level out of range: {} (expected 0.0–100.0)",
                        data.battery_level
                    );
                    return Ok::<_, warp::Rejection>(warp::reply::with_status(
                        msg,
                        warp::http::StatusCode::BAD_REQUEST,
                    ));
                }

                info!("{} Received battery level: {}", NAME, data.battery_level);
                let rest_ctx = ctx.lock().await;
                if let Some(ch) = rest_ctx.sensor_channel {
                    let _ = send_ev_data(
                        tx,
                        data.battery_level,
                        ch,
                        rest_ctx.ev_battery_capacity,
                        rest_ctx.ev_factor,
                    )
                    .await;
                } else {
                    warn!("{} Not sending packet because no sensor channel yet", NAME);
                }

                Ok(warp::reply::with_status(
                    "OK".into(),
                    warp::http::StatusCode::OK,
                ))
            }
        });

    info!("{} Server running on http://127.0.0.1:3030", NAME);

    warp::serve(battery_route).run(([127, 0, 0, 1], 3030)).await;

    Ok(())
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
    // parse initial sample Ford data
    let mut msg = SensorBatch::parse_from_bytes(FORD_EV_MODEL)?;

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
