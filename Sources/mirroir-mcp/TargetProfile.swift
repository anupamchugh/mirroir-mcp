// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: TargetProfile — pure value type describing an automation target's capabilities and user-facing messages.
// ABOUTME: Per-target profile constants live in the target-specific file (IPhoneMirroringTarget, MacOSAppTarget, …).

import Foundation
import HelperLib

/// How tap coordinates and safe zones are interpreted for this target.
enum CoordinateSystem: String, Sendable {
    /// iOS-style viewport with a status bar at top and a tab bar / home indicator at bottom.
    /// Safe-zone stencils reject taps outside the app content area.
    case mobile
    /// Desktop window with arbitrary content layout. No mobile-specific safe zones.
    case desktop
}

/// Vertical safe-zone stencil for rejecting invalid tap targets.
/// Mobile targets exclude status bar (top) and home indicator (bottom);
/// desktop targets treat the entire window as tappable.
struct SafeZoneConfig: Sendable {
    /// Minimum tap Y in points. Taps above this are rejected as "in status bar".
    let minTapY: Double
    /// Maximum tap Y as a fraction of window height. Taps below this are rejected
    /// unless the element is marked as breadth-navigation (tab bar).
    let maxTapYFraction: Double

    /// iOS mobile safe zone: reject above status bar and below 95% of window.
    static let mobile = SafeZoneConfig(minTapY: 80.0, maxTapYFraction: 0.95)

    /// Desktop safe zone: the entire window is valid.
    static let desktop = SafeZoneConfig(minTapY: 0.0, maxTapYFraction: 1.0)
}

/// Capability metadata describing how an automation target behaves.
///
/// Every behavioral branch that previously checked `targetType == "iphone-mirroring"`
/// or a shadow `isMobile: Bool` flag queries a field on this struct instead. Each
/// target module (IPhoneMirroringTarget, MacOSAppTarget, …) declares its own
/// profile constant — this file stays as a pure value type.
struct TargetProfile: Sendable {
    /// Stable target identifier used for telemetry and config-file lookup.
    /// Not used for runtime behavior branching.
    let name: String
    /// Human-readable name for error messages ("iPhone Mirroring", "Telegram").
    let displayName: String
    /// Coordinate-system interpretation for safe zones, tab bar detection, etc.
    let coordinateSystem: CoordinateSystem
    /// Cursor behavior for tap/swipe/drag actions.
    let cursorMode: CursorMode
    /// Whether Cmd+L / Cmd+T / etc. reach the target app.
    let supportsKeyboardShortcuts: Bool
    /// Whether the `open_url` MCP tool works on this target.
    let supportsDirectURLOpen: Bool
    /// User-facing explanation when `open_url` is invoked on a target that does
    /// not support it. `nil` when the target supports URL opening.
    let urlOpenUnsupportedMessage: String?
    /// Stencil for rejecting taps outside the valid content area.
    let safeZone: SafeZoneConfig
    /// Default exploration strategy when no explicit override or recipe match applies.
    let defaultStrategy: StrategyChoice
}
