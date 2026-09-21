pub mod analysis;
pub mod autoplay;
pub mod bot;
pub mod bridge;
pub mod capture;
pub mod cli;
pub mod config;
pub mod event_bus;
pub mod game_state;
pub mod github;
pub mod history;
pub mod inspector;
pub mod ipc;
pub mod logger;
pub mod proxy;
pub mod schema;
pub mod updater;
pub mod util;
pub mod web;

use clap::Parser;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing::{error, info, warn};

pub fn run() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build Tokio runtime");
    if let Err(error) = runtime.block_on(run_async()) {
        eprintln!("Akagi failed: {error:#}");
    }
}

async fn run_async() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    let (cfg, config_path) = config::load_config(args.config.as_deref());
    let log_dir = util::resolve_dir(&cfg.logging.dir);
    let targets = [
        logger::LogTarget::new("proxy", "akagi::proxy"),
        logger::LogTarget::new("bot", "akagi::bot"),
    ];
    let session = Arc::new(logger::init(
        &log_dir,
        &cfg.logging.level,
        &cfg.logging.all_level,
        &targets,
    )?);
    info!("Config loaded: {cfg:?}");

    let mjai_bus = event_bus::mjai_bus();
    let bot_response_bus = event_bus::bot_response_bus();
    let bot_status_bus = event_bus::bot_status_bus();
    let capture_status_bus = event_bus::capture_status_bus();
    let notify_bus = event_bus::notify_bus();
    let analysis_bus = event_bus::analysis_bus();
    let post_tracker_bus = event_bus::post_tracker_bus();
    let history_bus = event_bus::history_bus();

    let history_root = util::resolve_dir(Path::new("./history"));
    let history_store = Arc::new(history::HistoryStore::new(history_root)?);
    let game_tracker = game_state::tracker::new_handle();
    let analysis_cache = Arc::new(tokio::sync::RwLock::new(None));
    let python_runtime = bot::PythonRuntime::locate().ok();
    if python_runtime.is_none() {
        warn!("no Python 3.12 + uv runtime found; external bot install/sync is unavailable");
    }
    let history_platform =
        history::recorder::shared_platform(schema::Platform::from(cfg.platform.kind));

    let state = ipc::AppState::new(
        cfg.clone(),
        config_path,
        session.clone(),
        mjai_bus.clone(),
        post_tracker_bus.clone(),
        bot_response_bus.clone(),
        bot_status_bus.clone(),
        capture_status_bus,
        notify_bus,
        analysis_bus.clone(),
        history_bus.clone(),
        game_tracker.clone(),
        analysis_cache.clone(),
        history_store.clone(),
        history_platform.clone(),
        python_runtime,
    );
    ipc::install(state.clone());

    tokio::spawn(autoplay::majsoul_rematch::watch(state.clone()));

    tokio::spawn(game_state::tracker::drive_loop(
        game_tracker.clone(),
        mjai_bus.subscribe(),
        Some(post_tracker_bus.clone()),
    ));
    tokio::spawn(analysis::runner::drive_loop(
        post_tracker_bus.subscribe(),
        game_tracker,
        analysis_bus,
        analysis_cache,
    ));
    tokio::spawn(history::recorder::drive_loop(
        history_store,
        history_bus,
        history_platform,
        mjai_bus.subscribe(),
    ));

    if cfg.bot.enabled {
        state.bot_manager_started.store(true, Ordering::SeqCst);
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = bot::run_bot_manager(
                st.config.clone(),
                st.post_tracker_bus.clone(),
                st.bot_response_bus.clone(),
                st.bot_status_bus.clone(),
                st.notify_bus.clone(),
                st.log_session.inspector(),
                st.runtime.clone(),
                st.syncs_in_flight.clone(),
            )
            .await
            {
                error!("Bot manager failed: {e:#}");
            }
        });
    }

    if cfg.autoplay.enabled {
        state.autoplay_manager_started.store(true, Ordering::SeqCst);
        let st = state.clone();
        tokio::spawn(async move {
            let config_dir = st
                .config_path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            if let Err(e) = autoplay::run_autoplay_manager(
                st.config.clone(),
                st.autoplay_context.clone(),
                st.game_tracker.clone(),
                st.mjai_bus.clone(),
                st.bot_response_bus.clone(),
                st.notify_bus.clone(),
                config_dir,
            )
            .await
            {
                error!("Autoplay manager failed: {e:#}");
            }
        });
    }

    {
        let mut rx = mjai_bus.subscribe();
        let inspector = session.inspector();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                inspector.record(schema::InspectorEntry::MjaiEvent {
                    ts_ms: chrono::Local::now().timestamp_millis(),
                    event,
                });
            }
        });
    }

    if cfg.proxy.enabled {
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = ipc::capture_supervisor::spawn_capture_supervisor(st).await {
                error!("Capture supervisor failed: {e:#}");
            }
        });
    }

    let address = std::net::SocketAddr::from(([127, 0, 0, 1], args.port));
    web::serve(address, state, !args.no_open).await
}
