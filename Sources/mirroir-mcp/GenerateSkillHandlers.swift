// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Action handlers for the generate_skill MCP tool (start/capture/finish/explore).
// ABOUTME: Extracted from GenerateSkillTools.swift to keep the tool registration file focused on schema.

import AppKit
import Foundation
import HelperLib

extension MirroirMCP {

    // MARK: - Action Handlers

    static func handleStart(
        args: [String: JSONValue],
        session: ExplorationSession,
        registry: TargetRegistry
    ) -> MCPToolResult {
        guard let appName = args["app_name"]?.asString(), !appName.isEmpty else {
            return .error("Missing required parameter: app_name (for start action)")
        }

        if session.active {
            return .error(
                "An exploration session is already active for '\(session.currentAppName)'. " +
                "Call finish first or start a new session.")
        }

        let (ctx, err) = registry.resolveForTool(args)
        guard let ctx else { return err! }

        // Launch the app
        if let launchError = ctx.input.launchApp(name: appName) {
            return .error("Failed to launch '\(appName)': \(launchError)")
        }

        // Wait for app to settle
        usleep(EnvConfig.stepSettlingDelayMs * 1000)

        // Parse goal(s) and start session
        let goal = args["goal"]?.asString() ?? ""
        let goals = args["goals"]?.asStringArray() ?? []
        session.start(appName: appName, goal: goal, goals: goals)

        // Detect and store strategy
        let explicitStrategy = args["strategy"]?.asString()
        let strategyChoice = StrategyDetector.detect(
            targetType: ctx.targetType,
            appName: appName,
            explicitStrategy: explicitStrategy
        )
        session.setStrategy(strategyChoice.rawValue)

        // OCR first screen
        guard let result = ctx.describer.describe() else {
            return .error(
                "Failed to capture/analyze screen after launching '\(appName)'. " +
                "Is the target window visible?")
        }

        // Capture first screen (no action since this is the initial screen)
        session.capture(
            elements: result.elements,
            hints: result.hints,
            icons: result.icons,
            actionType: nil,
            arrivedVia: nil,
            screenshotBase64: result.screenshotBase64
        )

        // Generate mode-specific preamble
        let modeName = session.currentMode == .discovery ? "Discovery" : "Goal-driven"
        var preamble = "Exploration started for '\(appName)' (\(modeName) mode). Screen 1 captured."
        if !goals.isEmpty {
            preamble += " Manifest: \(goals.count) goals queued."
        }

        let description = ExplorationGuidanceHelper.formatScreenDescription(
            elements: result.elements,
            hints: result.hints,
            preamble: preamble
        )

        // Generate initial guidance
        let guidance = ExplorationGuide.analyze(
            mode: session.currentMode,
            goal: session.currentGoal,
            elements: result.elements,
            hints: result.hints,
            startElements: nil,
            actionLog: [],
            screenCount: 1,
            isMobile: ctx.profile.coordinateSystem == .mobile
        )

        let guidanceText = ExplorationGuide.formatGuidance(guidance)

        return MCPToolResult(
            content: [
                .text(description + guidanceText),
                .image(result.screenshotBase64, mimeType: "image/png"),
            ],
            isError: false
        )
    }

