// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Spotlight-launch + Spotlight-dismissal helper shared by handleStart and handleExplore.
// ABOUTME: Returns a Result so callers can preserve distinct error wording for launch RPC vs lingering Spotlight.

import Foundation
import HelperLib

extension MirroirMCP {

    /// Variants returned by `launchAndWait` on failure. Both wrap a fully
    /// formed user-facing message; the variant tag exists for callers that
    /// want to distinguish them in logging or recovery flows.
    enum LaunchFailure: Error, Sendable {
        case launchFailed(String)
        case spotlightLingered(String)

        var message: String {
            switch self {
            case .launchFailed(let m), .spotlightLingered(let m): return m
            }
        }
    }

    /// Max re-describes to wait out a home screen briefly showing after launch.
    private static let homeScreenSyncRetries = 4

    /// Spotlight-launch `appName` and wait for the overlay to dismiss, returning
    /// the first clean screen describe result.
    static func launchAndWait(
        appName: String,
        ctx: TargetContext
    ) -> Result<ScreenDescriber.DescribeResult, LaunchFailure> {
        if let launchError = ctx.input.launchApp(name: appName) {
            return .failure(.launchFailed("Failed to launch '\(appName)': \(launchError)"))
        }
        guard var result = SpotlightDetector.waitForDismissal(describer: ctx.describer) else {
            return .failure(.spotlightLingered(
                "'\(appName)' did not appear after launch — " +
                "Spotlight search may still be visible. Try launching the app manually first."))
        }
        // Post-launch sync: after a reset, Spotlight can dismiss onto the home
        // screen for a moment before the app finishes launching. Re-describe a
        // few times so exploration starts inside the app, not on the springboard.
        let screenHeight = Double(ctx.bridge.getWindowInfo()?.size.height ?? 898)
        var attempts = 0
        while attempts < homeScreenSyncRetries,
              AppContextDetector.detectHomeScreen(
                  elements: result.elements, screenHeight: screenHeight) {
            DebugLog.log("explore",
                "post-launch: home screen detected, waiting for '\(appName)' to foreground "
                + "(\(attempts + 1)/\(homeScreenSyncRetries))")
            usleep(EnvConfig.toolSettlingDelayUs)
            guard let next = ctx.describer.describe() else { break }
            result = next
            attempts += 1
        }
        return .success(result)
    }
}
