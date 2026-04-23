// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Post-backtrack verification, modal recovery, and plateau advisory for BFS exploration.
// ABOUTME: Detects when the explorer is lost and attempts recovery before cascading errors.

import Foundation
import HelperLib

/// Result of post-backtrack position verification.
enum BacktrackVerification {
    /// Successfully returned to the expected screen.
    case verified
    /// Returned to a different known screen — graph state corrected.
    case corrected(fingerprint: String)
    /// Explorer is lost — could not identify current screen after recovery attempts.
    case lost
}

extension BFSExplorer {

    // MARK: - Post-Backtrack Verification

    /// Modal dismiss patterns for recovery when the explorer is stuck on a modal sheet.
    /// Includes English and French patterns for iOS Health/Santé app compatibility.
    static let modalDismissPatterns: Set<String> = {
        var patterns = ElementClassifier.dismissCharacters
        patterns.formUnion(MobileAppStrategy.modalDismissPatterns)
        patterns.formUnion(["fermer", "annuler", "terminé"])
        return patterns
    }()

    /// Verify the explorer returned to the expected screen after tapping back.
    /// If the screen doesn't match, attempts recovery in sequence:
    /// 1. Dismiss modal (X, Close, Done, Fermer, Annuler) in top zone
    /// 2. Retry tapBackButton
    /// 3. Match against all known screens to correct graph state
    ///
    /// Prevents cascading errors when the explorer gets stuck on an unexpected
    /// screen (modal articles, deep sub-views, system sheets).
    func verifyBacktrack(
        expectedFP: String,
        afterElements: [TapPoint],
        describer: ScreenDescribing,
        input: InputProviding
    ) -> BacktrackVerification {
        // OCR the screen after backtrack
        guard let result = ExplorerUtilities.dismissAlertIfPresent(
            describer: describer, input: input
        ) else {
            DebugLog.log("bfs", "backtrack-verify: OCR failed after back tap")
            return .lost
        }

        // Check if we landed on the expected screen.
        // Two-tier check: Jaccard similarity first (fast, strict), then viewport containment
        // (handles scrolled viewports where Jaccard fails because the viewport is a subset
        // of the full calibrated element set).
        if let expectedNode = graph.node(for: expectedFP) {
            if StructuralFingerprint.areEquivalentTitleAware(expectedNode.elements, result.elements) {
                return .verified
            }
            if StructuralFingerprint.viewportContainedIn(
                viewport: result.elements, reference: expectedNode.elements
            ) {
                DebugLog.log("bfs", "backtrack-verify: viewport contained in expected screen (containment match)")
                return .verified
            }
        }

        let ocrTexts = result.elements.map { "\($0.text)@(\(Int($0.tapX)),\(Int($0.tapY)))" }
        DebugLog.log("bfs", "backtrack-verify: screen mismatch — " +
            "\(result.elements.count) elements: \(ocrTexts.joined(separator: ", "))")

        // Recovery 1: Try dismissing a modal (X, Close, Done in top 30% zone).
        // App Store modal sheets place the X at ~25% height, so 20% is too narrow.
        // Two-pass: prefer explicit text matches ("x", "close", "done") over generic
        // YOLO "icon" labels, which can collide with status bar icons.
        let topZone = windowSize.height * 0.30
        let rightHalf = windowSize.width * 0.5
        let statusBarCutoff = windowSize.height * 0.12
        let dismissButton: TapPoint? = {
            // Pass 1: explicit dismiss text (highest confidence)
            if let textMatch = result.elements.first(where: { el in
                guard el.tapY <= topZone else { return false }
                let text = el.text.trimmingCharacters(in: .whitespaces).lowercased()
                return Self.modalDismissPatterns.contains(text)
            }) { return textMatch }
            // Pass 2: YOLO "icon" in top-right, below the status bar
            return result.elements.first(where: { el in
                guard el.tapY <= topZone && el.tapY >= statusBarCutoff else { return false }
                let text = el.text.trimmingCharacters(in: .whitespaces).lowercased()
                return text == "icon" && el.tapX >= rightHalf
            })
        }()
        if let dismissButton {
            DebugLog.log("bfs", "backtrack-verify: tapping dismiss \"\(dismissButton.text)\" " +
                "at (\(Int(dismissButton.tapX)),\(Int(dismissButton.tapY)))")
            _ = input.tap(x: dismissButton.tapX, y: dismissButton.tapY)
            usleep(EnvConfig.stepSettlingDelayMs * 1000)

            if let afterDismiss = ExplorerUtilities.dismissAlertIfPresent(
                describer: describer, input: input
            ) {
                if let expectedNode = graph.node(for: expectedFP),
                   (StructuralFingerprint.areEquivalentTitleAware(
                       expectedNode.elements, afterDismiss.elements
                   ) || StructuralFingerprint.viewportContainedIn(
                       viewport: afterDismiss.elements, reference: expectedNode.elements
                   )) {
                    DebugLog.log("bfs", "backtrack-verify: modal dismiss recovered to expected screen")
                    return .verified
                }
                // Modal dismissed but landed on a different known screen
                if let matchedFP = graph.findMatchingNode(elements: afterDismiss.elements) {
                    DebugLog.log("bfs", "backtrack-verify: modal dismiss → known screen \(matchedFP.prefix(8))")
                    return .corrected(fingerprint: matchedFP)
                }
            }
        }

        // Recovery 2: Retry back button with fresh elements
        DebugLog.log("bfs", "backtrack-verify: retrying back button")
        backtracker.tapBack(
            elements: result.elements, input: input, windowSize: windowSize
        )

        guard let retryResult = ExplorerUtilities.dismissAlertIfPresent(
            describer: describer, input: input
        ) else {
            return .lost
        }

        if let expectedNode = graph.node(for: expectedFP),
           (StructuralFingerprint.areEquivalentTitleAware(
               expectedNode.elements, retryResult.elements
           ) || StructuralFingerprint.viewportContainedIn(
               viewport: retryResult.elements, reference: expectedNode.elements
           )) {
            DebugLog.log("bfs", "backtrack-verify: retry succeeded")
            return .verified
        }

        // Recovery 3: Match against any known screen
        if let matchedFP = graph.findMatchingNode(elements: retryResult.elements) {
            DebugLog.log("bfs", "backtrack-verify: landed on known screen \(matchedFP.prefix(8))")
            return .corrected(fingerprint: matchedFP)
        }

        // Recovery 4: Try a third back tap for multi-level detail hierarchies.
        // Some screens (e.g. Health > Activité) have 2+ levels of navigation
        // that require multiple back taps to reach the parent.
        let retryTexts = retryResult.elements.map { $0.text }.joined(separator: ", ")
        DebugLog.log("bfs", "backtrack-verify: still lost after 2 backs — trying 3rd back " +
            "(current: \(retryTexts.prefix(100)))")
        backtracker.tapBack(
            elements: retryResult.elements, input: input, windowSize: windowSize
        )
        guard let thirdResult = ExplorerUtilities.dismissAlertIfPresent(
            describer: describer, input: input
        ) else {
            DebugLog.log("bfs", "backtrack-verify: LOST — OCR failed on 3rd attempt")
            return .lost
        }
        if let expectedNode = graph.node(for: expectedFP),
           (StructuralFingerprint.areEquivalentTitleAware(
               expectedNode.elements, thirdResult.elements
           ) || StructuralFingerprint.viewportContainedIn(
               viewport: thirdResult.elements, reference: expectedNode.elements
           )) {
            DebugLog.log("bfs", "backtrack-verify: 3rd back succeeded!")
            return .verified
        }
        if let matchedFP = graph.findMatchingNode(elements: thirdResult.elements) {
            DebugLog.log("bfs", "backtrack-verify: 3rd back → known screen \(matchedFP.prefix(8))")
            return .corrected(fingerprint: matchedFP)
        }

        DebugLog.log("bfs", "backtrack-verify: LOST — unknown screen after 3 recovery attempts")
        return .lost
    }

