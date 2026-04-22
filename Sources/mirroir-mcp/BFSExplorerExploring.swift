// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: BFSExplorer `.exploring` phase — calibrate, plan, tap, classify transition, backtrack.
// ABOUTME: Extracted from BFSExplorer.swift to stay under the 500-line limit.

import Foundation
import HelperLib

extension BFSExplorer {

    /// Explore one element on the current frontier screen, tap back if it navigated.
    func stepExploring<S: ExplorationStrategy>(
        screen: FrontierScreen,
        describer: ScreenDescribing,
        input: InputProviding,
        strategy: S.Type
    ) -> ExploreStepResult {
        let currentFP = screen.fingerprint
        // OCR current screen, dismissing any system alert or app-specific obstacle
        let appObstacles = session.currentAppDescription?.obstacleMode == .auto
            ? (session.currentAppDescription?.obstacles ?? []) : []
        guard let result = ExplorerUtilities.dismissAlertIfPresent(
            describer: describer, input: input, obstacles: appObstacles
        ) else {
            return .paused(reason: "Failed to capture screen during exploration")
        }

        if let exit = handleContextEscape(elements: result.elements, input: input, describer: describer) { return exit }

        // Calibrate this screen if not already done: scroll full page to discover all
        // elements, then optionally run component detection + validation.
        var viewportElements = result.elements
        if !calibratedScreens.contains(currentFP) {
            let calResult = calibrateScreen(
                fingerprint: currentFP, describer: describer, input: input,
                skipComponentDetection: skipCalibration
            )
            calibratedScreens.insert(currentFP)
            switch calResult {
            case .failed(let reason):
                lock.lock(); isFinished = true; lock.unlock()
                return .paused(reason: reason)
            case .ok(let viewportMayHaveShifted):
                // Re-OCR only when calibration scrolled and found novel content.
                // The scroll-back may not land at exactly the original position,
                // so fresh elements prevent the resolver from scrolling unnecessarily.
                if viewportMayHaveShifted, let fresh = describer.describe() {
                    viewportElements = fresh.elements
                }
            }
        }
        // Log all OCR elements so we can compare with what's visible on screen
        let ocrTexts = viewportElements.map { "\($0.text)@(\(Int($0.tapX)),\(Int($0.tapY)))" }
        DebugLog.log("bfs", "OCR elements (\(viewportElements.count)): \(ocrTexts.joined(separator: ", "))")

        // Build plan from CURRENT viewport if none exists (per-viewport approach).
        if graph.screenPlan(for: currentFP) == nil {
            let canonicalElements = ExplorationRNG.canonicalOrder(viewportElements)
            let classified = ElementClassifier.classify(
                canonicalElements, budget: budget, screenHeight: windowSize.height
            )
            let visitedElements = graph.node(for: currentFP)?.visitedElements ?? []
            let plan = buildScreenPlan(
                classified: classified, visitedElements: visitedElements
            )
            graph.setScreenPlan(for: currentFP, plan: applyQBoostIfAvailable(plan: plan, fingerprint: currentFP))
            DebugLog.log("bfs", "=== VIEWPORT \(currentViewportIndex + 1)/\(totalViewpoints) ===")
            DebugLog.log("bfs", "viewport elements: \(viewportElements.count)")
            DebugLog.log("bfs", "components matched: \(plan.count)")
            let planTexts = plan.map { "\($0.displayLabel)(y=\(Int($0.point.tapY)), score=\(String(format: "%.1f", $0.score)))" }
            DebugLog.log("bfs", "click plan: \(planTexts)")
        }

        // Resolve next plan item against fresh viewport coordinates
        let rankedElement = resolveNextPlanItem(
            currentFP: currentFP, viewportElements: viewportElements,
            describer: describer, input: input, strategy: strategy
        )

        lock.lock()
        let currentActions = actionsOnCurrentScreen
        lock.unlock()

        let visited = graph.node(for: currentFP)?.visitedElements ?? []
        DebugLog.log("bfs", "exploring depth=\(screen.depth) fp=\(currentFP.prefix(8)) " +
            "actions=\(currentActions)/\(budget.maxActionsPerScreen) " +
            "visited=\(visited) next=\(rankedElement?.displayLabel ?? "nil")")

        guard let ranked = rankedElement, currentActions < budget.maxActionsPerScreen else {
            // Current viewport exhausted — scroll down to next viewport and rebuild plan.
            // Clear the plan so the next step builds a fresh one from the new viewport.
            if let scrollResult = performScrollIfAvailable(
                currentFP: currentFP, input: input, describer: describer
            ) {
                graph.clearScreenPlan(for: currentFP)
                lock.lock()
                currentViewportIndex += 1
                actionsOnCurrentScreen = 0
                lock.unlock()
                DebugLog.log("bfs", "=== VIEWPORT \(currentViewportIndex)/\(totalViewpoints) — scrolled down, plan cleared ===")
                return scrollResult
            }

            // Done with this screen — no more viewports to scroll to
            let visited = graph.node(for: currentFP)?.visitedElements ?? []
            DebugLog.log("bfs", "=== SCREEN DONE depth=\(screen.depth) visited=\(visited.count) items ===")
            if screen.depth == 0 {
                phase = .atRoot
            } else {
                phase = .returning(depthRemaining: screen.depth)
            }
            return .continue(description: "Finished exploring depth-\(screen.depth) screen")
        }

        let target = ranked.point
        let label = ranked.displayLabel

        // Global safe zone stencil: reject taps outside the app content area.
        // Status bar (y < 80pt) and home indicator zone are never valid tap targets.
        // Breadth navigation items (tab bar) are exempt from the bottom margin because
        // they are designed to sit at the very bottom of the screen.
        let safeMinY = LandmarkPicker.statusBarMaxY
        let safeMaxY = windowSize.height * 0.95
        let outsideBottom = !ranked.isBreadthNavigation && target.tapY > safeMaxY
        if target.tapY < safeMinY || outsideBottom
            || target.tapX < 0 || target.tapX > windowSize.width {
            DebugLog.log("bfs", "STENCIL \"\(label)\" at (\(Int(target.tapX)),\(Int(target.tapY))) — " +
                "outside safe zone (y: \(Int(safeMinY))–\(Int(safeMaxY))" +
                "\(ranked.isBreadthNavigation ? ", breadth-exempt" : ""))")
            graph.markElementVisited(fingerprint: currentFP, elementText: label)
            return .continue(description: "Skipped \"\(label)\" — outside safe zone")
        }

        // Check tap area cache — skip if we already tapped near these coordinates
        if graph.wasAlreadyTapped(fingerprint: currentFP, x: target.tapX, y: target.tapY) {
            DebugLog.log("bfs", "SKIP \"\(label)\" at (\(Int(target.tapX)),\(Int(target.tapY))) — " +
                "already tapped nearby (cache has \(graph.tapCount(for: currentFP)) entries)")
            graph.markElementVisited(fingerprint: currentFP, elementText: label)
            lock.lock()
            cacheHitsPerScreen[currentFP, default: 0] += 1
            screenActions[currentFP, default: []].append(
                ExplorationReportFormatter.ActionEntry(
                    label: label, x: target.tapX, y: target.tapY,
                    result: "cache_skip", skippedByCache: true))
            lock.unlock()
            return .continue(description: "Skipped \"\(label)\" — already tapped nearby")
        }

        // Mark visited using displayLabel (unique per component) to avoid
        // collisions when multiple components share the same raw text (e.g. "icon").
        graph.markElementVisited(fingerprint: currentFP, elementText: label)

        // Mark breadth_navigation components (e.g. tab bar items) as globally visited
        // so they are not re-tapped from every child screen.
        if graph.isBreadthLabel(label) {
            graph.markGloballyVisited(label: label)
            DebugLog.log("bfs", "globally visited breadth label: \"\(label)\"")
        }

        // Record tap coordinates in cache before tapping
        graph.recordTap(fingerprint: currentFP, x: target.tapX, y: target.tapY)

        // Tap the element and validate the result with vision
        let beforeElementCount = viewportElements.count
        _ = input.tap(x: target.tapX, y: target.tapY)
        usleep(EnvConfig.stepSettlingDelayMs * 1000)
        // OCR the resulting screen to validate the tap actually did something
        guard let afterResult = ExplorerUtilities.dismissAlertIfPresent(
            describer: describer, input: input
        ) else {
            return .paused(reason: "Failed to capture screen after tap")
        }
        DebugLog.log("bfs", "tap validation: \"\(label)\" — before=\(beforeElementCount) elements, " +
            "after=\(afterResult.elements.count) elements")

        // Re-check context after tap: if we accidentally triggered the home gesture,
        // detect it early. The improved AppContextDetector (nav-bar title + single-word
        // ratio filters) prevents false positives on chart/data screens.
        if let exit = handleContextEscape(elements: afterResult.elements, input: input, describer: describer) { return exit }

        let screenType = strategy.classifyScreen(
            elements: afterResult.elements, hints: afterResult.hints
        )

        // Classify edge type for intelligent backtracking, then record transition
        let edgeType = graph.node(for: currentFP).map { sourceNode in
            EdgeClassifier.classify(
                sourceNode: sourceNode, destinationElements: afterResult.elements,
                destinationHints: afterResult.hints, tappedElement: target,
                screenHeight: windowSize.height)
        } ?? .push
        let transition = graph.recordTransition(
            elements: afterResult.elements, icons: afterResult.icons,
            hints: afterResult.hints, screenshot: afterResult.screenshotBase64,
            actionType: "tap", elementText: target.text, displayLabel: label,
            screenType: screenType, edgeType: edgeType
        )

        // Record in session for flat screen list
        session.capture(
            elements: afterResult.elements, hints: afterResult.hints,
            icons: afterResult.icons, actionType: "tap",
            arrivedVia: target.text, displayLabel: label,
            screenshotBase64: afterResult.screenshotBase64,
            skipGraphTransition: true
        )

        lock.lock()
        actionCount += 1
        actionsOnCurrentScreen += 1
        lock.unlock()

        let transitionDesc: String
        switch transition {
        case .newScreen: transitionDesc = "new_screen"
        case .revisited: transitionDesc = "revisited"
        case .duplicate: transitionDesc = "no_navigation"
        }
        DebugLog.log("bfs", "tapped \"\(label)\" at (\(Int(target.tapX)),\(Int(target.tapY))) → \(transitionDesc)")
        // Update learned Q-value for this edge (Fastbot2 pattern)
        (graph as? NavigationGraph)?.updateQValue(
            fromFingerprint: currentFP, displayLabel: label, result: transition)
        lock.lock()
        screenActions[currentFP, default: []].append(
            ExplorationReportFormatter.ActionEntry(
                label: label, x: target.tapX, y: target.tapY,
                result: transitionDesc, skippedByCache: false))
        lock.unlock()

        switch transition {
        case .newScreen(let fp):
            coverageMonitor.recordDiscovery()
            let childDepth = screen.depth + 1
            if childDepth < budget.maxDepth && graph.nodeCount < budget.maxScreens {
                let newPath = screen.pathFromRoot + [PathSegment(
                    elementText: target.text, tapX: target.tapX, tapY: target.tapY
                )]
                frontier.append(FrontierScreen(
                    fingerprint: fp, pathFromRoot: newPath, depth: childDepth
                ))
            }

            // Tap back and verify we returned to the expected screen.
            DebugLog.log("bfs", "backtracking to \(currentFP.prefix(8)) after new screen")
            if let lostResult = tapBackAndVerify(
                expectedFP: currentFP, afterElements: afterResult.elements,
                describer: describer, input: input
            ) {
                DebugLog.log("bfs", "BACKTRACK FAILED — phase changing, remaining plan items lost")
                return lostResult
            }
            DebugLog.log("bfs", "backtrack OK — continuing on \(currentFP.prefix(8))")

            return .continue(
                description: "Tapped \"\(label)\" → new screen (\(graph.nodeCount) total)"
            )

        case .revisited:
            // Already-known screen — tap back, verify, don't re-explore
            DebugLog.log("bfs", "backtracking to \(currentFP.prefix(8)) after revisit")
            if let lostResult = tapBackAndVerify(
                expectedFP: currentFP, afterElements: afterResult.elements,
                describer: describer, input: input
            ) {
                DebugLog.log("bfs", "BACKTRACK FAILED on revisit — phase changing")
                return lostResult
            }

            return .continue(description: "Tapped \"\(label)\" → revisited screen")

        case .duplicate:
            // Mark this edge as dead so future exploration plans skip it
            graph.markEdgeDead(fromFingerprint: currentFP, displayLabel: label)
            graph.appendRecoveryEvent(PostActionVerifier.buildEvent(
                category: .deadTap,
                screenFingerprint: currentFP,
                description: "Tapped \"\(label)\" but screen did not change"
            ))
            DebugLog.log("bfs", "dead tap: \"\(label)\" on \(currentFP.prefix(8))")
            return .continue(description: "Tapped \"\(label)\" → dead tap (marked)")
        }
    }
}
