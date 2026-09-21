//! Loopback-only HTTP API and static Web UI.

use crate::ipc::{commands, AppState};
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, warn};

pub async fn serve(address: SocketAddr, state: AppState, open_browser: bool) -> anyhow::Result<()> {
    let frontend = frontend_dir();
    if !frontend.join("index.html").is_file() {
        anyhow::bail!(
            "Web UI not found at {} (run `npm --prefix frontend run build`)",
            frontend.display()
        );
    }
    let files =
        ServeDir::new(&frontend).not_found_service(ServeFile::new(frontend.join("index.html")));
    let app = Router::new()
        .route("/api/events", get(events))
        .route("/api/invoke/{command}", post(invoke))
        .route("/api/install-bot", post(install_bot))
        .fallback_service(files)
        .layer(DefaultBodyLimit::max(512 * 1024 * 1024))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address).await?;
    let address = listener.local_addr()?;
    let url = format!("http://{address}");
    info!("Web UI listening on {url}");
    println!("Akagi Web UI: {url}");
    if open_browser {
        if let Err(error) = opener::open_browser(&url) {
            warn!("could not open the Web UI: {error}");
        }
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn install_bot(State(state): State<AppState>, form: Multipart) -> axum::response::Response {
    answer(receive_bot_upload(form, &state).await)
}

async fn receive_bot_upload(
    mut form: Multipart,
    state: &AppState,
) -> Result<crate::schema::BotInfo, String> {
    let mut name = None;
    let mut archive = None;
    while let Some(mut field) = form
        .next_field()
        .await
        .map_err(|error| format!("read upload: {error}"))?
    {
        match field.name() {
            Some("name") => {
                let value = field
                    .text()
                    .await
                    .map_err(|error| format!("read bot name: {error}"))?;
                name = (!value.is_empty()).then_some(value);
            }
            Some("archive") => {
                let file_name = field
                    .file_name()
                    .and_then(uploaded_file_name)
                    .unwrap_or("bot.zip");
                let dir = tempfile::tempdir().map_err(|error| format!("create upload: {error}"))?;
                let path = dir.path().join(file_name);
                let mut output = tokio::fs::File::create(&path)
                    .await
                    .map_err(|error| format!("create upload: {error}"))?;
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|error| format!("read upload: {error}"))?
                {
                    output
                        .write_all(&chunk)
                        .await
                        .map_err(|error| format!("write upload: {error}"))?;
                }
                output
                    .flush()
                    .await
                    .map_err(|error| format!("write upload: {error}"))?;
                archive = Some((dir, path));
            }
            _ => {}
        }
    }
    let archive = archive.ok_or_else(|| "missing zip archive".to_string())?;
    commands::install_bot_from_zip(archive.1.to_string_lossy().into_owned(), name, state).await
}

fn uploaded_file_name(file_name: &str) -> Option<&str> {
    file_name.rsplit(['/', '\\']).next().filter(|name| {
        !name.is_empty()
            && FsPath::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    })
}

fn frontend_dir() -> PathBuf {
    let adjacent = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("frontend")));
    if let Some(path) = adjacent.filter(|p| p.join("index.html").is_file()) {
        return path;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend/dist")
}

fn field<T: DeserializeOwned>(args: &Value, camel: &str, snake: &str) -> Result<T, String> {
    let derived = snake_to_camel(snake);
    args.get(camel)
        .or_else(|| args.get(snake))
        .or_else(|| args.get(&derived))
        .cloned()
        .ok_or_else(|| format!("missing argument {camel}"))
        .and_then(|v| serde_json::from_value(v).map_err(|e| format!("invalid {camel}: {e}")))
}

fn optional<T: DeserializeOwned>(
    args: &Value,
    camel: &str,
    snake: &str,
) -> Result<Option<T>, String> {
    let derived = snake_to_camel(snake);
    match args
        .get(camel)
        .or_else(|| args.get(snake))
        .or_else(|| args.get(&derived))
    {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| format!("invalid {camel}: {e}")),
    }
}

fn snake_to_camel(value: &str) -> String {
    let mut upper = false;
    value
        .chars()
        .filter_map(|ch| {
            if ch == '_' {
                upper = true;
                None
            } else if upper {
                upper = false;
                Some(ch.to_ascii_uppercase())
            } else {
                Some(ch)
            }
        })
        .collect()
}

fn answer<T: Serialize, E: Serialize + std::fmt::Display>(
    result: Result<T, E>,
) -> axum::response::Response {
    match result {
        Ok(value) => Json(json!({ "ok": true, "value": value })).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": error, "message": error.to_string() })),
        )
            .into_response(),
    }
}

macro_rules! args {
    ($body:ident, $($name:ident : $ty:ty),* $(,)?) => {
        $(let $name: $ty = match field(&$body, stringify!($name), stringify!($name)) {
            Ok(value) => value,
            Err(error) => return answer::<(), _>(Err(error)),
        };)*
    };
}

