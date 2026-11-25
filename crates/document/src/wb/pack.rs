//! Packed stroke points (contract §2.6): 6 bytes a point, little-endian — `int16 dx`, `int16 dy`
//! (⅛ px, differences of absolutely quantised positions), `uint8 pressure`, `uint8 dt` (4 ms).

/// Largest step one point can carry, in ⅛ px.
const STEP: i64 = i16::MAX as i64;

/// Quantise `v` to ⅛ px, rounding half away from zero (`f64::round`).
fn q(v: f64) -> i64 {
    (v * 8.0).round() as i64
}

/// Pack `(x, y, pressure, t ms)` points against origin `o`. A step over ±32767 (4095.875 px)
/// gets linearly interpolated points in between (their pressure and time interpolated too).
pub fn encode(o: [f64; 2], pts: &[(f64, f64, f32, u32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pts.len() * 6);
    let (mut px, mut py) = (0i64, 0i64);
    let mut pt: u32 = 0;
    let mut prev: Option<(f64, f64, f32, u32)> = None;
    for (i, &(x, y, p, t)) in pts.iter().enumerate() {
        let (qx, qy) = (q(x - o[0]), q(y - o[1]));
        if let Some((x0, y0, p0, t0)) = prev {
            let steps = ((qx - px).abs().max((qy - py).abs()) + STEP - 1) / STEP;
            for k in 1..steps {
                let f = k as f64 / steps as f64;
                let ix = q(x0 + (x - x0) * f - o[0]);
                let iy = q(y0 + (y - y0) * f - o[1]);
                let ip = p0 + (p - p0) * f as f32;
                let it = t0 as f64 + (t as f64 - t0 as f64) * f;
                push(&mut out, ix - px, iy - py, ip, dt(it.round() as u32, pt));
                (px, py, pt) = (ix, iy, it.round() as u32);
            }
        }
        let d = if i == 0 { 0 } else { dt(t, pt) };
        let (dx, dy) = if i == 0 { (0, 0) } else { (qx - px, qy - py) };
        push(&mut out, dx, dy, p, d);
        // Point 0 sits on the origin by definition; its real offset (if any) is folded into the next step.
        (px, py) = if i == 0 { (0, 0) } else { (qx, qy) };
        pt = t;
        prev = Some((x, y, p, t));
    }
    out
}

fn dt(t: u32, prev: u32) -> u8 {
    ((t.saturating_sub(prev) as f64 / 4.0).round() as u32).min(255) as u8
}

fn push(out: &mut Vec<u8>, dx: i64, dy: i64, p: f32, dt: u8) {
    out.extend_from_slice(&(dx.clamp(-STEP, STEP) as i16).to_le_bytes());
    out.extend_from_slice(&(dy.clamp(-STEP, STEP) as i16).to_le_bytes());
    out.push((p.clamp(0.0, 1.0) * 255.0).round() as u8);
    out.push(dt);
}

/// Unpack to `(x, y, pressure 0..1, t ms)`; a trailing partial point is ignored.
pub fn decode(o: [f64; 2], bytes: &[u8]) -> Vec<(f64, f64, f32, u32)> {
    let (mut sx, mut sy, mut st) = (0i64, 0i64, 0u32);
    bytes
        .chunks_exact(6)
        .map(|c| {
            sx += i16::from_le_bytes([c[0], c[1]]) as i64;
            sy += i16::from_le_bytes([c[2], c[3]]) as i64;
            st += c[5] as u32 * 4;
            (
                o[0] + sx as f64 / 8.0,
                o[1] + sy as f64 / 8.0,
                c[4] as f32 / 255.0,
                st,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn golden_vector() {
        let o = [100.0, 50.0];
        let pts = [
            (100.0, 50.0, 0.5, 0),
            (101.5, 50.25, 1.0, 8),
            (99.5, 49.0, 0.2, 20),
        ];
        let b = encode(o, &pts);
        assert_eq!(hex(&b), "0000000080000c000200ff02f0fff6ff3303");
        let d = decode(o, &b);
        assert_eq!(d.len(), 3);
        for (got, want) in d.iter().zip(pts) {
            assert_eq!((got.0, got.1, got.3), (want.0, want.1, want.3));
        }
        assert_eq!(d[0].2, 128.0 / 255.0);
        assert_eq!(d[1].2, 1.0);
        assert_eq!(d[2].2, 51.0 / 255.0);
    }

    #[test]
    fn round_trip() {
        let o = [-40.25, 3000.5];
        let mut pts = Vec::new();
        let mut seed = 7u64;
        let mut t = 0u32;
        for i in 0..500 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = o[0] + (i as f64 * 0.75).sin() * 300.0 + (seed >> 40) as f64 / 1e6;
            let y = o[1] + i as f64 * 0.5;
            t += 4 * ((seed >> 60) as u32 + 1);
            pts.push((x, y, ((seed >> 32) & 0xff) as f32 / 255.0, t));
        }
        pts[0] = (o[0], o[1], 0.5, 0);
        let d = decode(o, &encode(o, &pts));
        assert_eq!(d.len(), pts.len());
        for (g, w) in d.iter().zip(&pts) {
            assert!((g.0 - w.0).abs() <= 1.0 / 16.0 + 1e-9, "{g:?} vs {w:?}");
            assert!((g.1 - w.1).abs() <= 1.0 / 16.0 + 1e-9);
            assert!((g.2 - w.2).abs() <= 1.0 / 510.0 + 1e-6);
            assert_eq!(g.3, w.3);
        }
    }

    #[test]
    fn long_step_is_interpolated() {
        let o = [0.0, 0.0];
        let pts = [(0.0, 0.0, 0.5, 0), (10_000.0, -5.0, 0.5, 40)];
        let b = encode(o, &pts);
        let d = decode(o, &b);
        // 80 000 ⅛-px units need three steps of ≤ 32 767.
        assert_eq!(d.len(), 4);
        let last = d.last().unwrap();
        assert_eq!((last.0, last.1, last.3), (10_000.0, -5.0, 40));
        for c in b.chunks_exact(6) {
            let dx = i16::from_le_bytes([c[0], c[1]]);
            assert!(dx.unsigned_abs() <= 32_767);
        }
        for w in d.windows(2) {
            assert!(w[1].0 > w[0].0, "interpolated points move forward");
        }
    }
}
