//! Advance Mahjong Soul's end-of-game screens and request the next match.
//! A confirmed protocol game end starts a bounded result-screen loop. Full
//! viewport screenshots detect the rematch and matchmaking buttons; repeated
//! presses at the result confirmation point also dismiss reward screens.

use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chromiumoxide::page::{Page, ScreenshotParams};
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

use crate::autoplay::cdp_input::dispatch_click;
use crate::autoplay::context::{AutoplayContext, CanvasRect};
use crate::config::{AppConfig, Platform};
use crate::ipc::{commands::complete_majsoul_game, AppState};
use crate::schema::{GameEndReason, MjaiEvent};

const REFERENCE_WIDTH: f64 = 1533.0;
const REFERENCE_HEIGHT: f64 = 861.0;
const POLL: Duration = Duration::from_millis(600);
const AFTER_CLICK: Duration = Duration::from_millis(1500);
const FLOW_TIMEOUT: Duration = Duration::from_secs(75);
const MAX_CLICKS: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Rematch,
    Dialog,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Results,
    MatchDialog,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Confirm,
    Rematch,
    Dialog,
}

impl Action {
    fn point(self) -> (f64, f64) {
        match self {
            Self::Confirm => (1390.0, 780.0),
            Self::Rematch => (1165.0, 788.0),
            Self::Dialog => (624.0, 634.0),
        }
    }
}

fn choose_action(phase: Phase, screen: Screen) -> Action {
    match (phase, screen) {
        (Phase::Results, Screen::Rematch) => Action::Rematch,
        (Phase::Results, _) => Action::Confirm,
        (Phase::MatchDialog, Screen::Dialog) => Action::Dialog,
        (Phase::MatchDialog, Screen::Rematch) => Action::Rematch,
        (Phase::MatchDialog, Screen::Other) => Action::Confirm,
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

    let deadline = Instant::now() + FLOW_TIMEOUT;
    let mut phase = Phase::Results;
    let mut clicks = 0;
    while Instant::now() < deadline && clicks < MAX_CLICKS {
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

        let screen = match capture_screen(&page).await {
            Ok(screen) => screen,
            Err(e) => {
                warn!("autoplay: rematch screenshot failed: {e:#}");
                tokio::time::sleep(POLL).await;
                continue;
            }
        };
        let action = choose_action(phase, screen);
        press(&page, action, cfg).await?;
        clicks += 1;
        match action {
            Action::Rematch => phase = Phase::MatchDialog,
            Action::Dialog => {
                info!(clicks, "autoplay: rematch dialog confirmation clicked");
                return Ok(());
            }
            Action::Confirm => {}
        }
        tokio::time::sleep(AFTER_CLICK).await;
    }
    anyhow::bail!("rematch stopped after {clicks} clicks without reaching matchmaking")
}

async fn capture_screen(page: &Page) -> anyhow::Result<Screen> {
    let bytes = page.screenshot(ScreenshotParams::builder().build()).await?;
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
    let rect = centered_game_rect(f64::from(info.width), f64::from(info.height))?;
    let sample = |x: f64, y: f64| -> Option<[u8; 3]> {
        let px = (rect.x + x / REFERENCE_WIDTH * rect.width).floor() as usize;
        let py = (rect.y + y / REFERENCE_HEIGHT * rect.height).floor() as usize;
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
    } else if blue(1165.0, 788.0) {
        Screen::Rematch
    } else {
        Screen::Other
    }
}

/// The game fills the largest 1533:861 rectangle centered in the viewport.
/// Use the same rule for screenshot pixels and CSS click coordinates.
fn centered_game_rect(width: f64, height: f64) -> anyhow::Result<CanvasRect> {
    anyhow::ensure!(
        width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0,
        "invalid viewport size"
    );
    let scale = (width / REFERENCE_WIDTH).min(height / REFERENCE_HEIGHT);
    let game_width = REFERENCE_WIDTH * scale;
    let game_height = REFERENCE_HEIGHT * scale;
    Ok(CanvasRect {
        x: (width - game_width) / 2.0,
        y: (height - game_height) / 2.0,
        width: game_width,
        height: game_height,
    })
}

async fn viewport_rect(page: &Page) -> anyhow::Result<CanvasRect> {
    let result = page
        .evaluate("(()=>[window.innerWidth,window.innerHeight])()")
        .await?;
    let size: [f64; 2] = serde_json::from_value(
        result
            .value()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("viewport size unavailable"))?,
    )?;
    centered_game_rect(size[0], size[1])
}

