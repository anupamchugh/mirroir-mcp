// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Tests for the session-escape plugin framework (camera dialog / resume overlay).
// ABOUTME: Uses a recording click closure so no real CGEvent clicks are posted.

import XCTest
import CoreGraphics
import HelperLib
@testable import mirroir_mcp

final class SessionEscapeTests: XCTestCase {

    // MARK: - ContinuityDialogEscape

    func testContinuityDialogEscapeClicksDismissButton() {
        let bridge = StubBridge()
        bridge.pausedButtonPoint = CGPoint(x: 1474, y: 570)
        var clicks: [CGPoint] = []

        let acted = ContinuityDialogEscape().attemptEscape(
            bridge: bridge, click: { clicks.append($0) }, tag: "test")

        XCTAssertTrue(acted)
        XCTAssertEqual(clicks.count, 1)
        XCTAssertEqual(clicks.first?.x, 1474)
        XCTAssertEqual(clicks.first?.y, 570)
    }

    func testContinuityDialogEscapeNoOpWhenNoButton() {
        let bridge = StubBridge()
        bridge.pausedButtonPoint = nil
        var clicks: [CGPoint] = []

        let acted = ContinuityDialogEscape().attemptEscape(
            bridge: bridge, click: { clicks.append($0) }, tag: "test")

        XCTAssertFalse(acted)
        XCTAssertTrue(clicks.isEmpty)
    }

    // MARK: - ResumeOverlayEscape

    func testResumeOverlayEscapePressesResume() {
        let bridge = StubBridge()
        bridge.pressResumeResult = true
        XCTAssertTrue(
            ResumeOverlayEscape().attemptEscape(bridge: bridge, click: { _ in }, tag: "test"))
    }

    // MARK: - Registry dispatch order

    func testRegistryPrefersContinuityDialogOverResume() {
        // Camera dialog present (titled button) → it handles the escape; the
        // resume overlay plugin must not run (state stays paused, no resume flip).
        let bridge = StubBridge()
        bridge.state = .paused
        bridge.pausedButtonPoint = CGPoint(x: 205, y: 515)
        bridge.connectOnResume = true
        var clicks: [CGPoint] = []

        let acted = SessionEscapeRegistry.attemptEscape(
            bridge: bridge, click: { clicks.append($0) }, tag: "test")

        XCTAssertTrue(acted)
        XCTAssertEqual(clicks.count, 1, "camera-dialog escape should act first")
        XCTAssertEqual(bridge.state, .paused,
            "resume overlay must not run when the camera dialog handled the escape")
    }

    func testRegistryFallsBackToResumeWhenNoDialog() {
        // No titled button → resume overlay plugin AX-presses (no click fired).
        let bridge = StubBridge()
        bridge.pausedButtonPoint = nil
        bridge.pressResumeResult = true
        var clicks: [CGPoint] = []

        let acted = SessionEscapeRegistry.attemptEscape(
            bridge: bridge, click: { clicks.append($0) }, tag: "test")

        XCTAssertTrue(acted)
        XCTAssertTrue(clicks.isEmpty, "no dialog button → resume via AX-press, no click")
    }

    func testRegistryReturnsFalseWhenNoPluginApplies() {
        // No button and resume fails → nothing acted.
        let bridge = StubBridge()
        bridge.pausedButtonPoint = nil
        bridge.pressResumeResult = false
        XCTAssertFalse(
            SessionEscapeRegistry.attemptEscape(bridge: bridge, click: { _ in }, tag: "test"))
    }
}
