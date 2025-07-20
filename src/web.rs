use crate::config::AppConfig;
use crate::config::SharedConfig;
use crate::ev::send_ev_data;
use crate::ev::BatteryData;
use crate::ev::EV_MODEL_FILE;
use crate::mitm::Packet;
use axum::{
    body::Body,
    extract::{Query, RawBody, State},
    http::{header, HeaderMap, Response, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use chrono::Local;
use hyper::body::to_bytes;
use simplelog::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::Sender;

const TEMPLATE: &str = include_str!("../static/index.html");
const PICO_CSS: &str = include_str!("../static/pico.min.css");

// module name for logging engine
const NAME: &str = "<i><bright-black> web: </>";

#[derive(Clone)]
pub struct AppState {
    pub config: SharedConfig,
    pub config_file: Arc<PathBuf>,
    pub tx: Option<Sender<Packet>>,
    pub sensor_channel: Arc<Option<u8>>,
}

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/config", get(get_config).post(set_config))
        .route("/download", get(download_handler))
        .route("/restart", get(restart_handler))
        .route("/upload-hex-model", post(upload_hex_model_handler))
        .route("/battery", post(battery_handler))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    let html = TEMPLATE
        .replace("{BUILD_DATE}", env!("BUILD_DATE"))
        .replace("{GIT_DATE}", env!("GIT_DATE"))
        .replace("{GIT_HASH}", env!("GIT_HASH"))
        .replace("{PICO_CSS}", PICO_CSS);
    Html(html)
}

pub async fn battery_handler(
    State(state): State<Arc<AppState>>,
    Json(data): Json<BatteryData>,
) -> impl IntoResponse {
    if data.battery_level < 0.0 || data.battery_level > 100.0 {
        let msg = format!(
            "battery_level out of range: {} (expected 0.0–100.0)",
            data.battery_level
        );
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    info!("{} Received battery level: {}", NAME, data.battery_level);

    if let Some(ch) = *state.sensor_channel {
        if let Some(tx) = &state.tx {
            if let Err(e) = send_ev_data(tx.clone(), data.battery_level, ch, 0, 0.0).await {
                error!("{} EV model error: {}", NAME, e);
            }
        }
    } else {
        warn!("{} Not sending packet because no sensor channel yet", NAME);
    }

    (StatusCode::OK, "OK").into_response()
}

fn generate_filename() -> String {
    let now = Local::now();
    now.format("%Y%m%d%H%M%S_aa-proxy-rs.log").to_string()
}

async fn restart_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.config.write().await.restart_requested = true;

    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from("Restart has been requested"))
        .unwrap()
}

async fn download_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let file_path = state.config.read().await.logfile.clone();
    // if we have filename parameter, use it; default otherwise
    let filename = params
        .get("filename")
        .cloned()
        .unwrap_or_else(generate_filename);

    match fs::read(file_path).await {
        Ok(content) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", filename),
            )
            .body(Body::from(content))
            .unwrap(),
        Err(_) => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Cannot access log file"))
            .unwrap(),
    }
}

async fn upload_hex_model_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RawBody(body): RawBody,
) -> impl IntoResponse {
    // read body as bytes
    let body_bytes = match to_bytes(body).await {
        Ok(bytes) => bytes,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Unable to read body: {}", err),
            )
        }
    };

    // convert to UTF-8 string
    let hex_str = match std::str::from_utf8(&body_bytes) {
        Ok(s) => s.trim(), // remove whitespaces
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Unable to parse body to UTF-8: {}", err),
            )
        }
    };

    // decode into Vec<u8>
    let binary_data = match hex::decode(hex_str) {
        Ok(data) => data,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid hex data: {}", err),
            )
        }
    };

    // save to model file
    let path: PathBuf = PathBuf::from(EV_MODEL_FILE);
    match fs::File::create(&path).await {
        Ok(mut file) => {
            if let Err(err) = file.write_all(&binary_data).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Error saving model file: {}", err),
                );
            }
            (
                StatusCode::OK,
                format!("File saved correctly as {:?}", path),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("File create error: {}", err),
        ),
    }
}

async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.read().await.clone();
    Json(cfg)
}

async fn set_config(
    State(state): State<Arc<AppState>>,
    Json(new_cfg): Json<AppConfig>,
) -> impl IntoResponse {
    {
        let mut cfg = state.config.write().await;
        *cfg = new_cfg.clone();
        cfg.save((&state.config_file).to_path_buf());
    }
    Json(new_cfg)
}
