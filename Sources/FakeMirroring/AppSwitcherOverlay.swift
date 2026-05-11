// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: App Switcher overlay rendering one card per running AppPack with swipe-up dismissal.
// ABOUTME: A swipe-up gesture on a card invokes registry.forceQuit so reset_before_explore exercises real reset.

import AppKit
import HelperLib

/// State for an active App Switcher overlay.
/// Cards are stacked vertically; each card shows its app's icon + name and
/// supports a swipe-up gesture that force-quits the underlying pack.
@MainActor
final class AppSwitcherOverlay {
    private let registry: AppRegistry
    /// Called when the user dismisses the overlay (swiped down off-screen, or
    /// closed via menu). FakeScreenView clears its `appSwitcherOverlay`.
    let onClose: () -> Void

    /// Card rectangles keyed by pack appName, computed lazily by sceneData().
    /// Used by FakeScreenView's hit handler to map a swipe-up to a force-quit.
    private(set) var cardRects: [(packName: String, rect: CGRect)] = []

    init(registry: AppRegistry, onClose: @escaping () -> Void) {
        self.registry = registry
        self.onClose = onClose
    }

    /// Force-quit the pack underneath the swipe. Returns true if a card was hit.
    func handleSwipeUp(at point: CGPoint) -> Bool {
        for (name, rect) in cardRects where rect.contains(point) {
            if let pack = registry.packs[name.lowercased()] {
                registry.forceQuit(pack)
            }
            return true
        }
        return false
    }

    func close() { onClose() }

    /// Render the running app cards as ScenarioData.
    func sceneData() -> ScenarioData {
        var plainTexts: [(String, CGPoint)] = []
        var placeholders: [CGRect] = []
        cardRects = []

        let cardWidth: CGFloat = 320
        let cardHeight: CGFloat = 220
        let cardX: CGFloat = (410 - cardWidth) / 2
        var cardY: CGFloat = 100

        for name in registry.runningPackIDs {
            guard let pack = registry.packs[name.lowercased()] else { continue }
            let rect = CGRect(x: cardX, y: cardY, width: cardWidth, height: cardHeight)
            placeholders.append(rect)
            plainTexts.append((pack.spec.icon, CGPoint(x: cardX + 20, y: cardY + 20)))
            plainTexts.append((pack.spec.appName, CGPoint(x: cardX + 20, y: cardY + 60)))
            cardRects.append((name, rect))
            cardY += cardHeight + 20
        }
        if registry.runningPackIDs.isEmpty {
            plainTexts.append(("No running apps", CGPoint(x: 100, y: 400)))
        }
        return ScenarioData(
            header: "App Switcher",
            rows: [],
            hasTabBar: false,
            plainTexts: plainTexts,
            buttons: [],
            placeholders: placeholders,
            hasBackChevron: false,
            cards: [],
            sliderTrack: nil,
            textFieldRects: []
        )
    }
}
