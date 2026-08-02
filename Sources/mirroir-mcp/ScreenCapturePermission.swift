// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Requests macOS Screen Recording access for the Mirroir MCP host.

import CoreGraphics

enum ScreenCapturePermission {
    static func requestIfNeeded(
        preflight: () -> Bool,
        request: () -> Bool
    ) -> Bool {
        preflight() || request()
    }

    static func requestFromSystem() -> Bool {
        guard #available(macOS 15.0, *) else { return true }
        return requestIfNeeded(
            preflight: { CGPreflightScreenCaptureAccess() },
            request: { CGRequestScreenCaptureAccess() }
        )
    }
}