    static func handleCapture(
        args: [String: JSONValue],
        session: ExplorationSession,
        registry: TargetRegistry
    ) -> MCPToolResult {
        guard session.active else {
            return .error("No active exploration session. Call generate_skill with action=\"start\" first.")
        }

        let (ctx, err) = registry.resolveForTool(args)
        guard let ctx else { return err! }

        // OCR current screen
        guard let result = ctx.describer.describe() else {
            return .error("Failed to capture/analyze screen. Is the target window visible?")
        }

        let arrivedVia = args["arrived_via"]?.asString()
        let actionType = args["action_type"]?.asString()

        let accepted = session.capture(
            elements: result.elements,
            hints: result.hints,
            icons: result.icons,
            actionType: actionType,
            arrivedVia: arrivedVia,
            screenshotBase64: result.screenshotBase64
        )

        if !accepted {
            // Still provide guidance even on duplicate rejection — use strategy if graph available
            let guidance = ExplorationGuidanceHelper.generateGuidance(
                session: session, elements: result.elements,
                icons: result.icons, hints: result.hints,
                isMobile: ctx.profile.coordinateSystem == .mobile
            )
            let guidanceText = ExplorationGuide.formatGuidance(guidance)

            return .text(
                "Screen unchanged \u{2014} capture skipped (duplicate of previous screen). " +
                "Try a different action before capturing again." + guidanceText)
        }

        let screenNum = session.screenCount
        let preamble = "Screen \(screenNum) captured" +
            (arrivedVia.map { " (arrived via \"\($0)\")" } ?? "") + "."

        let description = ExplorationGuidanceHelper.formatScreenDescription(
            elements: result.elements,
            hints: result.hints,
            preamble: preamble
        )

        // Generate guidance for the agent — prefer strategy-based when graph available
        let guidance = ExplorationGuidanceHelper.generateGuidance(
            session: session, elements: result.elements,
            icons: result.icons, hints: result.hints,
            isMobile: ctx.profile.coordinateSystem == .mobile
        )

        let guidanceText = ExplorationGuide.formatGuidance(guidance)

        return MCPToolResult(
            content: [
                .text(description + guidanceText),
                .image(result.screenshotBase64, mimeType: "image/png"),
            ],
            isError: false
        )
    }

    static func handleFinish(session: ExplorationSession) -> MCPToolResult {
        guard session.active else {
            return .error("No active exploration session. Call generate_skill with action=\"start\" first.")
        }

        guard session.screenCount > 0 else {
            return .error("No screens captured. Use capture action before finishing.")
        }

        // Check for remaining goals before finalize (which advances the queue)
        let remaining = session.remainingGoals
        let goalNum = session.currentGoalIndex + 1
        let totalGoals = session.totalGoals

        guard let data = session.finalize() else {
            return .error("Failed to finalize exploration session.")
        }

        // Use SkillBundleGenerator for multi-path graphs, single skill otherwise
        let bundle = SkillBundleGenerator.generate(
            appName: data.appName,
            goal: data.goal,
            snapshot: data.graphSnapshot,
            allScreens: data.screens,
            recipeMatch: session.currentRecipeMatch,
            appDescription: session.currentAppDescription
        )

        var text = ExplorationResultFormatter.formatBundle(
            bundle, preamble: "Generated \(bundle.skills.count) skills from exploration:")
        if !remaining.isEmpty {
            text += "\n\n---\nGoal \(goalNum)/\(totalGoals) complete. "
            text += "Next goal: \"\(remaining[0])\". "
            text += "Session auto-advanced \u{2014} call capture to continue, or finish again when done."
            if remaining.count > 1 {
                text += "\nRemaining after next: " +
                    remaining.dropFirst().map { "\"\($0)\"" }.joined(separator: ", ")
            }
        }
        return .text(text)
    }

    // MARK: - Explore Handler

