// ABOUTME: Upscales small CGImages to a minimum width for improved OCR accuracy.
// ABOUTME: At small zoom modes the screenshot has too few pixels for Apple Vision to resolve all text.

import CoreGraphics
import HelperLib

/// Upscales screenshots that are below a minimum pixel width so Apple Vision's
/// text recognizer has enough resolution to detect all labels reliably.
enum ImageUpscaler {

    /// Upscale `image` to at least `minWidth` pixels wide, preserving aspect ratio.
    /// Returns the original image unchanged if it is already wide enough.
    static func upscaleIfNeeded(image: CGImage, minWidth: Int) -> CGImage {
        guard image.width < minWidth, image.width > 0, image.height > 0 else {
            return image
        }

        let aspectRatio = Double(image.height) / Double(image.width)
        let targetWidth = minWidth
        let targetHeight = Int(Double(targetWidth) * aspectRatio)

        let colorSpace = CGColorSpaceCreateDeviceRGB()
        guard let context = CGContext(
            data: nil,
            width: targetWidth,
            height: targetHeight,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else {
            return image
        }

        context.interpolationQuality = .high
        context.draw(image, in: CGRect(x: 0, y: 0, width: targetWidth, height: targetHeight))

        guard let upscaled = context.makeImage() else {
            return image
        }

        DebugLog.log("OCR", "Upscaled \(image.width)x\(image.height) -> \(targetWidth)x\(targetHeight) (minWidth=\(minWidth))")
        return upscaled
    }
}
