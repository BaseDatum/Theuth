//! The screenshot ↔ desktop coordinate map (design doc 27 §3.3). Pure: no OS
//! calls. Every display is composited at logical scale, so one affine map —
//! bounding-box origin plus one downscale ratio — turns a pixel of the
//! screenshot into a global logical point; the per-display rectangles only
//! name the gutters and feed the text the model reads.

use std::fmt::Write as _;

/// One attached display in global logical points (`x`, `y` may be negative;
/// the main display is at `(0, 0)` on macOS).
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    /// 1-based, main first then by `(x, y)`; what `display:` accepts.
    pub index: u32,
    /// OS display id (CGDirectDisplayID on macOS); shown for humans only.
    pub id: u32,
    /// Display name from the OS.
    pub name: String,
    /// The primary display.
    pub main: bool,
    /// Left edge, logical points.
    pub x: i32,
    /// Top edge, logical points.
    pub y: i32,
    /// Logical width.
    pub width: u32,
    /// Logical height.
    pub height: u32,
    /// Physical pixels per logical point (`2` on Retina).
    pub scale: f32,
}

impl Display {
    /// Logical rectangle as `(left, top, right, bottom)`, exclusive on the far edges.
    fn bounds(&self) -> (i64, i64, i64, i64) {
        let (x, y) = (i64::from(self.x), i64::from(self.y));
        (x, y, x + i64::from(self.width), y + i64::from(self.height))
    }

    fn contains(&self, p: (i32, i32)) -> bool {
        let (l, t, r, b) = self.bounds();
        let (px, py) = (i64::from(p.0), i64::from(p.1));
        (l..r).contains(&px) && (t..b).contains(&py)
    }
}

/// Rectangle in image pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

impl Rect {
    fn contains(&self, x: u32, y: u32) -> bool {
        (self.x..self.x + self.w).contains(&x) && (self.y..self.y + self.h).contains(&y)
    }
}

/// The mapping from a screenshot's pixels to global logical points (27 §3.3).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Logical point at image `(0, 0)`: the displays' bounding-box origin.
    pub origin: (i32, i32),
    /// Logical units per image pixel, `>= 1.0` (`1.0` unless `max_edge` bit).
    pub ratio: f64,
    /// Image size in pixels.
    pub size: (u32, u32),
    /// Each display with its rectangle in image pixels.
    pub displays: Vec<(Display, Rect)>,
}

/// Logical offset from the origin → image pixel coordinate along one axis.
fn to_px(logical: i64, ratio: f64) -> u32 {
    let v = (logical as f64 / ratio).round();
    if v <= 0.0 { 0 } else { v as u32 }
}

impl Frame {
    /// Bounding box of `displays` at logical scale, then one uniform downscale
    /// so the long edge is `<= max_edge` (ratio `1.0` when it already fits).
    /// `displays` must be non-empty.
    pub fn compose(displays: &[Display], max_edge: u32) -> Frame {
        let (mut l, mut t, mut r, mut b) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for d in displays {
            let (dl, dt, dr, db) = d.bounds();
            l = l.min(dl);
            t = t.min(dt);
            r = r.max(dr);
            b = b.max(db);
        }
        let (bw, bh) = ((r - l).max(1), (b - t).max(1));
        let long = bw.max(bh);
        let ratio = if long > i64::from(max_edge) {
            long as f64 / f64::from(max_edge.max(1))
        } else {
            1.0
        };
        let size = (to_px(bw, ratio).max(1), to_px(bh, ratio).max(1));
        let displays = displays
            .iter()
            .map(|d| {
                let (dl, dt, dr, db) = d.bounds();
                // Round both edges, not origin + size: adjacent displays stay
                // seamless and nothing overshoots the canvas.
                let (x0, y0) = (to_px(dl - l, ratio), to_px(dt - t, ratio));
                let (x1, y1) = (to_px(dr - l, ratio), to_px(db - t, ratio));
                let rect = Rect {
                    x: x0,
                    y: y0,
                    w: x1.saturating_sub(x0).max(1),
                    h: y1.saturating_sub(y0).max(1),
                };
                (d.clone(), rect)
            })
            .collect();
        // The bounding box is at most i32 range wide because every edge is an i32.
        Frame {
            origin: (l as i32, t as i32),
            ratio,
            size,
            displays,
        }
    }

    /// Image pixel → global logical point (the pixel's centre, clamped into
    /// the display it lands on). `Err` outside the image or in a gutter
    /// between displays.
    pub fn to_point(&self, x: u32, y: u32) -> Result<(i32, i32), String> {
        if x >= self.size.0 || y >= self.size.1 {
            return Err(format!(
                "({x}, {y}) is outside the {}×{} screenshot",
                self.size.0, self.size.1
            ));
        }
        let Some((d, _)) = self.displays.iter().find(|(_, r)| r.contains(x, y)) else {
            return Err(format!(
                "({x}, {y}) is between displays; pick a point inside one"
            ));
        };
        let (l, t, r, b) = d.bounds();
        let px = i64::from(self.origin.0) + ((f64::from(x) + 0.5) * self.ratio).floor() as i64;
        let py = i64::from(self.origin.1) + ((f64::from(y) + 0.5) * self.ratio).floor() as i64;
        Ok((px.clamp(l, r - 1) as i32, py.clamp(t, b - 1) as i32))
    }

    /// Global logical point → image pixel; `None` when no display contains it.
    pub fn from_point(&self, p: (i32, i32)) -> Option<(u32, u32)> {
        let (_, rect) = self.displays.iter().find(|(d, _)| d.contains(p))?;
        let dx = i64::from(p.0) - i64::from(self.origin.0);
        let dy = i64::from(p.1) - i64::from(self.origin.1);
        let px = (dx as f64 / self.ratio).floor().max(0.0) as u32;
        let py = (dy as f64 / self.ratio).floor().max(0.0) as u32;
        Some((
            px.clamp(rect.x, rect.x + rect.w - 1),
            py.clamp(rect.y, rect.y + rect.h - 1),
        ))
    }

