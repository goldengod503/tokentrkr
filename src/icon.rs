use ksni::Icon;

const SIZE: i32 = 24;
const CENTER: f64 = 11.5; // (SIZE-1)/2
const OUTER_R: f64 = 11.0;
const INNER_R: f64 = 8.0;


/// Build a 24x24 tray icon: circular gauge ring that fills based on usage percent,
/// with a bold "T" glyph in the center.
///
/// Colors shift from teal (low usage) through amber to red (high usage).
pub fn build_icon(used_percent: Option<f64>) -> Icon {
    let pct = used_percent.unwrap_or(0.0).clamp(0.0, 100.0);
    let fraction = pct / 100.0;

    // Color for the filled portion of the ring
    let (ring_r, ring_g, ring_b) = usage_color(fraction);
    // Dim version for the unfilled portion
    let (bg_r, bg_g, bg_b) = (50u8, 55, 60);
    // "T" glyph color — white with slight warmth
    let (t_r, t_g, t_b) = (230u8, 235, 240);

    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f64 - CENTER;
            let dy = y as f64 - CENTER;
            let dist = (dx * dx + dy * dy).sqrt();

            let (a, r, g, b) = if is_t_glyph(x, y) {
                // Draw the "T"
                (255, t_r, t_g, t_b)
            } else if dist >= INNER_R && dist <= OUTER_R {
                // Ring area — check if this pixel is in the filled arc
                // Arc starts at top (12 o'clock) and sweeps clockwise
                let angle = clockwise_angle(dx, dy);
                let fill_angle = fraction * std::f64::consts::TAU;

                // Anti-alias the ring edges
                let ring_aa = ring_alpha(dist);

                if angle <= fill_angle {
                    // Filled portion — with edge softening at the fill boundary
                    let edge_fade = if (fill_angle - angle) < 0.08 {
                        ((fill_angle - angle) / 0.08) as f64
                    } else {
                        1.0
                    };
                    let alpha = (ring_aa * edge_fade * 255.0) as u8;
                    (alpha, ring_r, ring_g, ring_b)
                } else {
                    // Unfilled background ring
                    let alpha = (ring_aa * 180.0) as u8;
                    (alpha, bg_r, bg_g, bg_b)
                }
            } else {
                (0, 0, 0, 0)
            };

            let idx = ((y * SIZE + x) * 4) as usize;
            data[idx] = a;
            data[idx + 1] = r;
            data[idx + 2] = g;
            data[idx + 3] = b;
        }
    }

    Icon {
        width: SIZE,
        height: SIZE,
        data,
    }
}

/// Angle from top (12 o'clock), sweeping clockwise, in radians [0, 2π)
fn clockwise_angle(dx: f64, dy: f64) -> f64 {
    // atan2 gives angle from positive X axis, counter-clockwise
    // We want angle from negative Y axis (top), clockwise
    let angle = dy.atan2(dx) + std::f64::consts::FRAC_PI_2;
    if angle < 0.0 {
        angle + std::f64::consts::TAU
    } else {
        angle
    }
}

/// Anti-aliased alpha for the ring edges
fn ring_alpha(dist: f64) -> f64 {
    let outer_edge = 1.0 - (dist - OUTER_R + 0.5).clamp(0.0, 1.0);
    let inner_edge = (dist - INNER_R + 0.5).clamp(0.0, 1.0);
    outer_edge * inner_edge
}

/// Map usage fraction [0,1] to RGB color:
/// 0.0 = teal (#2DD4BF) → 0.5 = amber (#F59E0B) → 1.0 = red (#EF4444)
fn usage_color(fraction: f64) -> (u8, u8, u8) {
    if fraction <= 0.5 {
        let t = fraction * 2.0;
        (
            lerp_u8(45, 245, t),  // 2D → F5
            lerp_u8(212, 158, t), // D4 → 9E
            lerp_u8(191, 11, t),  // BF → 0B
        )
    } else {
        let t = (fraction - 0.5) * 2.0;
        (
            lerp_u8(245, 239, t), // F5 → EF
            lerp_u8(158, 68, t),  // 9E → 44
            lerp_u8(11, 68, t),   // 0B → 44
        )
    }
}

fn lerp_u8(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t).round() as u8
}

/// Define a chunky "T" glyph in the center of the 24x24 icon.
/// Manually placed pixels for a crisp look at small size.
fn is_t_glyph(x: i32, y: i32) -> bool {
    // Horizontal bar of T: y=7..9, x=8..16
    let h_bar = y >= 7 && y <= 8 && x >= 8 && x <= 15;
    // Vertical stem of T: y=9..17, x=10..13
    let v_stem = y >= 9 && y <= 16 && x >= 10 && x <= 13;
    h_bar || v_stem
}
