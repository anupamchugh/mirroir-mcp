// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Tests for APP.md tab-target injection and geometric tab-anchor synthesis.
// ABOUTME: Verifies icon-only tab bars get evenly-spaced synthesized anchors via buildScreenPlan.

import CoreGraphics
import XCTest
@testable import HelperLib
@testable import mirroir_mcp

final class BFSExplorerTabInjectionTests: XCTestCase {

    private let instagramTabs = ["Accueil", "Recherche", "Reels", "Boutique", "Profil"]

    /// Five noisy bottom-band icons resembling the real Instagram feed: the Home
    /// tab (x≈41) is missing and two small blobs are spurious — exactly the
    /// detection noise that geometric synthesis must ignore.
    private let noisyFeedIcons: [IconDetector.DetectedIcon] = [
        IconDetector.DetectedIcon(tapX: 142, tapY: 845, estimatedSize: 24),
        IconDetector.DetectedIcon(tapX: 169, tapY: 869, estimatedSize: 8),
        IconDetector.DetectedIcon(tapX: 206, tapY: 844, estimatedSize: 30),
        IconDetector.DetectedIcon(tapX: 233, tapY: 882, estimatedSize: 12),
        IconDetector.DetectedIcon(tapX: 343, tapY: 846, estimatedSize: 27),
    ]

    private func makeAppDescription(
        tabs: [String],
        tabLayout: TabLayout? = TabLayout(orientation: .horizontal, edge: .bottom)
    ) -> AppDescription {
        AppDescription(
            appName: "TestApp", schemaVersion: 1, locale: "fr_CA", archetype: nil,
            resetBeforeExplore: false, obstacleMode: .auto, context: "test",
            obstacles: [], skipElements: [], credentials: [:], hints: [],
            tabs: tabs, tabLayout: tabLayout, simulator: nil
        )
    }

    /// BFSExplorer with the default 410×890 window and no component definitions
    /// (legacy plan path), seeded with an APP.md describing `tabs`.
    private func makeExplorer(tabs: [String]) -> BFSExplorer {
        let session = ExplorationSession()
        session.start(appName: "TestApp", goal: "test")
        session.setAppDescription(makeAppDescription(tabs: tabs))
        let explorer = BFSExplorer(session: session, budget: .default)
        explorer.markStarted()
        return explorer
    }

    private func nav(_ text: String, x: Double, y: Double) -> ClassifiedElement {
        ClassifiedElement(
            point: TapPoint(text: text, tapX: x, tapY: y, confidence: 0.9),
            role: .navigation)
    }

    // MARK: - Geometric synthesis (icon-only bar)

    func testSynthesizesEvenlySpacedTabAnchorsForIconOnlyBar() {
        let explorer = makeExplorer(tabs: instagramTabs)

        let plan = explorer.buildScreenPlan(
            classified: [], visitedElements: [], icons: noisyFeedIcons)

        // All five tabs injected at the front, in declared order.
        let tabItems = Array(plan.prefix(instagramTabs.count))
        XCTAssertEqual(tabItems.map { $0.displayLabel }, instagramTabs)
        XCTAssertTrue(tabItems.allSatisfy { $0.isBreadthNavigation })
        XCTAssertTrue(tabItems.allSatisfy { $0.reason.contains("tab-synth") })

        // Anchors are evenly spaced across the 410-pt width: (i+0.5)*410/5.
        let expectedX: [Double] = [41, 123, 205, 287, 369]
        for (item, expected) in zip(tabItems, expectedX) {
            XCTAssertEqual(item.point.tapX, expected, accuracy: 0.5,
                "\(item.displayLabel) should be geometrically centered, ignoring noisy icons")
        }
        // Cross-axis Y is the median of detected band-icon Ys (846).
        XCTAssertTrue(tabItems.allSatisfy { abs($0.point.tapY - 846) < 1.0 })
    }

    func testSingleBandIconIsEnoughEvidenceToSynthesize() {
        // Baseline evidence gate is 1: a single detected band icon is enough.
        let explorer = makeExplorer(tabs: instagramTabs)
        let oneIcon = [IconDetector.DetectedIcon(tapX: 206, tapY: 850, estimatedSize: 30)]

        let plan = explorer.buildScreenPlan(
            classified: [], visitedElements: [], icons: oneIcon)

        let synthesized = plan.filter { $0.reason.contains("tab-synth") }
        XCTAssertEqual(synthesized.map { $0.displayLabel }, instagramTabs)
    }

    func testBarPositionFromIconsIgnoresBandTextContent() {
        // The component path supplies full-page text that can land in the bottom
        // band. The bar's cross-axis Y must come from the text-less icons, not be
        // dragged toward that content (median of icons = 846; with the two content
        // rows mixed in, the raw median would be 869).
        let explorer = makeExplorer(tabs: instagramTabs)
        let bandContent = [
            nav("Sponsored A", x: 150, y: 890),
            nav("Sponsored B", x: 260, y: 895),
        ]

        let plan = explorer.buildScreenPlan(
            classified: bandContent, visitedElements: [], icons: noisyFeedIcons)

        let tabItems = plan.filter { $0.reason.contains("tab-synth") }
        XCTAssertEqual(tabItems.count, instagramTabs.count)
        XCTAssertTrue(tabItems.allSatisfy { abs($0.point.tapY - 846) < 1.0 },
            "bar Y must be the icon median (846), not skewed by band text content")
    }