async fn press(page: &Page, action: Action, cfg: &Arc<RwLock<AppConfig>>) -> anyhow::Result<()> {
    let rect = viewport_rect(page).await?;
    let (x, y) = action.point();
    let px = rect.x + x / REFERENCE_WIDTH * rect.width;
    let py = rect.y + y / REFERENCE_HEIGHT * rect.height;
    anyhow::ensure!(rect.contains(px, py), "rematch click outside canvas");
    let guard = cfg.read().await;
    let timing = &guard.autoplay.majsoul;
    let hover = timing.hover_delay_ms.max(100);
    let hold = timing.click_hold_ms.max(50);
    drop(guard);
    dispatch_click(page, px, py, hover, hold).await?;
    info!(?action, px, py, "autoplay: result-screen click");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_button_signatures_select_each_stage() {
        let samples = [
            ((1165.0, 788.0), [49, 77, 135]),
            ((550.0, 634.0), [247, 214, 112]),
            ((700.0, 634.0), [247, 214, 112]),
        ];
        let sample = |x, y| samples.iter().find(|(p, _)| *p == (x, y)).map(|(_, c)| *c);
        assert_eq!(classify_pixels(&sample), Screen::Dialog);
        let no_dialog = |x, y| if y == 634.0 { None } else { sample(x, y) };
        assert_eq!(classify_pixels(&no_dialog), Screen::Rematch);
        let no_blue = |x, y| if x == 1165.0 { None } else { no_dialog(x, y) };
        assert_eq!(classify_pixels(&no_blue), Screen::Other);
    }

    #[test]
    fn reward_screens_keep_advancing_until_rematch_and_dialog() {
        assert_eq!(
            choose_action(Phase::Results, Screen::Other),
            Action::Confirm
        );
        assert_eq!(
            choose_action(Phase::Results, Screen::Dialog),
            Action::Confirm
        );
        assert_eq!(
            choose_action(Phase::Results, Screen::Rematch),
            Action::Rematch
        );
        assert_eq!(
            choose_action(Phase::MatchDialog, Screen::Other),
            Action::Confirm
        );
        assert_eq!(
            choose_action(Phase::MatchDialog, Screen::Dialog),
            Action::Dialog
        );
    }

    #[test]
    fn game_rect_is_centered_for_viewports_with_bars() {
        let same = centered_game_rect(1533.0, 861.0).unwrap();
        assert_eq!(
            same,
            CanvasRect {
                x: 0.0,
                y: 0.0,
                width: 1533.0,
                height: 861.0
            }
        );
        let wide = centered_game_rect(2000.0, 861.0).unwrap();
        assert_eq!(wide.y, 0.0);
        assert!((wide.x - 233.5).abs() < 0.001);
        let tall = centered_game_rect(1533.0, 1000.0).unwrap();
        assert_eq!(tall.x, 0.0);
        assert!((tall.y - 69.5).abs() < 0.001);
    }

    #[test]
    fn full_viewport_png_samples_the_centered_game() {
        let (width, height) = (200_u32, 100_u32);
        let rect = centered_game_rect(f64::from(width), f64::from(height)).unwrap();
        let x = (rect.x + 1165.0 / REFERENCE_WIDTH * rect.width).floor() as usize;
        let y = (rect.y + 788.0 / REFERENCE_HEIGHT * rect.height).floor() as usize;
        let mut pixels = vec![0_u8; width as usize * height as usize * 3];
        pixels[(y * width as usize + x) * 3..][..3].copy_from_slice(&[49, 77, 135]);
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
        }
        assert_eq!(classify_png(&png).unwrap(), Screen::Rematch);
    }
}
