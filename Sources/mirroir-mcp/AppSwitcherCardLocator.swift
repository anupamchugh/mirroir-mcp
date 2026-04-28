// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Locates a specific app's card in iPhone Mirroring's App Switcher by matching its preview text against the foreground app's OCR.
// ABOUTME: Eliminates the brittleness of hard-coded `appSwitcherCardXFraction` whose correct value depends on how many apps are in the switcher.

import Foundation
import HelperLib

/// AppSwitcherCardLocator — given OCR text from an app's foreground state and
/// OCR text from the App Switcher view, find the X coordinate of the card
/// whose preview shows that app.
///
/// iPhone Mirroring's App Switcher renders each running app as a scaled-down
/// preview. Text that appears in the foreground app also appears in the
/// preview (smaller, sometimes partially clipped). By intersecting the
/// foreground text set with the App Switcher OCR and clustering the matches
/// by X bucket, the cluster with the most matches identifies the card
/// belonging to the just-launched app — regardless of where iPhone Mirroring
/// chooses to position it within the carousel.
enum AppSwitcherCardLocator {

    /// Width of each X bucket (points). Larger buckets tolerate more OCR
    /// jitter; smaller buckets discriminate adjacent cards more sharply.
    /// 100pt is a good compromise on a 410-wide window: with three visible
    /// cards, each card occupies roughly one bucket.
    static let bucketWidthPt: Double = 100.0

    /// Minimum text length to use for matching. Single characters and short
    /// fragments produce too many false matches.
    static let minTextLength: Int = 3

    /// Locate the X coordinate of the just-launched app's card in App
    /// Switcher OCR by matching against the app's foreground OCR text.
    ///
    /// - Parameters:
    ///   - appElements: OCR text captured while the app was the foreground app.
    ///   - switcherElements: OCR text captured after opening App Switcher.
    /// - Returns: The median X coordinate of the matched cluster, or nil
    ///   when no clear match is found (caller should fall back to a default).
    static func locateCardX(
        appElements: [TapPoint],
        switcherElements: [TapPoint]
    ) -> Double? {
        let appTexts = Set(
            appElements
                .map { normalize($0.text) }
                .filter { $0.count >= minTextLength }
        )
        guard !appTexts.isEmpty else { return nil }

        let matches = switcherElements.filter { el in
            let key = normalize(el.text)
            return key.count >= minTextLength && appTexts.contains(key)
        }
        guard !matches.isEmpty else { return nil }

        var bucketCounts: [Int: Int] = [:]
        for m in matches {
            let bucket = Int(m.tapX / bucketWidthPt)
            bucketCounts[bucket, default: 0] += 1
        }
        guard let bestBucket = bucketCounts.max(by: { $0.value < $1.value })?.key else {
            return nil
        }

        let inBucket = matches.filter { Int($0.tapX / bucketWidthPt) == bestBucket }
        let xs = inBucket.map { $0.tapX }.sorted()
        return xs[xs.count / 2]
    }

    private static func normalize(_ text: String) -> String {
        text.lowercased().trimmingCharacters(in: .whitespaces)
    }
}