    /// Tap back and verify the result. Returns an ExploreStepResult if the explorer
    /// is lost (should stop exploring), or nil if backtrack succeeded.
    func tapBackAndVerify(
        expectedFP: String,
        afterElements: [TapPoint],
        describer: ScreenDescribing,
        input: InputProviding
    ) -> ExploreStepResult? {
        backtracker.tapBack(
            elements: afterElements, input: input, windowSize: windowSize
        )

        let verification = verifyBacktrack(
            expectedFP: expectedFP, afterElements: afterElements,
            describer: describer, input: input
        )

        switch verification {
        case .verified:
            graph.setCurrentFingerprint(expectedFP)
            return nil

        case .corrected(let actualFP):
            graph.setCurrentFingerprint(actualFP)
            DebugLog.log("bfs", "backtrack corrected: expected \(expectedFP.prefix(8)) " +
                "→ actual \(actualFP.prefix(8))")
            // If we landed back on the root screen, that's recoverable — the explorer
            // can continue from root. But if we landed on a non-root screen that isn't
            // the expected parent, continuing would tap elements on the wrong screen.
            if actualFP != graph.rootFingerprint && actualFP != expectedFP {
                // Landed on a different known screen — relaunch to recover
                DebugLog.log("bfs", "backtrack corrected to non-root screen — relaunching \(appName)")
                _ = input.launchApp(name: appName)
                usleep(EnvConfig.toolSettlingDelayUs)
                graph.setCurrentFingerprint(graph.rootFingerprint)
                frontierManager.phase = .atRoot
                return .continue(description: "Backtrack landed on wrong screen — relaunched app")
            }
            return nil

        case .lost:
            // Force relaunch the app to recover from stuck/modal state.
            DebugLog.log("bfs", "LOST — relaunching \(appName) to recover")
            _ = input.launchApp(name: appName)
            usleep(EnvConfig.toolSettlingDelayUs)
            graph.setCurrentFingerprint(graph.rootFingerprint)
            // If we were exploring root, stay in exploring phase to continue the plan.
            // Don't switch to .atRoot which would dequeue frontier children.
            if expectedFP != graph.rootFingerprint {
                frontierManager.phase = .atRoot
            }
            DebugLog.log("bfs", "recovered — phase=\(frontierManager.phase)")
            graph.appendRecoveryEvent(PostActionVerifier.buildEvent(
                category: .backtrackFailed,
                screenFingerprint: expectedFP,
                description: "Lost after backtrack — relaunched app to recover"
            ))
            return .continue(description: "Lost after backtrack — relaunched app, continuing")
        }
    }

