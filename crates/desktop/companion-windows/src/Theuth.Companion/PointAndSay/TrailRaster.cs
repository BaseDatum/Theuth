namespace Theuth.Companion.PointAndSay;

/// <summary>
/// The overlay's pixels (149 §3.7 / D13, the look of <c>UI/routes/overlay.tsx</c>): a premultiplied BGRA surface the size of
/// one monitor in physical pixels. The trail is a 6 pt stroke with round caps and joins at alpha 0.85 that persists while the
/// key is held (a circle stays a circle); the ring is 14 pt across, 2 pt wide, redrawn at the newest point each frame — at
/// 0.85 while the pen is down and 0.4 while it is up in <c>mouse</c> mode. Rasterised here with a signed-distance coverage
/// (no GDI+/Direct2D dependency; the trail is a few hundred short segments at most), composed into <see cref="Frame"/>.
/// </summary>
internal sealed class TrailRaster
{
    public const double StrokePt = 6;
    public const double RingPt = 14;
    public const double RingWidthPt = 2;
    public const double TrailAlpha = 0.85;
    public const double RingUpAlpha = 0.4;

    private readonly byte[] trail;
    private byte r, g, b;
    private readonly double scale;
    private (double X, double Y)? last;
    private bool down;
    private bool dirty;

    public TrailRaster(int width, int height, double scale, uint rgb)
    {
        Width = Math.Max(1, width);
        Height = Math.Max(1, height);
        this.scale = scale <= 0 ? 1 : scale;
        trail = new byte[Width * Height * 4];
        Frame = new byte[Width * Height * 4];
        r = (byte)(rgb >> 16);
        g = (byte)(rgb >> 8);
        b = (byte)rgb;
    }

    /// <summary>149 NATIVE: the trail colour is the system accent — re-set when Windows' accent changes (takes effect from the next press).</summary>
    public void SetColor(uint rgb)
    {
        r = (byte)(rgb >> 16);
        g = (byte)(rgb >> 8);
        b = (byte)rgb;
        dirty = true;
    }

    public int Width { get; }

    public int Height { get; }

    /// <summary>The composed surface (trail + ring), premultiplied BGRA, top-down rows.</summary>
    public byte[] Frame { get; }

    /// <summary>Something changed since the last <see cref="Compose"/>.</summary>
    public bool Dirty => dirty;

    public void Clear()
    {
        Array.Clear(trail);
        Array.Clear(Frame);
        last = null;
        down = false;
        dirty = true;
    }

    /// <summary>One sample in this monitor's local physical pixels; <paramref name="pen"/> false breaks the polyline (ring only).</summary>
    public void Append(double x, double y, bool pen)
    {
        if (pen)
        {
            if (down && last is { } p) Segment(p.X, p.Y, x, y);
            else Disc(x, y);
            down = true;
        }
        else
        {
            down = false;
        }
        last = (x, y);
        dirty = true;
    }

    /// <summary>Trail + ring into <see cref="Frame"/>.</summary>
    public void Compose()
    {
        Buffer.BlockCopy(trail, 0, Frame, 0, trail.Length);
        if (last is { } p) Ring(p.X, p.Y, down ? TrailAlpha : RingUpAlpha);
        dirty = false;
    }

    private double StrokeRadius => StrokePt * scale / 2;

    private void Disc(double cx, double cy)
    {
        double rad = StrokeRadius;
        Paint(trail, cx - rad - 1, cy - rad - 1, cx + rad + 1, cy + rad + 1, TrailAlpha, (px, py) =>
        {
            double d = Math.Sqrt((px - cx) * (px - cx) + (py - cy) * (py - cy));
            return Math.Clamp(rad + 0.5 - d, 0, 1);
        });
    }

    private void Segment(double x0, double y0, double x1, double y1)
    {
        double rad = StrokeRadius;
        double dx = x1 - x0, dy = y1 - y0;
        double len2 = dx * dx + dy * dy;
        Paint(trail, Math.Min(x0, x1) - rad - 1, Math.Min(y0, y1) - rad - 1, Math.Max(x0, x1) + rad + 1, Math.Max(y0, y1) + rad + 1, TrailAlpha, (px, py) =>
        {
            double t = len2 <= 1e-9 ? 0 : Math.Clamp(((px - x0) * dx + (py - y0) * dy) / len2, 0, 1);
            double qx = x0 + t * dx, qy = y0 + t * dy;
            double d = Math.Sqrt((px - qx) * (px - qx) + (py - qy) * (py - qy));
            return Math.Clamp(rad + 0.5 - d, 0, 1);
        });
    }

    private void Ring(double cx, double cy, double alpha)
    {
        double rad = RingPt * scale / 2;
        double half = RingWidthPt * scale / 2;
        Paint(Frame, cx - rad - half - 1, cy - rad - half - 1, cx + rad + half + 1, cy + rad + half + 1, alpha, (px, py) =>
        {
            double d = Math.Sqrt((px - cx) * (px - cx) + (py - cy) * (py - cy));
            return Math.Clamp(half + 0.5 - Math.Abs(d - rad), 0, 1);
        });
    }

    /// <summary>Max-alpha union so overlapping caps never darken; pixel centres sampled at +0.5.</summary>
    private void Paint(byte[] dst, double x0, double y0, double x1, double y1, double alpha, Func<double, double, double> coverage)
    {
        int ix0 = Math.Max(0, (int)Math.Floor(x0)), iy0 = Math.Max(0, (int)Math.Floor(y0));
        int ix1 = Math.Min(Width - 1, (int)Math.Ceiling(x1)), iy1 = Math.Min(Height - 1, (int)Math.Ceiling(y1));
        for (int y = iy0; y <= iy1; y++)
        {
            int row = y * Width * 4;
            for (int x = ix0; x <= ix1; x++)
            {
                double c = coverage(x + 0.5, y + 0.5) * alpha;
                if (c <= 0) continue;
                int i = row + x * 4;
                double a0 = dst[i + 3] / 255.0;
                if (c <= a0) continue; // an already fuller pixel stays
                dst[i] = (byte)Math.Round(b * c);
                dst[i + 1] = (byte)Math.Round(g * c);
                dst[i + 2] = (byte)Math.Round(r * c);
                dst[i + 3] = (byte)Math.Round(255 * c);
            }
        }
    }
}