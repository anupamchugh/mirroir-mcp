// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Unit tests for AppSwitcherCardLocator: matches foreground app OCR text against App Switcher OCR to find the app's card position.
// ABOUTME: Covers single-card layout, multi-card carousel with the target on the right, and the no-match fallback path.

import XCTest
@testable import HelperLib
@testable import mirroir_mcp

final class AppSwitcherCardLocatorTests: XCTestCase {

    private func tap(_ text: String, x: Double, y: Double = 200) -> TapPoint {
        TapPoint(text: text, tapX: x, tapY: y, confidence: 0.95)
    }

    // MARK: - Single-card layout

    func testReturnsCenterXWhenOnlyTargetCardVisible() {
        // Foreground Santé app with distinctive text.
        let app: [TapPoint] = [
            tap("Résumé", x: 92), tap("Épinglés", x: 88),
            tap("Activité", x: 82), tap("Bouger", x: 67),
            tap("Entraînements", x: 107), tap("Distance", x: 155),
        ]
        // App Switcher with only Santé (centered, smaller font).
        let switcher: [TapPoint] = [
            tap("Résumé", x: 200), tap("Épinglés", x: 198),
            tap("Activité", x: 195), tap("Bouger", x: 200),
        ]
        let result = AppSwitcherCardLocator.locateCardX(
            appElements: app, switcherElements: switcher
        )
        XCTAssertNotNil(result)
        if let r = result {
            XCTAssert(r >= 195 && r <= 200, "Should center on the only card; got \(r)")
        }
    }

    // MARK: - Multi-card layout

    func testFindsRightmostCardInThreeCardCarousel() {
        // Foreground Santé.
        let app: [TapPoint] = [
            tap("Résumé", x: 92), tap("Épinglés", x: 88),
            tap("Activité", x: 82), tap("Bouger", x: 67),
            tap("680 cal", x: 80), tap("Entraînements", x: 107),
        ]
        // App Switcher: Messages on left, Tapo center, Santé right.
        let switcher: [TapPoint] = [
            // Messages card content (left, x ~50-70)
            tap("Vous", x: 60),
            tap("Sans toi...pi", x: 71),
            tap("Ah oui? Pd", x: 70),
            tap("Pis? Comn", x: 70),
            // Tapo card content (center, x ~150-200)
            tap("Tapo", x: 176),
            tap("01/36", x: 114),
            // Santé card content (right, x ~340-370) — these are the matches
            tap("Résumé", x: 359),
            tap("Épinglés", x: 357),
            tap("Activité", x: 358),
            tap("Bouger", x: 343),
            tap("680 cal", x: 350),
            tap("Entraînemen", x: 363),  // truncated in App Switcher OCR
        ]
        let result = AppSwitcherCardLocator.locateCardX(
            appElements: app, switcherElements: switcher
        )
        XCTAssertNotNil(result)
        if let r = result {
            XCTAssert(r >= 340 && r <= 370,
                "Should locate Santé card at right (x ~340-370); got \(r)")
        }
    }

    // MARK: - No match fallback

    func testReturnsNilWhenNoTextOverlap() {
        let app: [TapPoint] = [
            tap("Réglages", x: 50), tap("Wi-Fi", x: 50), tap("Bluetooth", x: 50),
        ]
        // App Switcher shows completely different apps.
        let switcher: [TapPoint] = [
            tap("Tapo", x: 176),
            tap("Vous", x: 60),
            tap("Sans toi", x: 70),
        ]
        XCTAssertNil(AppSwitcherCardLocator.locateCardX(
            appElements: app, switcherElements: switcher
        ))
    }

    func testReturnsNilWhenAppOcrEmpty() {
        let switcher: [TapPoint] = [tap("Résumé", x: 200)]
        XCTAssertNil(AppSwitcherCardLocator.locateCardX(
            appElements: [], switcherElements: switcher
        ))
    }

    func testReturnsNilWhenSwitcherOcrEmpty() {
        let app: [TapPoint] = [tap("Résumé", x: 92)]
        XCTAssertNil(AppSwitcherCardLocator.locateCardX(
            appElements: app, switcherElements: []
        ))
    }

    func testIgnoresShortNoiseFragments() {
        // Single-character OCR noise should not anchor matching.
        let app: [TapPoint] = [
            tap("X", x: 50), tap(".", x: 60), tap("longerword", x: 70),
        ]
        // Switcher with the noise but at a different cluster than the real word.
        let switcher: [TapPoint] = [
            tap("X", x: 380), tap(".", x: 380),
            tap("longerword", x: 100),
        ]
        let result = AppSwitcherCardLocator.locateCardX(
            appElements: app, switcherElements: switcher
        )
        // Only "longerword" should match (X and . are too short).
        XCTAssertEqual(result ?? -1, 100, accuracy: 1)
    }

    func testCaseInsensitiveMatching() {
        let app: [TapPoint] = [tap("RÉSUMÉ", x: 92), tap("ÉPINGLÉS", x: 88)]
        let switcher: [TapPoint] = [
            tap("résumé", x: 359), tap("épinglés", x: 360),
        ]
        let result = AppSwitcherCardLocator.locateCardX(
            appElements: app, switcherElements: switcher
        )
        XCTAssertNotNil(result)
        XCTAssertEqual(result ?? -1, 359.5, accuracy: 1)
    }
}
