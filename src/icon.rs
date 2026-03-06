use ab_glyph::{FontRef, PxScale, point};
use ksni::Icon;

const SIZE: i32 = 256;
const CENTER: f64 = 127.5;
const RADIUS: f64 = 120.0;

const FONT_DATA: &[u8] = include_bytes!("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf");

pub fn build_icon(used_percent: Option<f64>) -> Icon {
    let pct = used_percent.unwrap_or(0.0).clamp(0.0, 100.0);
    let (bg_r, bg_g, bg_b) = bucket_color(pct);

    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];

    // Draw the colored circle
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f64 - CENTER;
            let dy = y as f64 - CENTER;
            let dist = (dx * dx + dy * dy).sqrt();

            let alpha = if dist <= RADIUS {
                ((RADIUS - dist + 0.5).clamp(0.0, 1.0) * 255.0) as u8
            } else {
                0
            };

            let idx = ((y * SIZE + x) * 4) as usize;
            data[idx] = alpha;
            data[idx + 1] = bg_r;
            data[idx + 2] = bg_g;
            data[idx + 3] = bg_b;
        }
    }

    // Render the percentage number on top
    let text = format!("{}", pct.round() as u32);
    render_text_centered(&mut data, &text);

    Icon {
        width: SIZE,
        height: SIZE,
        data,
    }
}

fn render_text_centered(data: &mut [u8], text: &str) {
    use ab_glyph::{Font, ScaleFont};

    let font = FontRef::try_from_slice(FONT_DATA).expect("failed to load font");

    let font_size = match text.len() {
        1 => 240.0f32,
        2 => 190.0,
        _ => 140.0,
    };
    let scale = PxScale::from(font_size);
    let scaled_font = font.as_scaled(scale);

    // Step 1: Collect all outlined glyphs and their pixel bounds at position (0, 0)
    // We'll render to a temp buffer first, then copy centered.
    let advances: Vec<f32> = text.chars().map(|c| {
        scaled_font.h_advance(font.glyph_id(c))
    }).collect();

    // Render text to a temporary buffer at a known position
    let tmp_size = SIZE * 2; // extra room
    let mut tmp = vec![0u8; (tmp_size * tmp_size) as usize]; // single channel coverage

    let baseline_x = SIZE as f32 / 2.0; // start in middle-ish of tmp
    let baseline_y = SIZE as f32;        // plenty of room above

    let mut cx = baseline_x;
    for (i, c) in text.chars().enumerate() {
        let glyph = font.glyph_id(c).with_scale_and_position(scale, point(cx, baseline_y));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            let ox = bounds.min.x as i32;
            let oy = bounds.min.y as i32;
            outlined.draw(|gx, gy, cov| {
                let px = gx as i32 + ox;
                let py = gy as i32 + oy;
                if px >= 0 && px < tmp_size && py >= 0 && py < tmp_size {
                    let idx = (py * tmp_size + px) as usize;
                    let v = (cov * 255.0).min(255.0) as u8;
                    // Max blend in case of overlap
                    if v > tmp[idx] {
                        tmp[idx] = v;
                    }
                }
            });
        }
        cx += advances[i];
    }

    // Step 2: Find bounding box of rendered pixels in tmp
    let mut min_x = tmp_size;
    let mut max_x = 0i32;
    let mut min_y = tmp_size;
    let mut max_y = 0i32;
    for y in 0..tmp_size {
        for x in 0..tmp_size {
            if tmp[(y * tmp_size + x) as usize] > 10 {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }

    let tw = max_x - min_x + 1;
    let th = max_y - min_y + 1;

    // Step 3: Copy from tmp to data, centered in SIZE x SIZE
    let dst_x = (SIZE - tw) / 2;
    let dst_y = (SIZE - th) / 2;

    for sy in min_y..=max_y {
        for sx in min_x..=max_x {
            let cov = tmp[(sy * tmp_size + sx) as usize];
            if cov > 0 {
                let dx = dst_x + (sx - min_x);
                let dy = dst_y + (sy - min_y);
                if dx >= 0 && dx < SIZE && dy >= 0 && dy < SIZE {
                    let idx = ((dy * SIZE + dx) * 4) as usize;
                    let existing_alpha = data[idx] as f32 / 255.0;
                    if existing_alpha > 0.0 {
                        let t = cov as f32 / 255.0;
                        data[idx + 1] = (data[idx + 1] as f32 * (1.0 - t)) as u8;
                        data[idx + 2] = (data[idx + 2] as f32 * (1.0 - t)) as u8;
                        data[idx + 3] = (data[idx + 3] as f32 * (1.0 - t)) as u8;
                    }
                }
            }
        }
    }
}

fn bucket_color(pct: f64) -> (u8, u8, u8) {
    if pct <= 25.0 {
        (45, 212, 191)
    } else if pct <= 50.0 {
        (245, 158, 11)
    } else if pct <= 75.0 {
        (249, 115, 22)
    } else if pct <= 90.0 {
        (239, 68, 68)
    } else {
        (185, 28, 28)
    }
}
