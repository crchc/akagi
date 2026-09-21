//! Advance Mahjong Soul's end-of-game screens and request the next match.
//! A confirmed protocol game end starts the flow; every press is gated by
//! pixels from the live canvas so animation delays and manual clicks do not
//! turn the recorded coordinates into blind presses.

use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::page::Viewport;
use chromiumoxide::page::{Page, ScreenshotParams};
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

use crate::autoplay::cdp_input::{dispatch_click, evaluate_canvas_rect};
use crate::autoplay::context::{AutoplayContext, CanvasRect};
use crate::config::{AppConfig, Platform};
use crate::ipc::{commands::complete_majsoul_game, AppState};
use crate::schema::{GameEndReason, MjaiEvent};

const REFERENCE_WIDTH: f64 = 1533.0;
const REFERENCE_HEIGHT: f64 = 861.0;
const POLL: Duration = Duration::from_millis(600);
const STEP_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Confirm,
    Rematch,
    Dialog,
    Other,
}

impl Screen {
    fn point(self) -> Option<(f64, f64)> {
        match self {
            Self::Confirm => Some((1390.0, 780.0)),
            Self::Rematch => Some((1165.0, 788.0)),
            Self::Dialog => Some((624.0, 634.0)),
            Self::Other => None,
        }
    }
}

/// Count each confirmed game once, then advance if another game remains.
pub async fn watch(state: AppState) {
    let mut rx = state.mjai_bus.subscribe();
    let mut config_rx = state.config_bus.subscribe();
    let mut in_game = false;
    let mut result_pending = false;
    loop {
        tokio::select! {
        event = rx.recv() => match event {
            Ok(MjaiEvent::StartGame { .. }) => {
                in_game = true;
                result_pending = false;
            }
            Ok(MjaiEvent::EndGame {
                reason: GameEndReason::Confirmed,
                ..
            }) if in_game => {
                in_game = false;
                let remaining = match complete_majsoul_game(&state).await {
                    Ok(value) => value,
                    Err(e) => {
                        warn!("autoplay: failed to save remaining games: {e}");
                        continue;
                    }
                };
                let guard = state.config.read().await;
                let majsoul = guard.platform.kind == Platform::Majsoul;
                let enabled = guard.autoplay.enabled && majsoul;
                drop(guard);
                info!(remaining, "autoplay: Mahjong Soul game completed");
                result_pending = majsoul && (!enabled || remaining == 0);
                if enabled && remaining > 0 {
                    if let Err(e) = advance(&state.config, &state.autoplay_context, &mut rx, &mut in_game).await {
                        warn!("autoplay: rematch flow stopped: {e:#}");
                    }
                }
            }
            Ok(MjaiEvent::EndGame { .. }) => {
                in_game = false;
                result_pending = false;
            }
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return,
        },
        update = config_rx.recv() => if let Ok(config) = update {
            if result_pending
                && config.autoplay.enabled
                && config.platform.kind == Platform::Majsoul
                && config.autoplay.majsoul.remaining_games > 0
            {
                result_pending = false;
                if let Err(e) = advance(&state.config, &state.autoplay_context, &mut rx, &mut in_game).await {
                    warn!("autoplay: rematch flow stopped: {e:#}");
                }
            }
        },
        }
    }
}

async fn advance(
    cfg: &Arc<RwLock<AppConfig>>,
    ctx: &Arc<AutoplayContext>,
    rx: &mut broadcast::Receiver<MjaiEvent>,
    in_game: &mut bool,
) -> anyhow::Result<()> {
    let page = ctx
        .page
        .read()
        .await
        .as_ref()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no Chromium page handle"))?;

    // The first two result screens use the same gold confirmation button.
    // If a user already passed either screen, the next visible stage wins.
    for _ in 0..2 {
        let (screen, rect) =
            wait_for(&page, cfg, rx, in_game, &[Screen::Confirm, Screen::Rematch]).await?;
        if screen == Screen::Rematch {
            break;
        }
        press(&page, rect, screen, cfg).await?;
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    let (screen, rect) =
        wait_for(&page, cfg, rx, in_game, &[Screen::Rematch, Screen::Dialog]).await?;
    if screen == Screen::Rematch {
        press(&page, rect, Screen::Rematch, cfg).await?;
    }
    let (_, rect) = wait_for(&page, cfg, rx, in_game, &[Screen::Dialog]).await?;
    press(&page, rect, Screen::Dialog, cfg).await?;
    info!("autoplay: rematch dialog confirmation clicked");
    Ok(())
}

async fn wait_for(
    page: &Page,
    cfg: &Arc<RwLock<AppConfig>>,
    rx: &mut broadcast::Receiver<MjaiEvent>,
    in_game: &mut bool,
    wanted: &[Screen],
) -> anyhow::Result<(Screen, CanvasRect)> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while Instant::now() < deadline {
        // A manually started game makes all pending result-screen clicks stale.
        loop {
            match rx.try_recv() {
                Ok(MjaiEvent::StartGame { .. }) => {
                    *in_game = true;
                    anyhow::bail!("another game started");
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    anyhow::bail!("mjai events lagged during rematch")
                }
                Err(broadcast::error::TryRecvError::Closed) => anyhow::bail!("mjai bus closed"),
            }
        }
        let guard = cfg.read().await;
        let active = guard.autoplay.enabled
            && guard.platform.kind == Platform::Majsoul
            && guard.autoplay.majsoul.remaining_games > 0;
        drop(guard);
        if !active {
            anyhow::bail!("rematch disabled in settings");
        }
        if let Ok(rect) = evaluate_canvas_rect(page).await {
            if rect.width > 0.0 && rect.height > 0.0 {
                match capture_screen(page, rect).await {
                    Ok(screen) if wanted.contains(&screen) => return Ok((screen, rect)),
                    Ok(_) => {}
                    Err(e) => warn!("autoplay: rematch screenshot failed: {e:#}"),
                }
            }
        }
        tokio::time::sleep(POLL).await;
    }
    anyhow::bail!("timed out waiting for {:?}", wanted)
}

