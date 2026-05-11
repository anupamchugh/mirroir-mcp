// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Detects whether the iPhone is already showing the target app to skip redundant Spotlight launches.
// ABOUTME: Uses the persisted graph root node's OCR text as a fingerprint and matches via Jaccard similarity.

import Foundation
import HelperLib

/// Decides whether `launchApp` (Spotlight dance) is needed before exploration.
/// Skipping it when the user is already in the target app saves ~3s and avoids
/// a context flicker that interrupts whatever they were testing.
///
/// Detection strategy: compare current OCR element text against the persisted
/// graph root's element text. A high-similarity match implies we're already
/// foreground in the same root screen the explorer would land on after launch.
enum AppForegroundDetector {

    /// Minimum Jaccard similarity on text-only OCR elements to declare a match.
    /// Tuned conservatively — false positives skip a needed launch and break
    /// exploration; false negatives just keep the existing (slower) behavior.
    static let matchThreshold: Double = 0.5

    /// Outcome of a foreground check.
    enum Verdict: Equatable, Sendable {
        /// Current screen matches the persisted root → caller should skip launchApp.
        case alreadyForeground(similarity: Double)
        /// Spotlight overlay is visible → caller must launch normally.
        case spotlightVisible
        /// No graph file for this app yet → caller has no fingerprint to match against.
        case noPersistedGraph
        /// Graph exists but current screen doesn't resemble the root.
        case unmatched(similarity: Double)
    }

    /// Run detection against the persisted graph for `appName`.
    ///
    /// Matches the current screen against ANY node in the graph, not just the
    /// root: if the prior exploration left iPhone on a sub-screen, that screen
    /// is still inside the target app and a relaunch wastes time.
    ///
    /// - Parameters:
    ///   - elements: OCR elements from the current screen.
    ///   - appName: The app being explored. Used as the graph filename via `GraphPersistence`.
    /// - Returns: A `Verdict` the caller can dispatch on.
    static func detect(elements: [TapPoint], appName: String) -> Verdict {
        if SpotlightDetector.isSpotlightVisible(elements: elements) {
            return .spotlightVisible
        }
        guard let snapshot = GraphPersistence.load(bundleID: appName), !snapshot.nodes.isEmpty else {
            return .noPersistedGraph
        }
        var bestSimilarity = 0.0
        for node in snapshot.nodes.values {
            let similarity = jaccardSimilarity(current: elements, root: node.elements)
            if similarity > bestSimilarity {
                bestSimilarity = similarity
            }
        }
        if bestSimilarity >= matchThreshold {
            return .alreadyForeground(similarity: bestSimilarity)
        }
        return .unmatched(similarity: bestSimilarity)
    }

    /// Jaccard similarity between two element sets, comparing normalized text.
    /// "icon" placeholders and empty strings are excluded — only meaningful labels count.
    static func jaccardSimilarity(current: [TapPoint], root: [TapPoint]) -> Double {
        let currentSet = textSet(from: current)
        let rootSet = textSet(from: root)
        let union = currentSet.union(rootSet)
        guard !union.isEmpty else { return 0.0 }
        let intersection = currentSet.intersection(rootSet)
        return Double(intersection.count) / Double(union.count)
    }

    private static func textSet(from points: [TapPoint]) -> Set<String> {
        var set: Set<String> = []
        for p in points {
            let normalized = p.text
                .lowercased()
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !normalized.isEmpty, normalized != "icon" else { continue }
            set.insert(normalized)
        }
        return set
    }
}
