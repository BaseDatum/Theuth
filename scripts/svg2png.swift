// Rasterize an SVG to a transparent PNG with AppKit (CoreSVG); no third-party tooling.
// Usage: swift scripts/svg2png.swift <in.svg> <size-px> <out.png>
import AppKit

let args = CommandLine.arguments
guard args.count == 4, let size = Int(args[2]), let svg = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write(Data("usage: svg2png.swift <in.svg> <size-px> <out.png>\n".utf8))
    exit(2)
}
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8, samplesPerPixel: 4,
    hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = NSSize(width: size, height: size)
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
svg.draw(in: NSRect(x: 0, y: 0, width: size, height: size), from: .zero, operation: .copy, fraction: 1)
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[3]))
