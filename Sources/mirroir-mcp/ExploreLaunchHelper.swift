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

    /// Spotlight-launch `appName` and wait for the overlay to dismiss, returning
    /// the first clean screen describe result.
    static func launchAndWait(
        appName: String,
        ctx: TargetContext
    ) -> Result<ScreenDescriber.DescribeResult, LaunchFailure> {
        if let launchError = ctx.input.launchApp(name: appName) {
            return .failure(.launchFailed("Failed to launch '\(appName)': \(launchError)"))
        }
        guard let result = SpotlightDetector.waitForDismissal(describer: ctx.describer) else {
            return .failure(.spotlightLingered(
                "'\(appName)' did not appear after launch — " +
                "Spotlight search may still be visible. Try launching the app manually first."))
        }
        return .success(result)
    }
}
