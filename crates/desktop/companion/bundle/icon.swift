// Draws the companion's 1024² app icon: a gold wordmark glyph on soot (the rail's wordmark, enlarged).
// Run with the `swift` interpreter: `swift icon.swift out.png`. No Xcode, no asset catalog.
import AppKit

let out = CommandLine.arguments.dropFirst().first ?? "icon-1024.png"
let size = 1024.0
let image = NSImage(size: NSSize(width: size, height: size))
image.lockFocus()
let soot = NSColor(srgbRed: 0x12 / 255, green: 0x10 / 255, blue: 0x0e / 255, alpha: 1)
let gold = NSColor(srgbRed: 0xc4 / 255, green: 0x7a / 255, blue: 0x2c / 255, alpha: 1)
// macOS icon grid: the squircle fills ~ 80 % of the canvas.
let inset = size * 0.1
NSBezierPath(roundedRect: NSRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset),
             xRadius: size * 0.18, yRadius: size * 0.18).addClip()
soot.setFill()
NSRect(x: 0, y: 0, width: size, height: size).fill()
let g = size * 0.26
gold.setFill()
NSBezierPath(roundedRect: NSRect(x: (size - g) / 2, y: (size - g) / 2, width: g, height: g), xRadius: g * 0.22, yRadius: g * 0.22).fill()
soot.setFill()
let hole = g * 0.46
NSBezierPath(roundedRect: NSRect(x: (size - hole) / 2, y: (size - hole) / 2, width: hole, height: hole), xRadius: hole * 0.1, yRadius: hole * 0.1).fill()
image.unlockFocus()

guard let tiff = image.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
      let png = rep.representation(using: .png, properties: [:]) else { exit(1) }
try png.write(to: URL(fileURLWithPath: out))
