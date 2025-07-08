use crate::config::AppConfig;
use axum::{
    extract::State,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
