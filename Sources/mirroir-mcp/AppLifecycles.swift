// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Concrete AppLifecycleHandling implementations — iPhone Mirroring vs. native macOS app.
// ABOUTME: iPhone uses Spotlight-via-Mirroring + App Switcher swipe-up; macOS uses launch via InputProviding + Cmd+Q.

import CoreGraphics
import Foundation
import HelperLib

/// Launch and force-quit an iPhone app via the macOS Mirroring bridge. Launch
/// uses Spotlight search (driven through `InputProviding.launchApp`), and
/// force-quit uses the App Switcher gesture (swipe up on the app card).
struct MirroringAppLifecycle: AppLifecycleHandling {
    func launch(appName: String, input: any InputProviding) -> String? {
        input.launchApp(name: appName)
    }

    func forceQuitBeforeExplore(
        appName: String,
        bridge: any WindowBridging,
        input: any InputProviding,
        describer: any ScreenDescribing
    ) -> String? {
        DebugLog.log("lifecycle",
            "MirroringAppLifecycle.forceQuit('\(appName)') — App Switcher OCR-locate + drag")
        guard let menuBridge = bridge as? (any MenuActionCapable) else {
            return "Bridge for '\(bridge.targetName)' is not MenuActionCapable — cannot force-quit"
        }
        return AppSwitcherDismissal.forceQuit(
            appName: appName,
            input: input,
            bridge: bridge,
            menuBridge: menuBridge,
            describer: describer
        )
    }
}

/// Launch and force-quit a native macOS application. Launch delegates to the
/// shared `InputProviding.launchApp` path (which uses NSWorkspace / `open -a`);
/// force-quit sends Cmd+Q to the frontmost target window.
struct MacOSAppLifecycle: AppLifecycleHandling {
    func launch(appName: String, input: any InputProviding) -> String? {
        input.launchApp(name: appName)
    }

    func forceQuitBeforeExplore(
        appName: String,
        bridge: any WindowBridging,
        input: any InputProviding,
        describer: any ScreenDescribing
    ) -> String? {
        DebugLog.log("lifecycle",
            "MacOSAppLifecycle.forceQuit('\(appName)') — Cmd+Q")
        bridge.activate()
        usleep(200_000)
        let result = input.pressKey(keyName: "q", modifiers: ["command"])
        usleep(500_000)
        if !result.success {
            return result.error ?? "Cmd+Q failed for '\(appName)'"
        }
        return nil
    }
}