async fn capture_screen(page: &Page, rect: CanvasRect) -> anyhow::Result<Screen> {
    let clip = Viewport {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        scale: 1.0,
    };
    let bytes = page
        .screenshot(ScreenshotParams::builder().clip(clip).build())
        .await?;
    classify_png(&bytes)
}

fn classify_png(bytes: &[u8]) -> anyhow::Result<Screen> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut data = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| anyhow::anyhow!("PNG buffer too large"))?
    ];
    let info = reader.next_frame(&mut data)?;
    let data = &data[..info.buffer_size()];
    let sample = |x: f64, y: f64| -> Option<[u8; 3]> {
        let px = ((x / REFERENCE_WIDTH) * f64::from(info.width)).floor() as usize;
        let py = ((y / REFERENCE_HEIGHT) * f64::from(info.height)).floor() as usize;
        if px >= info.width as usize || py >= info.height as usize {
            return None;
        }
        let channels = match info.color_type {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            _ => return None,
        };
        let i = (py * info.width as usize + px) * channels;
        Some([data[i], data[i + 1], data[i + 2]])
    };
    Ok(classify_pixels(&sample))
}

fn classify_pixels(sample: &impl Fn(f64, f64) -> Option<[u8; 3]>) -> Screen {
    let gold = |x, y| {
        sample(x, y).is_some_and(|[r, g, b]| r > 200 && g > 160 && b > 65 && b < 180 && r > g)
    };
    let blue = |x, y| {
        sample(x, y)
            .is_some_and(|[r, g, b]| b > 95 && b > g.saturating_add(25) && g > r.saturating_add(15))
    };
    if gold(550.0, 634.0) && gold(700.0, 634.0) {
        Screen::Dialog
    } else if blue(1165.0, 788.0) && gold(1340.0, 780.0) {
        Screen::Rematch
    } else if gold(1340.0, 780.0) && gold(1440.0, 780.0) {
        Screen::Confirm
    } else {
        Screen::Other
    }
}

async fn press(
    page: &Page,
    rect: CanvasRect,
    screen: Screen,
    cfg: &Arc<RwLock<AppConfig>>,
) -> anyhow::Result<()> {
    let (x, y) = screen.point().expect("only actionable screens are pressed");
    let px = rect.x + x / REFERENCE_WIDTH * rect.width;
    let py = rect.y + y / REFERENCE_HEIGHT * rect.height;
    anyhow::ensure!(rect.contains(px, py), "rematch click outside canvas");
    let guard = cfg.read().await;
    let timing = &guard.autoplay.majsoul;
    let hover = timing.hover_delay_ms.max(100);
    let hold = timing.click_hold_ms.max(50);
    drop(guard);
    dispatch_click(page, px, py, hover, hold).await?;
    info!(?screen, px, py, "autoplay: result-screen click");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_button_signatures_select_each_stage() {
        let samples = [
            ((1340.0, 780.0), [250, 223, 121]),
            ((1440.0, 780.0), [247, 224, 118]),
            ((1165.0, 788.0), [49, 77, 135]),
            ((550.0, 634.0), [247, 214, 112]),
            ((700.0, 634.0), [247, 214, 112]),
        ];
        let sample = |x, y| samples.iter().find(|(p, _)| *p == (x, y)).map(|(_, c)| *c);
        assert_eq!(classify_pixels(&sample), Screen::Dialog);
        let no_dialog = |x, y| if y == 634.0 { None } else { sample(x, y) };
        assert_eq!(classify_pixels(&no_dialog), Screen::Rematch);
        let no_blue = |x, y| if x == 1165.0 { None } else { no_dialog(x, y) };
        assert_eq!(classify_pixels(&no_blue), Screen::Confirm);
    }
}
