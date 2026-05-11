// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
// ABOUTME: Integration tests for interactive FakeMirroring tap → navigation flows.
// ABOUTME: Validates that tapping elements in FakeMirroring triggers scenario transitions detected by OCR.

import XCTest
import HelperLib
@testable import mirroir_mcp

/// Tests that tapping rendered elements in FakeMirroring causes visible screen transitions.
/// Validates the full pipeline: OCR → compute coordinates → tap → verify new screen content.
///
/// These tests exercise real CGEvent taps against FakeMirroring's mouseUp hit detection,
/// proving that the input simulation and OCR pipeline produce correct, actionable coordinates.
final class TapNavigationTests: XCTestCase {

    private var bridge: MirroringBridge!
    private var input: InputSimulation!
    private var describer: ScreenDescriber!

    override func setUpWithError() throws {
        try super.setUpWithError()
        try IntegrationTestHelper.ensureFakeMirroringRunning()
        bridge = MirroringBridge(bundleID: IntegrationTestHelper.fakeBundleID)
        guard IntegrationTestHelper.ensureWindowReady(bridge: bridge) else {
            throw IntegrationTestError.windowNotCapturable
        }
        let capture = ScreenCapture(bridge: bridge)
        describer = ScreenDescriber(bridge: bridge, capture: capture)
        input = InputSimulation(bridge: bridge)

        // Start every test on Settings scenario
        _ = bridge.triggerMenuAction(menu: "Scenario", item: "Settings")
        usleep(500_000)
    }

    override func tearDown() {
        // Restore Settings scenario for other test classes
        if let bridge = bridge {
            _ = bridge.triggerMenuAction(menu: "Scenario", item: "Settings")
            usleep(500_000)
            IntegrationTestHelper.ensureWindowReady(bridge: bridge)
        }
        super.tearDown()
    }

    // MARK: - Row Tap Navigation

    /// Tap "General" row on Settings → should navigate to Detail screen.
    /// Verifies by checking for "Keyboard" which exists only on Detail, not Settings.
    func testTapGeneralNavigatesToDetail() throws {
        try tapAndVerifyNavigation(
            tapLabel: "General",
            expectedElement: "Keyboard",
            description: "Settings → General → Detail"
        )
    }

    /// Drill Settings → General → About and verify the leaf screen with the
    /// "Model Number" detail. Exercises the same multi-hop tap-and-verify
    /// pipeline as the legacy "Settings → About → Detail (Back)" path, against
    /// the realistic Settings hierarchy (About lives under General, not root).
    func testTapAboutNavigatesToDetailWithBack() throws {
        try tapAndVerifyNavigation(
            tapLabel: "General",
            expectedElement: "About",
            description: "Settings → General"
        )
        try tapAndVerifyNavigation(
            tapLabel: "About",
            expectedElement: "Model Number",
            description: "General → About"
        )
    }

    // MARK: - Back Chevron Navigation

    /// Drill into Settings → General (a screen with a back chevron) and tap
    /// the "<" chevron to return to Settings. Verifies that OCR can locate
    /// the chevron glyph and that a tap there triggers the parent navigation.
    func testBackChevronReturnsToSettings() throws {
        // Navigate Settings → General through a row tap so we land on a
        // screen with back: main (renders the back chevron).
        try tapAndVerifyNavigation(
            tapLabel: "General",
            expectedElement: "About",
            description: "Settings → General"
        )

        // Find and tap the "<" back chevron rendered by drawBackChevron().
        let beforeScreen = try describeOrFail()
        guard let chevron = beforeScreen.elements.first(where: {
            $0.text == "<" || $0.text == "‹" || $0.text == "〈" || $0.text == "く"
        }) else {
            throw IntegrationTestError.elementNotFound("< (back chevron)")
        }

        let tapError = input.tap(x: chevron.tapX, y: chevron.tapY)
        XCTAssertNil(tapError, "Tap on back chevron should succeed: \(tapError ?? "")")
        usleep(800_000)

        // After the back tap we should be back on Settings root.
        let afterScreen = try describeOrFail()
        let afterTexts = afterScreen.elements.map { $0.text.lowercased() }
        XCTAssertTrue(afterTexts.contains("settings"),
                      "After back tap, should be on Settings. Found: \(afterTexts)")
    }

    // MARK: - Button Tap

    /// On Login screen, tap "Log In" button → should navigate to Feed.
    /// Verifies by checking for "johndoe" which exists only on Feed, not Login.
    func testTapLoginButtonNavigatesToFeed() throws {
        _ = bridge.triggerMenuAction(menu: "Scenario", item: "Login")
        usleep(500_000)

        try tapAndVerifyNavigation(
            tapLabel: "Log In",
            expectedElement: "johndoe",
            description: "Login → Log In → Feed"
        )
    }

    // MARK: - Card Tap

    /// On the Health summary screen, tap the "Steps" row → should navigate
    /// to the Steps detail screen. Verifies by checking for "Daily Average"
    /// which exists only on the detail screen, not the summary.
    func testTapHealthCardNavigatesToDetail() throws {
        _ = bridge.triggerMenuAction(menu: "Scenario", item: "Health")
        usleep(500_000)

        try tapAndVerifyNavigation(
            tapLabel: "Steps",
            expectedElement: "Daily Average",
            description: "Health → Steps → Detail"
        )
    }

    // MARK: - Helpers

    /// OCR the current screen, find the element matching `tapLabel`, tap it,
    /// then OCR again and verify that `expectedElement` (unique to the destination screen)
    /// appears in the OCR results.
    private func tapAndVerifyNavigation(
        tapLabel: String,
        expectedElement: String,
        description: String
    ) throws {
        let screen = try describeOrFail()
        let elements = screen.elements

        guard let target = elements.first(where: {
            $0.text.caseInsensitiveCompare(tapLabel) == .orderedSame
        }) else {
            throw IntegrationTestError.elementNotFound("\(tapLabel) (\(description))")
        }

        let tapError = input.tap(x: target.tapX, y: target.tapY)
        XCTAssertNil(tapError, "Tap on '\(tapLabel)' should succeed: \(tapError ?? "")")
        usleep(800_000)

        let afterScreen = try describeOrFail()
        let afterTexts = afterScreen.elements.map { $0.text.lowercased() }
        XCTAssertTrue(
            afterTexts.contains(expectedElement.lowercased()),
            "\(description): expected '\(expectedElement)' after tap. Found: \(afterTexts)"
        )
    }

    private func describeOrFail() throws -> ScreenDescriber.DescribeResult {
        for attempt in 1...3 {
            if let result = describer.describe() { return result }
            if attempt < 3 { usleep(500_000) }
        }
        throw IntegrationTestError.describeReturnedNil
    }
}
