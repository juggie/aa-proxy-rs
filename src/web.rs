use crate::config::AppConfig;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, Response, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use chrono::Local;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::fs;

const TEMPLATE: &str = include_str!("../static/index.html");
const PICO_CSS: &str = include_str!("../static/pico.min.css");

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Mutex<AppConfig>>,
    pub config_file: Arc<PathBuf>,
}

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/config", get(get_config).post(set_config))
        .route("/download", get(download_handler))
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

fn generate_filename() -> String {
    let now = Local::now();
    now.format("%Y%m%d%H%M%S_aa-proxy-rs.log").to_string()
}

async fn download_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let file_path = state.config.lock().unwrap().logfile.clone();
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

async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.lock().unwrap().clone();
    Json(cfg)
}

async fn set_config(
    State(state): State<Arc<AppState>>,
    Json(new_cfg): Json<AppConfig>,
) -> impl IntoResponse {
    {
        let mut cfg = state.config.lock().unwrap();
        *cfg = new_cfg.clone();
        cfg.save((&state.config_file).to_path_buf());
    }
    Json(new_cfg)
}
