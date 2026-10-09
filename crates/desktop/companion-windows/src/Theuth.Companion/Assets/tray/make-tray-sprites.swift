// Design 134 W2 — renders the tray sprites (theuth's glyph from crates/desktop/icons/tray.svg + the count badge) into
// multi-size .ico files: tray-<dark|light>-<0…9|9plus>.ico, PNG entries at 16/20/24/32/40/48 px. "dark" = a dark taskbar
// (white glyph), "light" = a light taskbar (black glyph). 149 NATIVE: monochrome like Windows 11's own tray icons — the count
// badge is the glyph colour's disc with the taskbar colour's digits (white disc · black digits on a dark taskbar, and the inverse);
// no palette colour (a pre-rendered PNG cannot follow the system accent).
// Run on the Mac (CoreGraphics + CoreText + ImageIO, Command Line Tools are enough):
//   swift make-tray-sprites.swift <out dir>
import CoreGraphics
import CoreText
import Foundation
import ImageIO
import UniformTypeIdentifiers

let outDir = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ".")
let sizes = [16, 20, 24, 32, 40, 48]
let srgb = CGColorSpace(name: CGColorSpace.sRGB)!

func color(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: srgb, components: [CGFloat((hex >> 16) & 0xff) / 255, CGFloat((hex >> 8) & 0xff) / 255, CGFloat(hex & 0xff) / 255, a])!
}

/// tray.svg in its own units (viewBox 60 60 904 904, y down): the head + beak, the eye is cut out separately.
func glyph() -> CGPath {
    let p = CGMutablePath()
    p.addEllipse(in: CGRect(x: 640 - 165, y: 340 - 165, width: 330, height: 330))
    p.move(to: CGPoint(x: 660, y: 178))
    p.addCurve(to: CGPoint(x: 186, y: 846), control1: CGPoint(x: 400, y: 196), control2: CGPoint(x: 230, y: 470))
    p.addLine(to: CGPoint(x: 244, y: 890))
    p.addCurve(to: CGPoint(x: 600, y: 500), control1: CGPoint(x: 300, y: 570), control2: CGPoint(x: 400, y: 380))
    p.closeSubpath()
    return p
}

func render(size s: Int, label: String?, lightTaskbar: Bool) -> Data {
    let ctx = CGContext(data: nil, width: s, height: s, bitsPerComponent: 8, bytesPerRow: 0, space: srgb,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    let S = CGFloat(s)
    ctx.setAllowsAntialiasing(true)
    ctx.setShouldAntialias(true)
    // The glyph, SVG coordinates (y down) → pixels.
    ctx.saveGState()
    ctx.translateBy(x: 0, y: S)
    ctx.scaleBy(x: 1, y: -1)
    let k = S / 904
    ctx.scaleBy(x: k, y: k)
    ctx.translateBy(x: -60, y: -60)
    ctx.setFillColor(lightTaskbar ? color(0x000000) : color(0xffffff))
    ctx.addPath(glyph())
    ctx.fillPath()
    ctx.setBlendMode(.clear)
    ctx.fillEllipse(in: CGRect(x: 610 - 36, y: 288 - 36, width: 72, height: 72))
    ctx.restoreGState()

    if let label {
        // The badge: bottom-right, in CG's own y-up space; a clear ring first so the glyph never touches it.
        let d = (S * 0.64).rounded()
        let ring = max(1, (S / 16).rounded())
        let c = CGPoint(x: S - d / 2, y: d / 2)
        ctx.setBlendMode(.clear)
        ctx.fillEllipse(in: CGRect(x: c.x - d / 2 - ring, y: c.y - d / 2 - ring, width: d + 2 * ring, height: d + 2 * ring))
        ctx.setBlendMode(.normal)
        ctx.setFillColor(lightTaskbar ? color(0x000000) : color(0xffffff))
        ctx.fillEllipse(in: CGRect(x: c.x - d / 2, y: c.y - d / 2, width: d, height: d))
        let fontSize = d * (label.count > 1 ? 0.56 : 0.78)
        let font = CTFontCreateUIFontForLanguage(.emphasizedSystem, fontSize, nil)!
        let attrs: [NSAttributedString.Key: Any] = [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): lightTaskbar ? color(0xffffff) : color(0x000000),
        ]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: label, attributes: attrs))
        let b = CTLineGetImageBounds(line, ctx)
        ctx.textPosition = CGPoint(x: c.x - b.midX, y: c.y - b.midY)
        CTLineDraw(line, ctx)
    }

    let image = ctx.makeImage()!
    let data = NSMutableData()
    let dest = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, image, nil)
    CGImageDestinationFinalize(dest)
    return data as Data
}

func le16(_ v: Int) -> Data { withUnsafeBytes(of: UInt16(v).littleEndian) { Data($0) } }
func le32(_ v: Int) -> Data { withUnsafeBytes(of: UInt32(v).littleEndian) { Data($0) } }

/// ICONDIR + one ICONDIRENTRY per PNG (PNG-compressed entries, Vista+), then the PNGs.
func ico(_ entries: [(Int, Data)]) -> Data {
    var out = Data()
    out += le16(0) + le16(1) + le16(entries.count)
    var offset = 6 + 16 * entries.count
    for (s, png) in entries {
        out.append(UInt8(s >= 256 ? 0 : s))
        out.append(UInt8(s >= 256 ? 0 : s))
        out.append(0) // colours
        out.append(0) // reserved
        out += le16(1) + le16(32) + le32(png.count) + le32(offset)
        offset += png.count
    }
    for (_, png) in entries { out += png }
    return out
}

try FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)
for light in [false, true] {
    for n in 0...10 {
        let label: String? = n == 0 ? nil : (n == 10 ? "9+" : "\(n)")
        let name = "tray-\(light ? "light" : "dark")-\(n == 10 ? "9plus" : "\(n)").ico"
        let data = ico(sizes.map { ($0, render(size: $0, label: label, lightTaskbar: light)) })
        try data.write(to: outDir.appendingPathComponent(name))
        print("\(name) \(data.count) bytes")
    }
}