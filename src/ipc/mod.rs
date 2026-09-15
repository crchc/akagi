//! Shared operations and state used by the local Web API.

pub mod capture_supervisor;
pub mod commands;
pub mod state;

pub use state::AppState;

/// Keep latest status snapshots current for newly connected browsers.
pub fn install(state: AppState) {
    let mut bot_rx = state.bot_status_bus.subscribe();
    let bot_status = state.bot_status.clone();
    tokio::spawn(async move {
        while let Ok(status) = bot_rx.recv().await {
            *bot_status.write().await = status;
        }
    });

    let mut capture_rx = state.capture_status_bus.subscribe();
    let capture_control = state.capture_control.clone();
    tokio::spawn(async move {
        while let Ok(status) = capture_rx.recv().await {
            capture_control.lock().await.status = status;
        }
    });
}