    // MARK: - Phase: Returning

    /// Tap back one level toward root. Each step reduces depth by one.
    func stepReturning(
        depthRemaining: Int,
        describer: ScreenDescribing,
        input: InputProviding
    ) -> ExploreStepResult {
        // Get current screen elements for back button detection
        let elements: [TapPoint]
        if let result = ExplorerUtilities.dismissAlertIfPresent(
            describer: describer, input: input
        ) {
            elements = result.elements
        } else {
            elements = []
        }

        // Use edge-type-aware backtracking: check what kind of edge
        // brought us to this screen and reverse accordingly.
        let currentFP = graph.currentFingerprint
        let incomingEdge = graph.incomingEdge(to: currentFP)
        var handled = false

        if let edge = incomingEdge {
            switch edge.edgeType {
            case .modal:
                if let dismiss = EdgeClassifier.findDismissTarget(
                    elements: elements, screenHeight: windowSize.height
                ) {
                    _ = input.tap(x: dismiss.tapX, y: dismiss.tapY)
                    usleep(EnvConfig.stepSettlingDelayMs * 1000)
                    handled = true
                }
            case .tab:
                // For tabs, find the source screen's tab element
                if let sourceNode = graph.node(for: edge.fromFingerprint) {
                    let tabBarZone = windowSize.height * EdgeClassifier.tabBarZoneFraction
                    if let tabElement = sourceNode.elements.first(where: { $0.tapY >= tabBarZone }) {
                        _ = input.tap(x: tabElement.tapX, y: tabElement.tapY)
                        usleep(EnvConfig.stepSettlingDelayMs * 1000)
                        handled = true
                    }
                }
            case .toggle:
                // Toggle doesn't need backtrack action, just proceed
                handled = true
            case .external, .dead:
                _ = input.pressKey(keyName: "h", modifiers: ["command", "shift"])
                usleep(EnvConfig.stepSettlingDelayMs * 1000)
                handled = true
            case .push, .same:
                break
            }
        }

        if !handled {
            backtracker.tapBack(
                elements: elements, input: input, windowSize: windowSize
            )
        }

        let remaining = depthRemaining - 1
        if remaining > 0 {
            frontierManager.phase = .returning(depthRemaining: remaining)
        } else {
            frontierManager.phase = .atRoot
            graph.setCurrentFingerprint(graph.rootFingerprint)
        }

        return .continue(
            description: "Returning to root (\(remaining) level\(remaining == 1 ? "" : "s") remaining)"
        )
    }

