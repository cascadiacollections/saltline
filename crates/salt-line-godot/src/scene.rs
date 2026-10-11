//! Shared presentation helpers. World geometry never follows window dimensions.
use crate::{
    palette::{slot, Palette},
    raster::{Canvas, Rect},
    sprites::{Sprites, BARREL_FRAME, BARREL_PIVOT},
};
use salt_line_sim::{ballistics::Shot, modifiers::Loadout, Vec2};

/// Fit an arena with uniform scale and centered margins; y is up in world space.
#[derive(Clone, Copy, Debug)]
pub struct WorldViewport {
    scale: f64,
    left: f64,
    top: f64,
    height: f64,
}
impl WorldViewport {
    pub fn fit(width: i32, height: i32, world_width: f64, world_height: f64) -> Self {
        assert!(width > 0 && height > 0 && world_width > 0.0 && world_height > 0.0);
        let scale = (width as f64 / world_width).min(height as f64 / world_height);
        Self {
            scale,
            left: (width as f64 - world_width * scale) * 0.5,
            top: (height as f64 - world_height * scale) * 0.5,
            height: world_height,
        }
    }
    pub fn project(self, point: Vec2) -> (i32, i32) {
        (
            (self.left + point.x * self.scale).round() as i32,
            (self.top + (self.height - point.y) * self.scale).round() as i32 - 1,
        )
    }
    /// Destination rectangle for a full-world background at its native aspect.
    pub fn bounds(self, world_width: f64) -> (f64, f64, f64, f64) {
        (
            self.left,
            self.top,
            world_width * self.scale,
            self.height * self.scale,
        )
    }
}

/// Native sprite rendering shared by artillery and Trial. Barrel root is
/// overlapped by the hull, and both modes use the same pivot and frame selection.
pub fn draw_emplacement(
    canvas: &mut Canvas,
    sprites: Option<&Sprites>,
    loadout: &Loadout,
    shot: Shot,
    pivot_x: i32,
    pivot_y: i32,
) {
    let Some(sprites) = sprites else {
        return;
    };
    let (tank, pivot) = sprites.tank(loadout);
    if let Some(barrel) = sprites.barrel(loadout) {
        canvas.blit_region(
            barrel,
            pivot_x - BARREL_PIVOT.0,
            pivot_y - BARREL_PIVOT.1,
            Rect {
                x: Sprites::barrel_frame_x(shot.quantized(loadout).barrel_frame()),
                y: 0,
                w: BARREL_FRAME,
                h: BARREL_FRAME,
            },
        );
    }
    if let Some(tank) = tank {
        canvas.blit(tank, pivot_x - pivot.0, pivot_y - pivot.1);
    }
}

/// Decorative coastal layers, with a clear open sky for target readability.
/// Does not create terrain collision or consume simulation randomness.
pub fn draw_coast(canvas: &mut Canvas, pal: &Palette) {
    let w = canvas.w;
    let h = canvas.h;
    let horizon = h - 30;
    for x in 0..w {
        let peak = ((x / 19) % 5 - 2).abs() * 3 + ((x / 7) % 3);
        canvas.vline(x, horizon - peak, horizon, pal.get(slot::SKY_LO));
    }
    canvas.rect(0, horizon + 1, w, 30, pal.get(slot::SKY_MID));
    for y in (horizon + 3..h).step_by(4) {
        for x in (0..w).step_by(23) {
            canvas.hline(x + y % 11, x + y % 11 + 12, y, pal.get(slot::SKY_LO));
        }
    }
    for x in [w / 2, w * 3 / 4, w - 28] {
        canvas.rect(x, horizon - 15, 5, 16, pal.get(slot::SHADOW));
        canvas.rect(x - 2, horizon - 12, 9, 2, pal.get(slot::ROCK));
        canvas.vline(x + 2, horizon - 20, horizon - 15, pal.get(slot::ST));
        canvas.hline(x + 5, x + 25, horizon - 6, pal.get(slot::SHADOW));
    }
    canvas.rect(0, h - 7, 64, 7, pal.get(slot::ROCK));
    for x in (0..64).step_by(5) {
        canvas.hline(x, x + 3, h - 7 - (x / 5) % 2, pal.get(slot::SAND));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wide_window_adds_margins_without_stretching_the_arena() {
        let native = WorldViewport::fit(320, 180, 320.0, 180.0);
        let wide = WorldViewport::fit(400, 180, 320.0, 180.0);
        assert_eq!(wide.bounds(320.0), (40.0, 0.0, 320.0, 180.0));
        for p in [Vec2::new(30.0, 15.0), Vec2::new(200.0, 90.0)] {
            let a = native.project(p);
            let b = wide.project(p);
            assert_eq!(b, (a.0 + 40, a.1));
        }
    }
    #[test]
    fn hd_preserves_uniform_world_scale() {
        let view = WorldViewport::fit(1280, 720, 320.0, 180.0);
        let a = view.project(Vec2::new(30.0, 15.0));
        let b = view.project(Vec2::new(40.0, 25.0));
        assert_eq!((b.0 - a.0, b.1 - a.1), (40, -40));
    }
}