macro_rules! opt_args {
    ($body:ident, $($name:ident : $ty:ty),* $(,)?) => {
        $(let $name: Option<$ty> = match optional(&$body, stringify!($name), stringify!($name)) {
            Ok(value) => value,
            Err(error) => return answer::<(), _>(Err(error)),
        };)*
    };
}

async fn invoke(
    State(state): State<AppState>,
    Path(command): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    match command.as_str() {
        "get_config" => answer(commands::get_config(&state).await),
        "update_config" => {
            let new_config = field(&body, "newConfig", "new_config");
            match new_config {
                Ok(v) => answer(commands::update_config(v, &state).await),
                Err(e) => answer::<(), _>(Err(e)),
            }
        }
        "update_config_preserving_controls" => {
            let new_config = field(&body, "newConfig", "new_config");
            match new_config {
                Ok(v) => answer(commands::update_config_preserving_controls(v, &state).await),
                Err(e) => answer::<(), _>(Err(e)),
            }
        }
        "update_autoplay_controls" => {
            opt_args!(body, enabled: bool, remaining_games: u32);
            answer(commands::update_autoplay_controls(enabled, remaining_games, &state).await)
        }
        "list_bots" => answer(commands::list_bots(&state).await),
        "get_bot_settings" => {
            args!(body, name: String);
            answer(commands::get_bot_settings(name, &state).await)
        }
        "update_bot_settings" => {
            args!(body, name: String, values: std::collections::BTreeMap<String, Value>);
            answer(commands::update_bot_settings(name, values, &state).await)
        }
        "set_active_bot" => {
            args!(body, mode: String, name: String);
            answer(commands::set_active_bot(mode, name, &state).await)
        }
        "install_bot_from_github" => {
            args!(body, repo: String);
            opt_args!(body, asset_glob: String, name: String);
            answer(commands::install_bot_from_github(repo, asset_glob, name, &state).await)
        }
        "install_bot_from_zip" => {
            let zip_path = match field(&body, "zipPath", "zip_path") {
                Ok(v) => v,
                Err(e) => return answer::<(), _>(Err(e)),
            };
            opt_args!(body, name: String);
            answer(commands::install_bot_from_zip(zip_path, name, &state).await)
        }
        "update_bot_from_manifest" => {
            args!(body, name: String);
            answer(commands::update_bot_from_manifest(name, &state).await)
        }
        "sync_bot_deps" => {
            args!(body, name: String, force: bool);
            answer(commands::sync_bot_deps(name, force, &state).await)
        }
        "start_capture" => answer(commands::start_capture(&state).await),
        "restart_capture" => answer(commands::restart_capture(&state).await),
        "stop_capture" => answer(commands::stop_capture(&state).await),
        "detect_system_chrome" => answer(commands::detect_system_chrome().await),
        "list_cft_installed" => answer(commands::list_cft_installed().await),
        "download_chrome_for_testing" => {
            opt_args!(body, channel: String);
            answer(commands::download_chrome_for_testing(channel, &state).await)
        }
        "remove_chrome_for_testing" => {
            args!(body, version: String);
            answer(commands::remove_chrome_for_testing(version, &state).await)
        }
        "get_status" => answer(commands::get_status(&state).await),
        "get_capture_status" => answer(commands::get_capture_status(&state).await),
        "get_log_dir" => answer(commands::get_log_dir(&state).await),
        "open_log_folder" => {
            opt_args!(body, session: String);
            answer(commands::open_log_folder(session, &state).await)
        }
        "open_external_url" => {
            args!(body, url: String);
            answer(commands::open_external_url(url).await)
        }
        "list_log_sessions" => answer(commands::list_log_sessions(&state).await),
        "read_log_session" => {
            args!(body, req: crate::schema::ReadLogRequest);
            answer(commands::read_log_session(req, &state).await)
        }
        "read_inspector" => {
            args!(body, req: crate::schema::ReadInspectorRequest);
            answer(commands::read_inspector(req, &state).await)
        }
        "get_analysis" => answer(commands::get_analysis(&state).await),
        "get_game_snapshot" => answer(commands::get_game_snapshot(&state).await),
        "compute_bot_hora_score" => {
            let actor = match field(&body, "actor", "actor") {
                Ok(v) => v,
                Err(e) => return answer::<(), _>(Err(e)),
            };
            let is_tsumo = match field(&body, "isTsumo", "is_tsumo") {
                Ok(v) => v,
                Err(e) => return answer::<(), _>(Err(e)),
            };
            answer(commands::compute_bot_hora_score(actor, is_tsumo, &state).await)
        }
        "get_mahgen_view" => answer(commands::get_mahgen_view(&state).await),
        "delete_bot" => {
            args!(body, name: String);
            answer(commands::delete_bot(name, &state).await)
        }
        "list_game_history" => {
            opt_args!(body, filter: crate::schema::HistoryFilter, limit: u32, offset: u32);
            answer(commands::list_game_history(filter, limit, offset, &state).await)
        }
        "get_game_history_record" => {
            args!(body, id: String);
            answer(commands::get_game_history_record(id, &state).await)
        }
        "get_game_history_events" => {
            args!(body, id: String);
            answer(commands::get_game_history_events(id, &state).await)
        }
        "delete_game_history_entry" => {
            args!(body, id: String);
            answer(commands::delete_game_history_entry(id, &state).await)
        }
        "check_for_update" => answer(commands::check_for_update(&state).await),
        "apply_update" => answer(commands::apply_update(&state).await),
        "native_api_redeem" => {
            args!(body, base_url: String, code: String);
            opt_args!(body, proxy: String, email: String, renew_key: String);
            answer(commands::native_api_redeem(base_url, proxy, code, email, renew_key).await)
        }
        "native_api_key_status" => {
            args!(body, base_url: String, key: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_key_status(base_url, proxy, key).await)
        }
        "native_api_models" => {
            args!(body, base_url: String, key: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_models(base_url, proxy, key).await)
        }
        "native_api_health" => {
            args!(body, base_url: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_health(base_url, proxy).await)
        }
        "native_api_review_history_game" => {
            args!(body, base_url: String, key: String, id: String);
            opt_args!(body, proxy: String, model: String);
            answer(
                commands::native_api_review_history_game(base_url, proxy, key, id, model, &state)
                    .await,
            )
        }
        "native_api_review_status" => {
            args!(body, base_url: String, key: String, review_id: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_review_status(base_url, proxy, key, review_id).await)
        }
        "native_api_review_share" => {
            args!(body, base_url: String, key: String, review_id: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_review_share(base_url, proxy, key, review_id).await)
        }
        "native_api_list_shares" => {
            args!(body, base_url: String, key: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_list_shares(base_url, proxy, key).await)
        }
        "native_api_revoke_share" => {
            args!(body, base_url: String, key: String, share_id: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_revoke_share(base_url, proxy, key, share_id).await)
        }
        "native_api_create_order" => {
            args!(body, base_url: String, product: String, redeem: bool);
            opt_args!(body, proxy: String);
            answer(commands::native_api_create_order(base_url, proxy, product, redeem).await)
        }
        "native_api_order_result" => {
            args!(body, base_url: String, order_id: String, claim: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_order_result(base_url, proxy, order_id, claim).await)
        }
        "native_api_create_subscription" => {
            args!(body, base_url: String, product: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_create_subscription(base_url, proxy, product).await)
        }
        "native_api_subscription_result" => {
            args!(body, base_url: String, subscription_id: String, claim: String);
            opt_args!(body, proxy: String);
            answer(
                commands::native_api_subscription_result(base_url, proxy, subscription_id, claim)
                    .await,
            )
        }
        "native_api_create_checkout" => {
            args!(body, base_url: String, product: String, redeem: bool);
            opt_args!(body, proxy: String);
            answer(commands::native_api_create_checkout(base_url, proxy, product, redeem).await)
        }
        "native_api_checkout_result" => {
            args!(body, base_url: String, checkout_id: String, claim: String);
            opt_args!(body, proxy: String);
            answer(commands::native_api_checkout_result(base_url, proxy, checkout_id, claim).await)
        }
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "message": format!("unknown command {command}") })),
        )
            .into_response(),
    }
}