    // MARK: - Accessors

    /// Whether the exploration has completed.
    var completed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return isFinished
    }

    var stats: (nodeCount: Int, edgeCount: Int, actionCount: Int, elapsedSeconds: Int) {
        lock.lock(); defer { lock.unlock() }
        return (graph.nodeCount, graph.edgeCount, actionCount, Int(Date().timeIntervalSince(startTime)))
    }

    // MARK: - Bundle Generation

    func generateBundle() -> SkillBundle {
        let recipeMatch = session.currentRecipeMatch
        let appDesc = session.currentAppDescription
        guard let data = session.finalize() else {
            return SkillBundle(appName: "", skills: [], manifest: nil)
        }
        return SkillBundleGenerator.generate(
            appName: data.appName, goal: data.goal,
            snapshot: data.graphSnapshot, allScreens: data.screens,
            recipeMatch: recipeMatch, appDescription: appDesc
        )
    }

    // MARK: - Q-Value Boost

    /// Apply learned Q-value boosts to a plan if the graph has persisted edge data.
    /// Returns the original plan unchanged if no Q-values are available.
    func applyQBoostIfAvailable(plan: [RankedElement], fingerprint: String) -> [RankedElement] {
        guard let navGraph = graph as? NavigationGraph else { return plan }
        let qValues = Dictionary(
            (navGraph.adjacency[fingerprint] ?? []).map { ($0.displayLabel, $0.qValue) },
            uniquingKeysWith: { _, last in last }
        )
        return ScreenPlanner.applyQBoost(plan: plan, qValues: qValues)
    }

    // MARK: - Plateau Advisory

    /// Maximum advisor attempts per screen before giving up.
    static let maxAdvisorAttemptsPerScreen = 2

    /// Ask the AI advisor for guidance when the plan is exhausted and coverage
    /// has plateaued. Returns nil if no advisor is configured, we're not in
    /// plateau phase, the advisor has no actionable suggestion, or max attempts
    /// on this screen have been reached.
    func tryPlateauAdvisor(
        fingerprint: String,
        screenshotBase64: String,
        viewportElements: [TapPoint],
        input: InputProviding
    ) -> ExploreStepResult? {
        guard coverageMonitor.currentPhase == .plateau, let advisor = advisor else {
            return nil
        }
        // Limit advisor attempts per screen to prevent infinite loops
        let advisorKey = "advisor:\(fingerprint)"
        let attempts = (graph.node(for: fingerprint)?.visitedElements
            .filter { $0.hasPrefix("advisor:") }.count) ?? 0
        guard attempts < Self.maxAdvisorAttemptsPerScreen else {
            DebugLog.log("bfs", "plateau advisor: max attempts (\(Self.maxAdvisorAttemptsPerScreen)) reached for \(fingerprint.prefix(8))")
            return nil
        }
        let visited = graph.node(for: fingerprint)?.visitedElements ?? []
        let suggestions = advisor.suggest(
            screenshotBase64: screenshotBase64,
            elements: viewportElements,
            visitedElements: visited,
            exploredScreenCount: graph.nodeCount
        )
        coverageMonitor.recordLLMAction()
        // Track advisor attempt regardless of outcome
        graph.markElementVisited(fingerprint: fingerprint, elementText: advisorKey)
        guard let suggestion = suggestions.first,
              let target = viewportElements.first(where: { $0.text == suggestion.elementText }) else {
            return nil
        }
        DebugLog.log("bfs", "plateau advisor: \"\(suggestion.elementText)\" " +
            "(\(suggestion.reasoning), confidence=\(suggestion.confidence))")
        graph.markElementVisited(fingerprint: fingerprint, elementText: target.text)
        _ = input.tap(x: target.tapX, y: target.tapY)
        usleep(EnvConfig.stepSettlingDelayMs * 1000)
        lock.lock(); actionCount += 1; lock.unlock()
        frontierManager.incrementActionsOnCurrentScreen()
        return .continue(
            description: "Plateau advisor: tapped \"\(suggestion.elementText)\""
        )
    }
}
