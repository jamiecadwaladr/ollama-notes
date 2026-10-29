// Draws the app icon, a small notes map: the ◎ hub linked to note cards in the map's kind colours.
// Usage: make-icon <dir.iconset>   (build.sh turns the iconset into AppIcon.icns with iconutil)
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat(hex >> 16 & 0xff) / 255, green: CGFloat(hex >> 8 & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
}

/// The icon in a 1024-point square (origin bottom left), on the macOS grid: an 824-point tile with a soft shadow.
func draw(_ ctx: CGContext) {
    let tile = CGPath(roundedRect: CGRect(x: 100, y: 100, width: 824, height: 824), cornerWidth: 186, cornerHeight: 186, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 28, color: color(0, 0.5))
    ctx.addPath(tile)
    ctx.setFillColor(color(0x161923))
    ctx.fillPath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(tile)
    ctx.clip()
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let gradient = CGGradient(colorsSpace: space, colors: [color(0x2a3047), color(0x0f1117)] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(gradient, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
    ctx.setFillColor(color(0x2a2f3f)) // the canvas dot grid
    for x in stride(from: 148, through: 880, by: 61) {
        for y in stride(from: 148, through: 880, by: 61) { ctx.fillEllipse(in: CGRect(x: x - 4, y: y - 4, width: 8, height: 8)) }
    }

    let hub = CGPoint(x: 512, y: 500)
    let cards: [(CGPoint, UInt32)] = [ // concept, fact, decision, question
        (CGPoint(x: 300, y: 730), 0x60a5fa), (CGPoint(x: 735, y: 742), 0x2dd4bf),
        (CGPoint(x: 752, y: 300), 0xfbbf24), (CGPoint(x: 278, y: 282), 0xf472b6),
    ]
    ctx.setLineCap(.round)
    ctx.setStrokeColor(color(0x5b6690))
    ctx.setLineWidth(16)
    for (p, _) in cards {
        ctx.move(to: hub)
        ctx.addLine(to: p)
    }
    ctx.strokePath()
    ctx.setLineDash(phase: 0, lengths: [26, 22]) // a keyword link
    ctx.setLineWidth(10)
    ctx.move(to: cards[0].0)
    ctx.addLine(to: cards[1].0)
    ctx.strokePath()
    ctx.setLineDash(phase: 0, lengths: [])

    for (p, kind) in cards {
        let rect = CGRect(x: p.x - 104, y: p.y - 62, width: 208, height: 124)
        let card = CGPath(roundedRect: rect, cornerWidth: 24, cornerHeight: 24, transform: nil)
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -6), blur: 16, color: color(0, 0.45))
        ctx.addPath(card)
        ctx.setFillColor(color(0x1e2230))
        ctx.fillPath()
        ctx.restoreGState()
        ctx.saveGState()
        ctx.addPath(card)
        ctx.clip()
        ctx.setFillColor(color(kind))
        ctx.fill(CGRect(x: rect.minX, y: rect.minY, width: 18, height: rect.height))
        ctx.restoreGState()
        ctx.addPath(card)
        ctx.setStrokeColor(color(0x343a50))
        ctx.setLineWidth(3)
        ctx.strokePath()
        ctx.setFillColor(color(0xe6e8ef, 0.9)) // title line, then two body lines
        ctx.addPath(CGPath(roundedRect: CGRect(x: rect.minX + 42, y: rect.maxY - 46, width: 120, height: 18), cornerWidth: 9, cornerHeight: 9, transform: nil))
        ctx.fillPath()
        ctx.setFillColor(color(0x8b91a5, 0.8))
        for (dy, w) in [(80.0, 136.0), (106.0, 96.0)] {
            ctx.addPath(CGPath(roundedRect: CGRect(x: rect.minX + 42, y: rect.maxY - dy, width: w, height: 12), cornerWidth: 6, cornerHeight: 6, transform: nil))
        }
        ctx.fillPath()
    }

    // the hub: ◎ in the accent colour, with a glow
    ctx.setFillColor(color(0x0f1117))
    ctx.fillEllipse(in: CGRect(x: hub.x - 132, y: hub.y - 132, width: 264, height: 264))
    ctx.setShadow(offset: .zero, blur: 40, color: color(0x7c8cff, 0.75))
    ctx.setStrokeColor(color(0x7c8cff))
    ctx.setLineWidth(34)
    ctx.strokeEllipse(in: CGRect(x: hub.x - 104, y: hub.y - 104, width: 208, height: 208))
    ctx.setFillColor(color(0x9aa6ff))
    ctx.fillEllipse(in: CGRect(x: hub.x - 48, y: hub.y - 48, width: 96, height: 96))
    ctx.restoreGState()
}

let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "AppIcon.iconset")
try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let px = size * scale
        let ctx = CGContext(data: nil, width: px, height: px, bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        ctx.scaleBy(x: CGFloat(px) / 1024, y: CGFloat(px) / 1024)
        draw(ctx)
        let name = "icon_\(size)x\(size)\(scale == 2 ? "@2x" : "").png"
        let dest = CGImageDestinationCreateWithURL(out.appendingPathComponent(name) as CFURL, UTType.png.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(dest, ctx.makeImage()!, nil)
        CGImageDestinationFinalize(dest)
    }
}
