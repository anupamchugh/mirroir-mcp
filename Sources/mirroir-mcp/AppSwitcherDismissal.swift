// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Shared force-quit flow for iPhone Mirroring — launch via Spotlight, locate the card via OCR intersection, drag to dismiss, return home.
// ABOUTME: Single source of truth used by reset_app (MCP tool), compiled-skill replay, and explore reset_before_explore so all paths fail closed identically.

import CoreGraphics
import Foundation
import HelperLib

/// AppSwitcherDismissal — orchestration helper that force-quits an iPhone
/// app via the App Switcher.
///
/// Steps:
/// 1. Launch the app (resolves localized display name via Spotlight)
/// 2. Capture foreground OCR (fingerprint)
/// 3. Open App Switcher
/// 4. Locate the just-launched card via OCR-intersection (`AppSwitcherCardLocator`)
/// 5. Drag the card upward to dismiss
/// 6. Return to the home screen, retrying once if iOS lands on Spotlight
///
/// The flow fails **closed** at every step: if launch fails, OCR is empty, or
/// the card cannot be located, the function returns an error message and does
/// not blindly drag at a hard-coded fraction. The `reset_app` tool, compiled
/// `reset_app` step replay, and `forceQuitBeforeExplore` lifecycle hook all
/// share this single implementation.
enum AppSwitcherDismissal {

    /// Force-quit `appName`. Returns `nil` on success, an error message on
    /// any failure step. Caller is responsible for the surrounding tool
    /// response/StepResult shape — this helper is action-only.
    ///
    /// Preconditions:
    /// - `bridge` and `input` operate on the same iPhone Mirroring window
    /// - `menuBridge` and `bridge` refer to the same target
    /// - `describer` reads from the same window
    static func forceQuit(
        appName: String,
        input: any InputProviding,
        bridge: any WindowBridging,
        menuBridge: any MenuActionCapable,
        describer: any ScreenDescribing
    ) -> String? {
        // 1. Launch — handles localized app names via Spotlight.
        if let error = input.launchApp(name: appName) {
            return "Failed to launch '\(appName)': \(error)"
        }
        usleep(EnvConfig.toolSettlingDelayUs)

        // 2. Capture foreground OCR — fingerprint for card matching.
        guard let appOcr = describer.describe(), !appOcr.elements.isEmpty else {
            return "Failed to capture foreground OCR for '\(appName)'"
        }

        // 3. Open App Switcher.
        guard menuBridge.triggerMenuAction(menu: "View", item: "App Switcher") else {
            _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
            return "Failed to open App Switcher. Is '\(bridge.targetName)' running?"
        }
        usleep(EnvConfig.toolSettlingDelayUs)

        // 4. Capture App Switcher OCR.
        guard let switcherOcr = describer.describe() else {
            _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
            return "Failed to capture App Switcher screen for verification"
        }
        guard !switcherOcr.elements.isEmpty else {
            _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
            return "App Switcher appears empty — no app cards detected"
        }

        // 5. Locate the matching card via OCR intersection. Fail closed
        // when the locator returns nil — never drag a guess.
        guard let cardX = AppSwitcherCardLocator.locateCardX(
            appElements: appOcr.elements,
            switcherElements: switcherOcr.elements
        ) else {
            _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
            return "Cannot locate '\(appName)' card in App Switcher (OCR text intersection found no match) — refusing to drag at hard-coded coordinates"
        }
        DebugLog.log("reset_app", "located '\(appName)' card at x=\(Int(cardX)) via OCR match")

        // 6. Drag the card upward.
        let windowSize = bridge.getWindowInfo()?.size ?? CGSize(width: 410, height: 898)
        let cardY = Double(windowSize.height) * EnvConfig.appSwitcherCardYFraction
        let toY = max(0, cardY - EnvConfig.appSwitcherSwipeDistance)
        if let error = input.drag(
            fromX: cardX, fromY: cardY,
            toX: cardX, toY: toY,
            durationMs: EnvConfig.appSwitcherSwipeDurationMs
        ) {
            _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
            return "Failed to swipe app card: \(error)"
        }
        usleep(EnvConfig.toolSettlingDelayUs)

        // 7. Return to home screen.
        //
        // After a successful card-dismiss drag, iOS occasionally interprets
        // the next View > Home Screen menu trigger as a Spotlight invocation
        // instead of a home gesture — landing the user on Spotlight with
        // the previous query still typed. Verify the result by OCR and retry
        // once when Spotlight is detected; the second trigger reliably
        // reaches home.
        _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
        usleep(EnvConfig.toolSettlingDelayUs)
        if let postHomeOcr = describer.describe(),
           SpotlightDetector.isSpotlightVisible(elements: postHomeOcr.elements) {
            DebugLog.log("reset_app", "Home Screen menu landed on Spotlight, retrying once")
            _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
        }
        return nil
    }
}