    // MARK: - Presence gate

    func testSkipsSynthesisWhenNoBandEvidence() {
        // Content only above the tab band and no icons → no tab bar → no anchors.
        let explorer = makeExplorer(tabs: instagramTabs)
        let content = [nav("Some feed content", x: 200, y: 300)]

        let plan = explorer.buildScreenPlan(
            classified: content, visitedElements: [], icons: [])

        XCTAssertFalse(plan.contains { $0.reason.contains("tab-synth") })
        XCTAssertFalse(plan.contains { self.instagramTabs.contains($0.displayLabel) })
    }

    // MARK: - Text-match priority

    func testTextLabeledBarMatchesByTextNotSynthesis() {
        // App Store-style labeled tab bar: tabs carry OCR text → Strategy 1 matches.
        let explorer = makeExplorer(tabs: instagramTabs)
        let labeled = instagramTabs.enumerated().map { index, name in
            nav(name, x: Double(45 + index * 80), y: 855)
        }

        let plan = explorer.buildScreenPlan(
            classified: labeled, visitedElements: [], icons: [])

        let tabItems = plan.filter {
            self.instagramTabs.contains($0.displayLabel) && $0.isBreadthNavigation
        }
        XCTAssertEqual(Set(tabItems.map { $0.displayLabel }), Set(instagramTabs))
        XCTAssertTrue(tabItems.allSatisfy { $0.reason.contains("tab-driven(") },
            "Labeled bar should match by text, not geometric synthesis")
        XCTAssertFalse(plan.contains { $0.reason.contains("tab-synth") })
    }

    func testEmptyTextIconsDoNotSpuriouslyTextMatch() {
        // Icon-only points carry text "": they must NOT match every tab via
        // `tabLower.contains("")`; the bar must fall through to synthesis.
        let explorer = makeExplorer(tabs: instagramTabs)

        let plan = explorer.buildScreenPlan(
            classified: [], visitedElements: [], icons: noisyFeedIcons)

        XCTAssertFalse(plan.contains { $0.reason.contains("tab-driven(") },
            "Empty-text icons must not be text-matched in Strategy 1")
        XCTAssertEqual(
            plan.filter { $0.reason.contains("tab-synth") }.count, instagramTabs.count)
    }

    // MARK: - Visited tabs

    func testAlreadyVisitedTabsAreNotReinjected() {
        let explorer = makeExplorer(tabs: instagramTabs)

        let plan = explorer.buildScreenPlan(
            classified: [], visitedElements: ["Accueil", "Reels"], icons: noisyFeedIcons)

        let synthesized = plan.filter { $0.reason.contains("tab-synth") }.map { $0.displayLabel }
        XCTAssertEqual(Set(synthesized), Set(["Recherche", "Boutique", "Profil"]))
        // Index-based positions are preserved: Recherche is tab index 1 → (1.5)*82 = 123.
        let recherche = plan.first { $0.displayLabel == "Recherche" }
        XCTAssertEqual(recherche?.point.tapX ?? -1, 123, accuracy: 0.5)
    }

    // MARK: - Resolver bypass for text-less synthesized anchors

    func testSynthesizedTabAnchorTappedAtPlannedCoordinatesWithoutTextMatch() {
        let session = ExplorationSession()
        session.start(appName: "TestApp", goal: "test")
        session.setAppDescription(makeAppDescription(tabs: instagramTabs))
        session.capture(
            elements: makeExplorerElements(["Feed item"]), hints: [], icons: [],
            actionType: nil, arrivedVia: nil, screenshotBase64: "img0")
        let explorer = BFSExplorer(session: session, budget: .default)
        explorer.markStarted()

        let graph = session.currentGraph
        let fp = graph.rootFingerprint
        let anchor = RankedElement(
            point: TapPoint(text: "", tapX: 205, tapY: 846, confidence: 1.0),
            score: 100, reason: "tab-synth(Reels@2,horizontal)",
            displayLabel: "Reels", isBreadthNavigation: true)
        graph.setScreenPlan(for: fp, plan: [anchor])

        // Viewport carries no "Reels" text — pure text resolution would fail.
        let viewport = makeExplorerElements(["Feed item", "Another post"])
        let resolved = explorer.resolveNextPlanItem(
            currentFP: fp, viewportElements: viewport,
            describer: MockExplorerDescriber(screens: [
                ScreenDescriber.DescribeResult(elements: viewport, screenshotBase64: "img")
            ]),
            input: MockExplorerInput(), strategy: MobileAppStrategy.self)

        XCTAssertEqual(resolved?.displayLabel, "Reels")
        XCTAssertEqual(resolved?.point.tapX ?? -1, 205, accuracy: 0.5)
        XCTAssertEqual(resolved?.point.tapY ?? -1, 846, accuracy: 0.5)
    }

    // MARK: - No tabs declared

    func testNoTabsDeclaredLeavesPlanUnchanged() {
        let result = TabTargetInjector.inject(
            into: [], classifiedPoints: [], icons: noisyFeedIcons, visitedElements: [],
            tabs: [], tabLayout: nil, windowSize: CGSize(width: 410, height: 890))

        XCTAssertTrue(result.isEmpty)
    }
}
