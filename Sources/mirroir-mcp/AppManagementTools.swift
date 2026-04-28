// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Registers the reset_app MCP tool for force-quitting apps via the App Switcher.
// ABOUTME: Launches the app first via Spotlight (handles localization), then dismisses its card.

import Foundation
import HelperLib

extension MirroirMCP {
    static func registerAppManagementTools(
        server: MCPServer,
        registry: TargetRegistry
    ) {
        // reset_app — force-quit an app via the App Switcher
        server.registerTool(MCPToolDefinition(
            name: "reset_app",
            description: """
                Force-quit an app on the mirrored iPhone by launching it via Spotlight \
                (which handles localization), opening the App Switcher where the \
                just-launched app is the centered card, and swiping it up to dismiss. \
                Use this before launch_app to ensure a fresh start.
                """,
            inputSchema: [
                "type": .string("object"),
                "properties": .object([
                    "name": .object([
                        "type": .string("string"),
                        "description": .string("The app name to force-quit"),
                    ])
                ]),
                "required": .array([.string("name")]),
            ],
            handler: { args in
                let (ctx, err) = registry.resolveForTool(args)
                guard let ctx else { return err! }
                guard let menuBridge = ctx.bridge as? (any MenuActionCapable) else {
                    return .error("Target '\(ctx.name)' does not support reset_app")
                }
                let input = ctx.input

                guard let appName = args["name"]?.asString() else {
                    return .error("Missing required parameter: name (string)")
                }

                // Launch the app via Spotlight. This handles localization: typing
                // "Settings" finds "Réglages" on a French iPhone, for example.
                if let error = input.launchApp(name: appName) {
                    return .error("Failed to launch '\(appName)': \(error)")
                }
                usleep(EnvConfig.toolSettlingDelayUs)

                // Capture the foreground app's text before opening App
                // Switcher. This becomes the fingerprint we use to identify
                // the right card — iPhone Mirroring's App Switcher does NOT
                // always center the just-launched app, so a hard-coded
                // x-fraction is unreliable.
                let describer = ctx.describer
                let appOcr = describer.describe()
                guard let appElements = appOcr?.elements, !appElements.isEmpty else {
                    return .error("Failed to capture foreground OCR for '\(appName)'")
                }

                // Open the App Switcher.
                guard menuBridge.triggerMenuAction(menu: "View", item: "App Switcher") else {
                    _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
                    return .error("Failed to open App Switcher. Is '\(ctx.name)' running?")
                }
                usleep(EnvConfig.toolSettlingDelayUs)

                let windowSize = ctx.bridge.getWindowInfo()?.size
                guard let switcherOcr = describer.describe() else {
                    _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
                    return .error("Failed to capture App Switcher screen for verification")
                }
                guard !switcherOcr.elements.isEmpty else {
                    _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
                    return .error("App Switcher appears empty — no app cards detected")
                }

                // Find the card whose preview shows the just-launched app by
                // intersecting OCR text. Falls back to the configured
                // x-fraction if no clear match (e.g. preview text too small
                // for OCR to read).
                let windowWidth = windowSize.map { Double($0.width) } ?? 410.0
                let cardX: Double
                if let locatedX = AppSwitcherCardLocator.locateCardX(
                    appElements: appElements,
                    switcherElements: switcherOcr.elements
                ) {
                    DebugLog.log("reset_app", "located '\(appName)' card at x=\(Int(locatedX)) via OCR match")
                    cardX = locatedX
                } else {
                    DebugLog.log("reset_app", "no OCR match for '\(appName)' card, falling back to xFraction=\(EnvConfig.appSwitcherCardXFraction)")
                    cardX = windowWidth * EnvConfig.appSwitcherCardXFraction
                }

                let cardY = (windowSize.map { Double($0.height) } ?? 890.0) * EnvConfig.appSwitcherCardYFraction
                let toY = max(0, cardY - EnvConfig.appSwitcherSwipeDistance)
                if let error = input.drag(fromX: cardX, fromY: cardY,
                                           toX: cardX, toY: toY,
                                           durationMs: EnvConfig.appSwitcherSwipeDurationMs) {
                    _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
                    return .error("Failed to swipe app card: \(error)")
                }

                usleep(EnvConfig.toolSettlingDelayUs)

                // Return to home screen.
                //
                // After a successful card-dismiss drag, iOS occasionally
                // interprets the next View > Home Screen menu trigger as a
                // Spotlight invocation instead of a home gesture — landing
                // the user on Spotlight (with the previous query still
                // typed). Verify the result by OCR and retry once when
                // Spotlight is detected; the second trigger reliably
                // reaches home.
                _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
                usleep(EnvConfig.toolSettlingDelayUs)
                if let postHomeOcr = describer.describe(),
                   SpotlightDetector.isSpotlightVisible(elements: postHomeOcr.elements) {
                    DebugLog.log("reset_app", "Home Screen menu landed on Spotlight, retrying once")
                    _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
                }

                return .text("Force-quit '\(appName)'")
            }
        ))
    }
}
