// Rewrite PNGs without an alpha channel, composited on black. App Store
// Connect rejects an iOS AppIcon set whose PNGs carry alpha, and
// `cargo tauri icon --ios-color` keeps the channel even when it fills it.
// Usage: swift scripts/flatten-png.swift <file.png>...
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

for path in CommandLine.arguments.dropFirst() {
    let url = URL(fileURLWithPath: path)
    guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
          let img = CGImageSourceCreateImageAtIndex(src, 0, nil),
          let ctx = CGContext(data: nil, width: img.width, height: img.height, bitsPerComponent: 8, bytesPerRow: 0,
                              space: CGColorSpace(name: CGColorSpace.sRGB)!,
                              bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
    else { fputs("flatten-png: cannot read \(path)\n", stderr); exit(1) }
    let rect = CGRect(x: 0, y: 0, width: img.width, height: img.height)
    ctx.setFillColor(CGColor(red: 0, green: 0, blue: 0, alpha: 1))
    ctx.fill(rect)
    ctx.draw(img, in: rect)
    guard let flat = ctx.makeImage(),
          let dst = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
    else { fputs("flatten-png: cannot write \(path)\n", stderr); exit(1) }
    CGImageDestinationAddImage(dst, flat, nil)
    guard CGImageDestinationFinalize(dst) else { fputs("flatten-png: finalize failed \(path)\n", stderr); exit(1) }
}
