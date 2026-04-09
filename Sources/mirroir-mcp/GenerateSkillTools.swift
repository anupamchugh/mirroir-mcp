// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Registers the generate_skill MCP tool for AI-driven app exploration.
// ABOUTME: Session-based workflow: start (launch + OCR) -> capture (OCR + guidance) -> finish (emit SKILL.md).

import Foundation
import HelperLib

extension MirroirMCP {
    static func registerGenerateSkillTools(
        server: MCPServer,
        registry: TargetRegistry
    ) {
        let session = ExplorationSession()

        server.registerTool(MCPToolDefinition(
            name: "generate_skill",
            description: """
                Generate a SKILL.md by exploring an app. Session-based workflow: \
                (1) action="start" \u{2014} launch app + OCR. \
                (2) Navigate with tap/swipe/type_text, then action="capture" per screen. \
                (3) action="finish" \u{2014} emit SKILL.md. \
                Use action="explore" for autonomous exploration (BFS default, or DFS). \
                Set fresh=true to discard persisted graph and explore from scratch. \
                WARNING: Exploration steals Mac keyboard focus (global HID events). \
                SECURITY: May navigate into sensitive screens. Do not run unattended.
                """,
            inputSchema: [
                "type": .string("object"),
                "properties": .object([
                    "action": .object([
                        "type": .string("string"),
                        "description": .string(
                            "Session action: \"start\" to launch app and begin, " +
                            "\"capture\" to OCR current screen and append, " +
                            "\"finish\" to generate SKILL.md from all captures, " +
                            "\"explore\" for autonomous exploration (BFS or DFS)."),
                        "enum": .array([
                            .string("start"),
                            .string("capture"),
                            .string("finish"),
                            .string("explore"),
                        ]),
                    ]),
                    "app_name": .object([
                        "type": .string("string"),
                        "description": .string(
                            "App to explore (required for start action)."),
                    ]),
                    "goal": .object([
                        "type": .string("string"),
                        "description": .string(
                            "Optional flow description, e.g. \"check software version\" (for start action). " +
                            "Omit for discovery mode."),
                    ]),
                    "goals": .object([
                        "type": .string("array"),
                        "items": .object(["type": .string("string")]),
                        "description": .string(
                            "Optional array of goals for manifest mode. " +
                            "Each goal is explored in sequence, producing one SKILL.md per goal."),
                    ]),
                    "arrived_via": .object([
                        "type": .string("string"),
                        "description": .string(
                            "Element tapped to reach current screen, e.g. \"General\" (for capture action)."),
                    ]),
                    "action_type": .object([
                        "type": .string("string"),
                        "description": .string(
                            "Action performed to reach current screen: " +
                            "\"tap\", \"swipe\", \"type\", \"press_key\", \"scroll_to\", " +
                            "\"long_press\", \"remember\", \"screenshot\", \"press_home\", " +
                            "\"open_url\" (for capture action)."),
                    ]),
                    "max_depth": .object([
                        "type": .string("integer"),
                        "description": .string(
                            "Maximum BFS depth for explore action (default: 6)."),
                    ]),
                    "max_screens": .object([
                        "type": .string("integer"),
                        "description": .string(
                            "Maximum screens to visit for explore action (default: 30)."),
                    ]),
                    "max_time": .object([
                        "type": .string("integer"),
                        "description": .string(
                            "Maximum seconds for explore action (default: 300)."),
                    ]),
                    "strategy": .object([
                        "type": .string("string"),
                        "description": .string(
                            "Override exploration strategy: \"mobile\" (default), " +
                            "\"social\" (Reddit, Instagram, TikTok), " +
                            "\"desktop\" (generic macOS windows). " +
                            "Auto-detected from target type and app name if omitted."),
                        "enum": .array([
                            .string("mobile"),
                            .string("social"),
                            .string("desktop"),
                        ]),
                    ]),
                    "fresh": .object([
                        "type": .string("boolean"),
                        "description": .string(
                            "When true, discard any persisted navigation graph and " +
                            "explore from scratch. Default: true. Set false for incremental exploration."),
                    ]),
                    "seed": .object([
                        "type": .string("integer"),
                        "description": .string(
                            "Seed for deterministic exploration ordering. " +
                            "Same seed produces identical exploration sequences."),
                    ]),
                    "skip_calibration": .object([
                        "type": .string("boolean"),
                        "description": .string(
                            "Skip component detection and validation during calibration. " +
                            "Full-page scrolling still runs to discover below-fold elements. " +
                            "Useful with vision describers that produce clean semantic elements. Default: false."),
                    ]),
                    "explorer": .object([
                        "type": .string("string"),
                        "description": .string(
                            "Exploration algorithm: \"bfs\" (breadth-first, default) " +
                            "or \"dfs\" (depth-first)."),
                        "enum": .array([
                            .string("bfs"),
                            .string("dfs"),
                        ]),
                    ]),
                ]),
                "required": .array([.string("action")]),
            ],
            handler: { args in
                guard let action = args["action"]?.asString() else {
                    return .error("Missing required parameter: action")
                }

                switch action {
                case "start":
                    return handleStart(args: args, session: session, registry: registry)
                case "capture":
                    return handleCapture(args: args, session: session, registry: registry)
                case "finish":
                    return handleFinish(session: session)
                case "explore":
                    return handleExplore(args: args, session: session, registry: registry, server: server)
                default:
                    return .error("Unknown action '\(action)'. Use: start, capture, finish, explore.")
                }
            }
        ))
    }

    // MARK: - Action Handlers

    private static func handleStart(
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
            isMobile: ctx.targetType == "iphone-mirroring"
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

    private static func handleCapture(
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
                isMobile: ctx.targetType == "iphone-mirroring"
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
            isMobile: ctx.targetType == "iphone-mirroring"
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

    private static func handleFinish(session: ExplorationSession) -> MCPToolResult {
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

        var text = ExplorationResultFormatter.formatBundle(bundle, preamble: "Generated \(bundle.skills.count) skills from exploration:")
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

    private static func handleExplore(
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

        // Launch the app
        if let launchError = ctx.input.launchApp(name: appName) {
            return .error("Failed to launch '\(appName)': \(launchError)")
        }

        // Wait for Spotlight to dismiss and the app to become visible.
        // launchApp uses Spotlight search which may linger after pressing Return.
        guard let firstResult = SpotlightDetector.waitForDismissal(describer: ctx.describer) else {
            return .error(
                "'\(appName)' did not appear after launch — " +
                "Spotlight search may still be visible. Try launching the app manually first.")
        }

        // Load APP.md description for this app (if available).
        // Stored on session AFTER start() to avoid being cleared by start()'s reset.
        let appDesc = AppDescriptionLoader.load(appName: appName)

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
                session: session, budget: budget, windowSize: windowSize
            )
        } else {
            let componentDefinitions = ComponentLoader.loadAll()
            let recipeDefinitions = RecipeLoader.loadAll()
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
                recipes: recipeDefinitions
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
                return .text(ExplorationResultFormatter.formatExploreResult(bundle: bundle, explorer: explorer))
            }
        }

        // Should not reach here, but just in case
        return .text(stepResults.joined(separator: "\n"))
    }

}
