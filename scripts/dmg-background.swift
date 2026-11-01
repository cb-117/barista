// Draws the installer window background for the Barista disk image.
// Usage: swift scripts/dmg-background.swift OUTPUT.tiff
// Layout must match scripts/dmg-settings.py (660×420 window, icons at x 170 and 490, y 190).
import AppKit

let size = NSSize(width: 660, height: 420)
let foam = NSColor(srgbRed: 1.0, green: 0.96, blue: 0.87, alpha: 1)
let card = NSColor(srgbRed: 1.0, green: 0.98, blue: 0.94, alpha: 1)
let latte = NSColor(srgbRed: 0.91, green: 0.83, blue: 0.71, alpha: 1)
let crema = NSColor(srgbRed: 0.78, green: 0.35, blue: 0.17, alpha: 1)
let ink = NSColor(srgbRed: 0.17, green: 0.10, blue: 0.07, alpha: 1)
let muted = NSColor(srgbRed: 0.48, green: 0.35, blue: 0.28, alpha: 1)

func rounded(_ size: CGFloat, _ weight: NSFont.Weight) -> NSFont {
    let base = NSFont.systemFont(ofSize: size, weight: weight)
    return base.fontDescriptor.withDesign(.rounded).flatMap { NSFont(descriptor: $0, size: size) } ?? base
}

func draw(_ text: String, font: NSFont, color: NSColor, in rect: NSRect, alignment: NSTextAlignment = .center) {
    let style = NSMutableParagraphStyle()
    style.alignment = alignment
    style.lineSpacing = 2
    (text as NSString).draw(in: rect, withAttributes: [.font: font, .foregroundColor: color, .paragraphStyle: style])
}

func render(scale: CGFloat) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale),
                               bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = size
    NSGraphicsContext.saveGraphicsState()
    let context = NSGraphicsContext(bitmapImageRep: rep)!
    NSGraphicsContext.current = context
    // Top-left origin, like Finder's icon positions.
    context.cgContext.translateBy(x: 0, y: size.height)
    context.cgContext.scaleBy(x: 1, y: -1)
    let flipped = NSGraphicsContext(cgContext: context.cgContext, flipped: true)
    NSGraphicsContext.current = flipped

    foam.setFill()
    NSRect(origin: .zero, size: size).fill()

    draw("Drag Barista into Applications", font: rounded(22, .bold), color: ink, in: NSRect(x: 0, y: 34, width: size.width, height: 32))

    // A gentle arc from the app icon to the Applications folder.
    let arrow = NSBezierPath()
    arrow.move(to: NSPoint(x: 262, y: 186))
    arrow.curve(to: NSPoint(x: 392, y: 186), controlPoint1: NSPoint(x: 300, y: 160), controlPoint2: NSPoint(x: 354, y: 160))
    arrow.lineWidth = 5
    arrow.lineCapStyle = .round
    crema.setStroke()
    arrow.stroke()
    let head = NSBezierPath()
    head.move(to: NSPoint(x: 377, y: 168))
    head.line(to: NSPoint(x: 396, y: 189))
    head.line(to: NSPoint(x: 369, y: 193))
    head.lineWidth = 5
    head.lineCapStyle = .round
    head.lineJoinStyle = .round
    head.stroke()

    // First-launch help until the app is notarized.
    let box = NSRect(x: 40, y: 316, width: 580, height: 74)
    let panel = NSBezierPath(roundedRect: box, xRadius: 14, yRadius: 14)
    card.setFill(); panel.fill()
    latte.setStroke(); panel.lineWidth = 1.5; panel.stroke()
    draw("First time opening it?", font: rounded(13, .bold), color: ink, in: box.insetBy(dx: 18, dy: 12).offsetBy(dx: 0, dy: 0), alignment: .left)
    draw("If macOS says it can’t verify Barista, click Done. Then open System Settings → Privacy & Security, scroll down, and click Open Anyway.",
         font: rounded(12, .regular), color: muted,
         in: NSRect(x: box.minX + 18, y: box.minY + 31, width: box.width - 36, height: 40), alignment: .left)

    NSGraphicsContext.restoreGraphicsState()
    return rep
}

guard CommandLine.arguments.count == 2 else {
    FileHandle.standardError.write(Data("Usage: swift scripts/dmg-background.swift OUTPUT.tiff\n".utf8))
    exit(2)
}
let image = NSImage(size: size)
image.addRepresentation(render(scale: 1))
image.addRepresentation(render(scale: 2))
guard let tiff = image.tiffRepresentation(using: .lzw, factor: 0) else { exit(1) }
try tiff.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
print("Wrote \(CommandLine.arguments[1])")