fn event<T: Serialize>(name: &str, payload: &T) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event(name)
        .json_data(payload)
        .expect("serializable event"))
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let mut mjai = state.mjai_bus.subscribe();
    let mut responses = state.bot_response_bus.subscribe();
    let mut bots = state.bot_status_bus.subscribe();
    let mut capture = state.capture_status_bus.subscribe();
    let mut notify = state.notify_bus.subscribe();
    let mut config = state.config_bus.subscribe();
    let mut analysis = state.analysis_bus.subscribe();
    let mut history = state.history_bus.subscribe();
    let mut logs = state.log_session.subscribe();
    let mut inspector = state.log_session.subscribe_inspector();
    let stream = async_stream::stream! {
        loop {
            let next = tokio::select! {
                Ok(v) = mjai.recv() => event("mjai-event", &v),
                Ok(v) = responses.recv() => event("bot-response", &v),
                Ok(v) = bots.recv() => event("bot-status", &v),
                Ok(v) = capture.recv() => event("capture-status", &v),
                Ok(v) = notify.recv() => event("notify", &v),
                Ok(v) = config.recv() => event("config-updated", &v),
                Ok(v) = analysis.recv() => event("analysis-result", &v),
                Ok(v) = history.recv() => event("history-recorded", &v),
                Ok(v) = logs.recv() => event("log-entry", &v),
                Ok(v) = inspector.recv() => event("inspector-entry", &v),
                else => break,
            };
            yield next;
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[cfg(test)]
mod tests {
    use super::uploaded_file_name;

    #[test]
    fn uploaded_file_name_keeps_only_the_zip_basename() {
        assert_eq!(uploaded_file_name("mortal-v4.zip"), Some("mortal-v4.zip"));
        assert_eq!(
            uploaded_file_name(r"C:\fakepath\mortal.zip"),
            Some("mortal.zip")
        );
    }
}