    /// The per-display part of the screenshot text (27 §4), e.g.
    /// `display 1 "Built-in Retina Display" main 1512×982 scale 2 → image (0,458)-(1512,1440)`,
    /// one per display joined with ` · `.
    pub fn describe_displays(&self) -> String {
        let mut s = String::new();
        for (i, (d, r)) in self.displays.iter().enumerate() {
            if i > 0 {
                s.push_str(" · ");
            }
            let _ = write!(
                s,
                "display {} \"{}\"{} {}×{} scale {} → image ({},{})-({},{})",
                d.index,
                d.name,
                if d.main { " main" } else { "" },
                d.width,
                d.height,
                d.scale,
                r.x,
                r.y,
                r.x + r.w,
                r.y + r.h
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(index: u32, main: bool, x: i32, y: i32, w: u32, h: u32, scale: f32) -> Display {
        Display {
            index,
            id: index,
            name: format!("D{index}"),
            main,
            x,
            y,
            width: w,
            height: h,
            scale,
        }
    }

    /// Retina main at (0,0), a 2560×1440 to its right raised by 458.
    fn two() -> Vec<Display> {
        vec![
            display(1, true, 0, 0, 1512, 982, 2.0),
            display(2, false, 1512, -458, 2560, 1440, 1.0),
        ]
    }

    #[test]
    fn two_displays_scale_and_map() {
        let f = Frame::compose(&two(), 1568);
        assert_eq!(f.origin, (0, -458));
        assert!(
            (f.ratio - 4072.0 / 1568.0).abs() < 1e-9,
            "ratio {}",
            f.ratio
        );
        assert_eq!(f.size.0, 1568);
        assert!((553..=555).contains(&f.size.1), "height {}", f.size.1);
        let (d2, r2) = &f.displays[1];
        // A pixel inside display 2's image rect lands inside its logical rect.
        for (px, py) in [
            (r2.x, r2.y),
            (r2.x + r2.w - 1, r2.y + r2.h - 1),
            (1000, 300),
        ] {
            let p = f.to_point(px, py).unwrap();
            assert!(d2.contains(p), "({px},{py}) → {p:?} outside display 2");
            let back = f.from_point(p).unwrap();
            assert!(
                back.0.abs_diff(px) <= 1 && back.1.abs_diff(py) <= 1,
                "{back:?} vs ({px},{py})"
            );
        }
        // Display 1 occupies the lower-left; its top-left pixel maps onto its top edge.
        let (d1, r1) = &f.displays[0];
        let p = f.to_point(r1.x, r1.y).unwrap();
        assert!(d1.contains(p) && p.1 == 0, "{p:?}");
    }

    #[test]
    fn gutter_and_outside_errors() {
        let f = Frame::compose(&two(), 1568);
        assert_eq!(
            f.to_point(100, 50).unwrap_err(),
            "(100, 50) is between displays; pick a point inside one"
        );
        let (w, h) = f.size;
        assert_eq!(
            f.to_point(w, 0).unwrap_err(),
            format!("({w}, 0) is outside the {w}×{h} screenshot")
        );
        assert!(f.to_point(0, h).is_err());
        assert_eq!(f.from_point((100, -100)), None);
        assert_eq!(f.from_point((-1, 0)), None);
    }

    #[test]
    fn single_display_is_identity() {
        let f = Frame::compose(&[display(1, true, 0, 0, 1512, 982, 2.0)], 1568);
        assert_eq!(f.ratio, 1.0);
        assert_eq!(f.size, (1512, 982));
        assert_eq!(f.origin, (0, 0));
        assert_eq!(f.to_point(100, 200).unwrap(), (100, 200));
        assert_eq!(f.to_point(1511, 981).unwrap(), (1511, 981));
        assert_eq!(f.from_point((100, 200)), Some((100, 200)));
        assert!(f.to_point(1512, 0).is_err());
    }

    #[test]
    fn three_displays_canvas() {
        let ds = vec![
            display(1, true, 0, 0, 1000, 500, 1.0),
            display(2, false, 1000, 0, 1000, 500, 1.0),
            display(3, false, -1000, 100, 1000, 500, 1.0),
        ];
        let f = Frame::compose(&ds, 4000);
        assert_eq!(f.ratio, 1.0);
        assert_eq!(f.origin, (-1000, 0));
        assert_eq!(f.size, (3000, 600));
        assert_eq!(
            f.displays[2].1,
            Rect {
                x: 0,
                y: 100,
                w: 1000,
                h: 500
            }
        );
        assert_eq!(
            f.displays[1].1,
            Rect {
                x: 2000,
                y: 0,
                w: 1000,
                h: 500
            }
        );
        let f = Frame::compose(&ds, 1500);
        assert_eq!(f.ratio, 2.0);
        assert_eq!(f.size, (1500, 300));
    }

    #[test]
    fn describe() {
        let f = Frame::compose(&two(), 100_000);
        let mut ds = two();
        ds[0].name = "Built-in Retina Display".into();
        ds[1].name = "DELL U2723QE".into();
        ds[1].scale = 1.5;
        let f2 = Frame::compose(&ds, 100_000);
        assert_eq!(f.size, (4072, 1440));
        assert_eq!(
            f2.describe_displays(),
            "display 1 \"Built-in Retina Display\" main 1512×982 scale 2 → image (0,458)-(1512,1440) · \
             display 2 \"DELL U2723QE\" 2560×1440 scale 1.5 → image (1512,0)-(4072,1440)"
        );
    }
}