    static func handleExplore(
        args: [String: JSONValue],
        session: ExplorationSession,
        registry: TargetRegistry,
        server: MCPServer
    ) -> MCPToolResult {
        guard let appName = args["app_name"]?.asString(), !appName.isEmpty else {
            return .error("Missing required parameter: app_name (for explore action)")
        }

        if session.active {
            return .error(
                "An exploration session is already active for '\(session.currentAppName)'. " +
                "Call finish first.")
        }

        let (ctx, err) = registry.resolveForTool(args)
        guard let ctx else { return err! }

        // Load APP.md description early — needed before launch to check reset_before_explore.
        let appDesc = AppDescriptionLoader.load(appName: appName)

        // Force-quit the app before launching if APP.md requests it.
        // Ensures a clean start for apps with overlays or stateful UI (TikTok, Instagram).
        if appDesc?.resetBeforeExplore == true {
            DebugLog.log("explore", "reset_before_explore: force-quitting '\(appName)' before launch")
            if let menuBridge = ctx.bridge as? (any MenuActionCapable) {
                _ = menuBridge.triggerMenuAction(menu: "View", item: "App Switcher")
                usleep(500_000)
                // Swipe up to dismiss the frontmost app card
                let preSize = ctx.bridge.getWindowInfo()?.size ?? CGSize(width: 410, height: 890)
                _ = ctx.input.swipe(
                    fromX: preSize.width / 2, fromY: preSize.height * 0.4,
                    toX: preSize.width / 2, toY: 0, durationMs: 300)
                usleep(500_000)
                _ = menuBridge.triggerMenuAction(menu: "View", item: "Home Screen")
                usleep(300_000)
            }
        }

        // Launch the app
        if let launchError = ctx.input.launchApp(name: appName) {
            return .error("Failed to launch '\(appName)': \(launchError)")
        }

        // Wait for Spotlight to dismiss and the app to become visible.
        guard let firstResult = SpotlightDetector.waitForDismissal(describer: ctx.describer) else {
            return .error(
                "'\(appName)' did not appear after launch — " +
                "Spotlight search may still be visible. Try launching the app manually first.")
        }

        // Parse budget overrides; merge skip elements from permissions.json and APP.md
        let maxDepth = args["max_depth"]?.asInt() ?? ExplorationBudget.default.maxDepth
        let maxScreens = args["max_screens"]?.asInt() ?? ExplorationBudget.default.maxScreens
        let maxTime = args["max_time"]?.asInt() ?? ExplorationBudget.default.maxTimeSeconds
        let extraPatterns = PermissionPolicy.loadConfig()?.skipElements ?? []
        let appSkipPatterns = appDesc?.skipElements ?? []
        let budget = ExplorationBudget(
            maxDepth: maxDepth,
            maxScreens: maxScreens,
            maxTimeSeconds: maxTime,
            maxActionsPerScreen: ExplorationBudget.default.maxActionsPerScreen,
            scrollLimit: ExplorationBudget.default.scrollLimit,
            skipPatterns: ExplorationBudget.builtInSkipPatterns + extraPatterns + appSkipPatterns
        )

        let goal = args["goal"]?.asString() ?? ""
        let fresh = args["fresh"]?.asBool() ?? true
        let seed = args["seed"]?.asInt().map { UInt64($0) }
        let skipCalibration = args["skip_calibration"]?.asBool() ?? false
        let explorerChoice = args["explorer"]?.asString() ?? "bfs"
        let explicitStrategy = args["strategy"]?.asString()
        let strategyChoice = StrategyDetector.detect(
            targetType: ctx.targetType,
            appName: appName,
            explicitStrategy: explicitStrategy
        )

        // Handle graph persistence: delete on fresh, log if existing
        if fresh {
            GraphPersistence.delete(bundleID: appName)
        } else if let existing = GraphPersistence.load(bundleID: appName) {
            DebugLog.log("explore", "Loaded persisted graph: \(existing.nodes.count) nodes, " +
                "\(existing.edges.count) edges, \(existing.deadEdges.count) dead edges")
        }

        session.start(appName: appName, goal: goal)
        session.setStrategy(strategyChoice.rawValue)
        if let desc = appDesc {
            session.setAppDescription(desc)
            DebugLog.log("explore",
                "APP.md loaded for '\(appName)': \(desc.skipElements.count) skip, " +
                "\(desc.obstacles.count) obstacles, mode=\(desc.obstacleMode.rawValue)")
        }

        // Capture first screen
        session.capture(
            elements: firstResult.elements, hints: firstResult.hints,
            icons: firstResult.icons, actionType: nil, arrivedVia: nil,
            screenshotBase64: firstResult.screenshotBase64
        )

        // Create explorer (BFS or DFS) and run exploration loop
        let windowSize = ctx.bridge.getWindowInfo()?.size ?? CGSize(width: 410, height: 890)
        let explorer: any Exploring
        if explorerChoice == "dfs" {
            explorer = DFSExplorer(
                session: session, budget: budget, windowSize: windowSize,
                backtracker: ctx.backtracker
            )
        } else {
            let componentDefinitions = ComponentLoader.loadAll()
            let recipeDefinitions = RecipeLoader.loadAll()

            // APP.md archetype: look up recipe by name and pre-set on session.
            // This bypasses auto-detection — the developer's declaration wins.
            if let archetypeName = appDesc?.archetype,
               let recipe = recipeDefinitions.first(where: { $0.name == archetypeName }) {
                let match = RecipeMatch(recipe: recipe, score: 100.0, reason: "APP.md archetype")
                session.setRecipeMatch(match)
                let refined = RecipeMatcher.strategyFromRecipe(recipe, fallback: strategyChoice)
                session.setStrategy(refined.rawValue)
                DebugLog.log("explore", "archetype from APP.md: '\(archetypeName)' " +
                    "(nav=\(recipe.navigationModel.type), strategy=\(refined.rawValue))")
            }
            let detectionMode = ComponentDetectionMode(rawValue: EnvConfig.componentDetection) ?? .llmFirstScreen
            let classifier = detectionMode.buildClassifier(server: server)
            let advisor: any ExplorationAdvising = EmbacleFFI.isAvailable
                ? VisionExplorationAdvisor() : HeuristicExplorationAdvisor()
            explorer = BFSExplorer(
                session: session, budget: budget, windowSize: windowSize,
                componentDefinitions: componentDefinitions,
                classifier: classifier,
                bridge: ctx.bridge,
                seed: seed,
                skipCalibration: skipCalibration,
                advisor: advisor,
                recipes: recipeDefinitions,
                backtracker: ctx.backtracker
            )
        }
        explorer.markStarted()

        var stepResults: [String] = [
            "Autonomous \(explorerChoice.uppercased()) exploration started for '\(appName)'.",
            "Budget: depth=\(maxDepth), screens=\(maxScreens), time=\(maxTime)s",
        ]

        // Run exploration loop using detected strategy
        while !explorer.completed {
            let result: ExploreStepResult
            switch strategyChoice {
            case .social:
                result = explorer.step(
                    describer: ctx.describer, input: ctx.input,
                    strategy: SocialAppStrategy.self)
            case .desktop:
                result = explorer.step(
                    describer: ctx.describer, input: ctx.input,
                    strategy: DesktopAppStrategy.self)
            case .mobile:
                result = explorer.step(
                    describer: ctx.describer, input: ctx.input,
                    strategy: MobileAppStrategy.self)
            }

            switch result {
            case .continue(let desc):
                stepResults.append(desc)
                DebugLog.log("explore", "step \(stepResults.count): \(desc)")
            case .backtracked(_, _):
                stepResults.append("Backtracked to parent screen.")
                DebugLog.log("explore", "step \(stepResults.count): backtracked")
            case .paused(let reason):
                stepResults.append("Paused: \(reason)")
                let stats = explorer.stats
                let summary = stepResults.joined(separator: "\n")
                let report = explorer.generateReport()
                // Persist partial graph for future incremental runs
                let snapshot = explorer.graph.finalize()
                GraphPersistence.save(snapshot: snapshot, bundleID: appName)
                return .text(
                    "\(summary)\n\nExploration paused after \(stats.actionCount) actions, " +
                    "\(stats.nodeCount) screens in \(stats.elapsedSeconds)s.\n\n\(report)")
            case .finished(let bundle):
                // Persist the completed graph for future incremental runs
                let snapshot = explorer.graph.finalize()
                GraphPersistence.save(snapshot: snapshot, bundleID: appName)
                return .text(
                    ExplorationResultFormatter.formatExploreResult(
                        bundle: bundle, explorer: explorer))
            }
        }

        // Should not reach here, but just in case
        return .text(stepResults.joined(separator: "\n"))
    }
}
