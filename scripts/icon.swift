// icon.swift SOURCE.png OUT.png: the app icon from a square render. Crops
// the middle of SOURCE, scales it into the macOS icon shape (an 824-point
// rounded square on a 1024 canvas, as Apple's icon grid has it) and adds
// a faint edge so the black tile reads on dark backgrounds.
import AppKit

let args = CommandLine.arguments
guard args.count == 3, let source = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write("usage: icon.swift SOURCE.png OUT.png\n".data(using: .utf8)!)
    exit(2)
}
let canvas = 1024.0, tile = 824.0, radius = 185.4
let inset = (canvas - tile) / 2
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(canvas), pixelsHigh: Int(canvas),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let shape = NSBezierPath(
    roundedRect: NSRect(x: inset, y: inset, width: tile, height: tile),
    xRadius: radius, yRadius: radius)
NSColor.black.setFill()
shape.fill()
shape.addClip()
// The middle 70% of the render: the strand nearly fills the tile.
let side = min(source.size.width, source.size.height) * 0.7
let from = NSRect(
    x: (source.size.width - side) / 2, y: (source.size.height - side) / 2,
    width: side, height: side)
source.draw(in: NSRect(x: inset, y: inset, width: tile, height: tile), from: from,
            operation: .sourceOver, fraction: 1.0)
NSColor(white: 1.0, alpha: 0.14).setStroke()
shape.lineWidth = 3
shape.stroke()
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
