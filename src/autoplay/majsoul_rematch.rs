//! Advance Mahjong Soul's end-of-game screens and request the next match.
//! Only a confirmed protocol game end that follows a started game starts the
//! result-screen loop, which keeps detecting and clicking buttons until the
//! next game starts. Full viewport screenshots detect result buttons by their
//! color-filled regions; unrecognized interstitial screens receive a click at
//! the Rematch position until a match has been requested.

use std::io::Cursor;
use std::time::Duration;

use chromiumoxide::page::{Page, ScreenshotParams};
use serde::Deserialize;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

use crate::autoplay::cdp_input::dispatch_click;
use crate::autoplay::context::CanvasRect;
use crate::config::{AppConfig, Platform};
use crate::ipc::{commands::complete_majsoul_game, AppState};
use crate::schema::{GameEndReason, MjaiEvent};

// 1600x900 is a whole-number 16:9 coordinate space. The button rectangles
// were measured on a 1533x861 capture and rounded once into this space.
const GAME_WIDTH: f64 = 1600.0;
const GAME_HEIGHT: f64 = 900.0;
const POLL: Duration = Duration::from_millis(600);
const AFTER_CLICK: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Region {
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
}

const CONFIRM: Region = Region {
    left: 1352,
    top: 797,
    right: 1550,
    bottom: 849,
};
const REMATCH: Region = Region {
    left: 1119,
    top: 797,
    right: 1313,
    bottom: 849,
};
const DIALOG: Region = Region {
    left: 530,
    top: 636,
    right: 772,
    bottom: 686,
};

