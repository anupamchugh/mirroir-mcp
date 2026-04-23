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
        input: any InputProviding
    ) {
        DebugLog.log("lifecycle",
            "MirroringAppLifecycle.forceQuit('\(appName)') — App Switcher swipe-up")
        guard let menuBridge = bridge as? (any MenuActionCapable) else {
            DebugLog.log("lifecycle", "bridge is not MenuActionCapable — skipping force-quit")
            return
        }
        _ = menuBridge.triggerMenuAction(menu: "View", item: "App Switcher")
        usleep(500_000)
        let preSize = bridge.getWindowInfo()?.size ?? CGSize(width: 410, height: 890)
        // Swipe up to dismiss the frontmost app card.
        _ = input.swipe(
            fromX: preSize.width / 2, fromY: preSize.height * 0.4,
            toX: preSize.width / 2, toY: 0, durationMs: 300
        )
        usleep(500_000)
        _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
        usleep(300_000)
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
        input: any InputProviding
    ) {
        DebugLog.log("lifecycle",
            "MacOSAppLifecycle.forceQuit('\(appName)') — Cmd+Q")
        bridge.activate()
        usleep(200_000)
        _ = input.pressKey(keyName: "q", modifiers: ["command"])
        usleep(500_000)
    }
}
