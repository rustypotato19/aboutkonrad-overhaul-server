use axum::{
    extract::State,
    http::{ header, Method, StatusCode },
    response::IntoResponse,
    routing::{ get, post },
    Json,
    Router,
};
use serde::{ Deserialize, Serialize };
use std::{
    collections::{ HashMap, VecDeque },
    net::SocketAddr,
    path::PathBuf,
    sync::{ Arc, Mutex },
};
use tokio::fs;
use tower_http::{ cors::{ Any, CorsLayer }, services::ServeDir };

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Playback {
    track: Option<String>,
    artist: Option<String>,
    status: String,
    timestamp: Option<u64>,
    total_duration: Option<u64>,
    link: Option<String>,
    elapsed: u64,
}

#[derive(Clone)]
struct AppState {
    playback_stack: Arc<Mutex<VecDeque<Playback>>>, // stack of last N entries
    locales_path: PathBuf,
}

const STACK_SIZE: usize = 10;

#[tokio::main]
async fn main() {
    let initial_playback = Playback {
        track: None,
        artist: None,
        status: "paused".to_string(),
        timestamp: None,
        total_duration: None,
        link: None,
        elapsed: 0,
    };

    let state = AppState {
        playback_stack: Arc::new(Mutex::new(VecDeque::from([initial_playback]))),
        locales_path: PathBuf::from("./locales"),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE]);

    let app = Router::new()
        .nest_service("/locales", ServeDir::new("./locales"))
        .route("/api/locales", get(list_locales))
        .route("/ytmusic/update", post(update_playback))
        .route("/ytmusic/current", get(get_playback))
        .with_state(state)
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3001));
    println!("🚀 Server running at http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service()).await.unwrap();
}

// === Handlers ===

async fn list_locales(State(state): State<AppState>) -> impl IntoResponse {
    match fs::read_dir(&state.locales_path).await {
        Ok(mut entries) => {
            let mut locales = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Ok(file_type) = entry.file_type().await {
                    if file_type.is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            locales.push(name.to_string());
                        }
                    }
                }
            }
            Json(HashMap::from([("locales".to_string(), locales)])).into_response()
        }
        Err(_) =>
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HashMap::from([("error".to_string(), "Failed to read locales".to_string())])),
            ).into_response(),
    }
}

#[derive(Deserialize/* , Debug */)]
#[serde(rename_all = "camelCase")]
struct UpdatePayload {
    track: Option<String>,
    artist: Option<String>,
    status: Option<String>,
    timestamp: Option<u64>,
    total_duration: Option<u64>,
    link: Option<String>,
    elapsed: Option<u64>,
}

async fn update_playback(
    State(state): State<AppState>,
    Json(payload): Json<UpdatePayload>
) -> impl IntoResponse {
    // Log the received data
    //println!("Received YT playback update: {:#?}", payload);

    let mut stack = state.playback_stack.lock().unwrap();

    // Construct new playback entry
    let new_status = payload.status.clone().unwrap_or_else(|| "paused".to_string());
    let new_playback = Playback {
        track: payload.track.clone(),
        artist: payload.artist.clone(),
        status: new_status.clone(),
        timestamp: payload.timestamp,
        total_duration: payload.total_duration,
        link: payload.link.clone(),
        elapsed: payload.elapsed.unwrap_or(0),
    };

    // Push to stack, maintain size limit
    stack.push_back(new_playback);
    if stack.len() > STACK_SIZE {
        stack.pop_front();
    }

    StatusCode::OK
}

async fn get_playback(State(state): State<AppState>) -> impl IntoResponse {
    let stack = state.playback_stack.lock().unwrap();
    if stack.is_empty() {
        return Json(Playback {
            track: None,
            artist: None,
            status: "paused".to_string(),
            timestamp: None,
            total_duration: None,
            link: None,
            elapsed: 0,
        });
    }

    let mut latest = stack.back().unwrap().clone();
    let now = chrono::Utc::now().timestamp_millis() as u64;

    if let Some(ts) = latest.timestamp {
        // If last update is older than 5 seconds beyond elapsed, consider player gone
        if now > ts + latest.elapsed * 1000 + 5000 {
            latest.status = "null".to_string();
            latest.track = None;
            latest.artist = None;
            latest.elapsed = 0;
            latest.timestamp = None;
            latest.total_duration = None;
            latest.link = None;
        } else if latest.status == "playing" {
            // Only increment elapsed if data is recent
            latest.elapsed = (now - ts) / 1000;
        }
    }

    // If all entries are paused, set status null
    let all_paused = stack.iter().all(|p| p.status == "paused");
    if all_paused {
        latest.status = "null".to_string();
    }

    Json(latest)
}