impl Region {
    fn center(self) -> (f64, f64) {
        (
            f64::from(self.left + self.right) / 2.0,
            f64::from(self.top + self.bottom) / 2.0,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Confirm,
    Rematch,
    Dialog,
    Other,
}

/// The button to click on `screen`, if any. Once a match has been requested,
/// only recognized match buttons are clicked so nothing lands on the lobby.
fn target(screen: Screen, requested: bool) -> Option<Region> {
    match screen {
        // A blind advance click can hit Rematch before its blue region is
        // recognized, so the dialog stays actionable after a request.
        Screen::Dialog => Some(DIALOG),
        Screen::Rematch => Some(REMATCH),
        _ if requested => None,
        Screen::Confirm => Some(CONFIRM),
        // Unrecognized interstitials advance at the Rematch position.
        Screen::Other => Some(REMATCH),
    }
}

/// The whole flow, including the remaining-games countdown, only runs while
/// "Enable autoplay" is on.
fn autoplay_enabled(config: &AppConfig) -> bool {
    config.autoplay.enabled && config.platform.kind == Platform::Majsoul
}

fn rematch_enabled(config: &AppConfig) -> bool {
    autoplay_enabled(config) && config.autoplay.majsoul.remaining_games > 0
}

#[derive(Deserialize)]
struct Geometry {
    canvas: CanvasRect,
    viewport_width: f64,
    viewport_height: f64,
}

/// Count each confirmed game once, then advance if another game remains.
/// Only the transition from a started game to its confirmed end triggers the
/// flow, and only while autoplay is enabled; settings changes while idle on a
/// result screen do not.
pub async fn watch(state: AppState) {
    let mut rx = state.mjai_bus.subscribe();
    let mut in_game = false;
    loop {
        match rx.recv().await {
            Ok(MjaiEvent::StartGame { .. }) => in_game = true,
            Ok(MjaiEvent::EndGame {
                reason: GameEndReason::Confirmed,
                ..
            }) if in_game => {
                in_game = false;
                if !autoplay_enabled(&*state.config.read().await) {
                    continue;
                }
                match complete_majsoul_game(&state).await {
                    Ok(remaining) => info!(remaining, "autoplay: Mahjong Soul game completed"),
                    Err(e) => {
                        warn!("autoplay: failed to save remaining games: {e}");
                        continue;
                    }
                }
                if rematch_enabled(&*state.config.read().await) {
                    match advance(&state, &mut rx).await {
                        Ok(()) => in_game = true,
                        Err(e) => warn!("autoplay: rematch flow stopped: {e:#}"),
                    }
                }
            }
            Ok(MjaiEvent::EndGame { .. }) => in_game = false,
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Keep detecting and clicking result-screen buttons until the next game
/// starts (`Ok`) or rematching is turned off.
async fn advance(state: &AppState, rx: &mut broadcast::Receiver<MjaiEvent>) -> anyhow::Result<()> {
    let page = state
        .autoplay_context
        .page
        .read()
        .await
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no Chromium page handle"))?;
    let mut requested = false;
    loop {
        loop {
            match rx.try_recv() {
                Ok(MjaiEvent::StartGame { .. }) => return Ok(()),
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(e) => anyhow::bail!("mjai bus during rematch: {e}"),
            }
        }
        anyhow::ensure!(
            rematch_enabled(&*state.config.read().await),
            "rematch disabled in settings"
        );
        let delay = match click_visible_button(&page, &state.config, &mut requested).await {
            Ok(true) => AFTER_CLICK,
            Ok(false) => POLL,
            Err(e) => {
                warn!("autoplay: result-screen step failed: {e:#}");
                POLL
            }
        };
        tokio::time::sleep(delay).await;
    }
}

/// Screenshot once and click the matching button. Returns whether it clicked.
async fn click_visible_button(
    page: &Page,
    cfg: &RwLock<AppConfig>,
    requested: &mut bool,
) -> anyhow::Result<bool> {
    let (screen, rect) = capture_screen(page).await?;
    let Some(region) = target(screen, *requested) else {
        return Ok(false);
    };
    let (x, y) = region.center();
    let px = rect.x + x / GAME_WIDTH * rect.width;
    let py = rect.y + y / GAME_HEIGHT * rect.height;
    anyhow::ensure!(rect.contains(px, py), "rematch click outside canvas");
    let (hover, hold) = {
        let timing = &cfg.read().await.autoplay.majsoul;
        (timing.hover_delay_ms.max(100), timing.click_hold_ms.max(50))
    };
    dispatch_click(page, px, py, hover, hold).await?;
    info!(?screen, px, py, "autoplay: result-screen click");
    *requested |= matches!(screen, Screen::Dialog | Screen::Rematch);
    Ok(true)
}

async fn capture_screen(page: &Page) -> anyhow::Result<(Screen, CanvasRect)> {
    let geometry = canvas_geometry(page).await?;
    let bytes = page.screenshot(ScreenshotParams::builder().build()).await?;
    let screen = classify_png(&bytes, &geometry)?;
    Ok((screen, geometry.canvas))
}

async fn canvas_geometry(page: &Page) -> anyhow::Result<Geometry> {
    let result = page.evaluate("(()=>{const c=document.querySelector('canvas');if(!c)return null;const r=c.getBoundingClientRect();return {canvas:{x:r.x,y:r.y,width:r.width,height:r.height},viewport_width:innerWidth,viewport_height:innerHeight};})()").await?;
    let geometry: Geometry = serde_json::from_value(
        result
            .value()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("canvas geometry unavailable"))?,
    )?;
    let r = geometry.canvas;
    anyhow::ensure!(
        geometry.viewport_width.is_finite()
            && geometry.viewport_height.is_finite()
            && geometry.viewport_width > 0.0
            && geometry.viewport_height > 0.0
            && r.x.is_finite()
            && r.y.is_finite()
            && r.width.is_finite()
            && r.height.is_finite()
            && r.width > 0.0
            && r.height > 0.0
            && r.x >= -2.0
            && r.y >= -2.0
            && r.x + r.width <= geometry.viewport_width + 2.0
            && r.y + r.height <= geometry.viewport_height + 2.0,
        "invalid canvas geometry"
    );
    Ok(geometry)
}

fn classify_png(bytes: &[u8], geometry: &Geometry) -> anyhow::Result<Screen> {
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
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => anyhow::bail!("unsupported screenshot color format"),
    };
    let sx = f64::from(info.width) / geometry.viewport_width;
    let sy = f64::from(info.height) / geometry.viewport_height;
    let rect = CanvasRect {
        x: geometry.canvas.x * sx,
        y: geometry.canvas.y * sy,
        width: geometry.canvas.width * sx,
        height: geometry.canvas.height * sy,
    };
    let sample = |x: f64, y: f64| -> Option<[u8; 3]> {
        let px = (rect.x + x / GAME_WIDTH * rect.width).floor() as usize;
        let py = (rect.y + y / GAME_HEIGHT * rect.height).floor() as usize;
        if px >= info.width as usize || py >= info.height as usize {
            return None;
        }
        let i = (py * info.width as usize + px) * channels;
        Some([data[i], data[i + 1], data[i + 2]])
    };
    let rematch_blue = coverage(REMATCH, &sample, is_blue)?;
    let dialog_gold = coverage(DIALOG, &sample, is_gold)?;
    let confirm_gold = coverage(CONFIRM, &sample, is_gold)?;
    let screen = classify_coverage(rematch_blue, dialog_gold, confirm_gold);
    if screen == Screen::Other {
        info!(rematch_blue, dialog_gold, confirm_gold, canvas = ?geometry.canvas, "autoplay: result screen unrecognized");
    }
    Ok(screen)
}

fn is_gold([r, g, b]: [u8; 3]) -> bool {
    r > 200 && g > 160 && b > 65 && b < 180 && r > g
}

fn is_blue([r, g, b]: [u8; 3]) -> bool {
    b > 95 && b > g.saturating_add(25) && g > r.saturating_add(15)
}

fn coverage(
    region: Region,
    sample: &impl Fn(f64, f64) -> Option<[u8; 3]>,
    matches: impl Fn([u8; 3]) -> bool,
) -> anyhow::Result<f64> {
    let mut count = 0;
    for row in 0..4 {
        for col in 0..8 {
            let x = f64::from(region.left)
                + f64::from(region.right - region.left)
                    * (0.1 + 0.8 * (f64::from(col) + 0.5) / 8.0);
            let y = f64::from(region.top)
                + f64::from(region.bottom - region.top)
                    * (0.1 + 0.8 * (f64::from(row) + 0.5) / 4.0);
            let pixel =
                sample(x, y).ok_or_else(|| anyhow::anyhow!("button region outside screenshot"))?;
            count += u32::from(matches(pixel));
        }
    }
    Ok(f64::from(count) / 32.0)
}

fn classify_coverage(rematch_blue: f64, dialog_gold: f64, confirm_gold: f64) -> Screen {
    // The result page can show Confirm and Rematch together; the modal
    // darkens both. This order keeps the right-hand Confirm from taking us
    // back to the lobby after Rematch appears.
    if dialog_gold >= 0.55 {
        Screen::Dialog
    } else if rematch_blue >= 0.40 {
        Screen::Rematch
    } else if confirm_gold >= 0.55 && rematch_blue == 0.0 {
        Screen::Confirm
    } else {
        Screen::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_screens_choose_their_buttons() {
        // Color coverage measured within the three supplied button regions.
        assert_eq!(classify_coverage(0.8125, 0.0, 0.875), Screen::Rematch);
        assert_eq!(classify_coverage(0.0, 0.90625, 0.0), Screen::Dialog);
        assert_eq!(classify_coverage(0.0, 0.0, 0.875), Screen::Confirm);
        // A partly visible blue button must not be mistaken for Confirm.
        assert_eq!(classify_coverage(1.0 / 32.0, 0.0, 0.875), Screen::Other);
        assert_eq!(classify_coverage(0.125, 0.0, 0.875), Screen::Other);
    }

    #[test]
    fn buttons_are_clicked_every_time_they_are_seen() {
        for requested in [false, true] {
            assert_eq!(target(Screen::Dialog, requested), Some(DIALOG));
            assert_eq!(target(Screen::Rematch, requested), Some(REMATCH));
        }
        assert_eq!(target(Screen::Confirm, false), Some(CONFIRM));
        assert_eq!(target(Screen::Other, false), Some(REMATCH));
    }

    #[test]
    fn requested_match_never_clicks_blindly() {
        assert_eq!(target(Screen::Other, true), None);
        assert_eq!(target(Screen::Confirm, true), None);
    }

    #[test]
    fn full_viewport_png_uses_the_actual_canvas_rect() {
        let (width, height) = (200_u32, 120_u32);
        let canvas = CanvasRect {
            x: 20.0,
            y: 15.0,
            width: 160.0,
            height: 90.0,
        };
        let geometry = Geometry {
            canvas,
            viewport_width: 200.0,
            viewport_height: 120.0,
        };
        let mut pixels = vec![0_u8; width as usize * height as usize * 3];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let gx = (x as f64 - canvas.x) / canvas.width * GAME_WIDTH;
                let gy = (y as f64 - canvas.y) / canvas.height * GAME_HEIGHT;
                let color = if gx >= f64::from(REMATCH.left)
                    && gx <= f64::from(REMATCH.right)
                    && gy >= f64::from(REMATCH.top)
                    && gy <= f64::from(REMATCH.bottom)
                {
                    [49, 77, 135]
                } else if gx >= f64::from(CONFIRM.left)
                    && gx <= f64::from(CONFIRM.right)
                    && gy >= f64::from(CONFIRM.top)
                    && gy <= f64::from(CONFIRM.bottom)
                {
                    [247, 216, 115]
                } else {
                    [0, 0, 0]
                };
                pixels[(y * width as usize + x) * 3..][..3].copy_from_slice(&color);
            }
        }
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
        assert_eq!(classify_png(&png, &geometry).unwrap(), Screen::Rematch);
    }
}
