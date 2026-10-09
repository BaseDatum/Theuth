#!/usr/bin/env swift
// 149 C: rasterise crates/desktop/companion-assets/harness/*.svg to <name>-<px>.png at 16/20/24/32 px into the
// same directory (NSImage decodes SVG on macOS 11+; the bitmap is drawn at 1× — the companion scales for 2×
// by loading the next size up). Run from anywhere: `swift scripts/render-harness-icons.swift`; commit the PNGs.
import AppKit

let sizes = [16, 20, 24, 32]
let here = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
let dir = here.appendingPathComponent("../crates/desktop/companion-assets/harness").standardizedFileURL
let svgs = try FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)
    .filter { $0.pathExtension == "svg" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
guard !svgs.isEmpty else { fputs("no .svg under \(dir.path)\n", stderr); exit(1) }

var written = 0
for svg in svgs {
    guard let image = NSImage(contentsOf: svg) else { fputs("cannot decode \(svg.lastPathComponent)\n", stderr); exit(1) }
    let name = svg.deletingPathExtension().lastPathComponent
    for px in sizes {
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0) else { fputs("no bitmap for \(name) \(px)\n", stderr); exit(1) }
        rep.size = NSSize(width: px, height: px)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSGraphicsContext.current?.imageInterpolation = .high
        image.draw(in: NSRect(x: 0, y: 0, width: px, height: px), from: .zero, operation: .sourceOver, fraction: 1)
        NSGraphicsContext.restoreGraphicsState()
        guard let png = rep.representation(using: .png, properties: [:]) else { fputs("no png for \(name) \(px)\n", stderr); exit(1) }
        try png.write(to: dir.appendingPathComponent("\(name)-\(px).png"))
        written += 1
    }
}
print("wrote \(written) PNGs (\(svgs.count) glyphs × \(sizes.count) sizes) under \(dir.path)")
