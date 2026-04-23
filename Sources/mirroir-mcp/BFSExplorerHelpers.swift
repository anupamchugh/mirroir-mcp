// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Helper methods for BFSExplorer: calibration pipeline, plan resolution, scroll support.
// ABOUTME: Split from BFSExplorer.swift to stay under the 500-line limit.

import Foundation
import HelperLib

/// Result of a screen calibration attempt.
enum CalibrationResult {
    /// Calibration succeeded and plan was built.
    /// `viewportMayHaveShifted` is true when calibration scrolled and discovered
    /// novel content — the caller should re-OCR to get fresh viewport coordinates
    /// because the scroll-back may not land at exactly the same position.
    case ok(viewportMayHaveShifted: Bool)
    /// Calibration validation failed (strict mode, too many unclassified elements).
    case failed(String)
}

extension BFSExplorer {

    /// Check if the explorer has left the target app and handle accordingly.
    /// Returns an ExploreStepResult to return early, or nil if still in-app.
    ///
    /// Short-circuits when the viewport matches a known graph node (via Jaccard or
    /// containment). This prevents false home-screen positives on root screens like
    /// Santé's dashboard, which has many short labels in a grid-like layout but no
    /// back chevron.
    func handleContextEscape(
        elements: [TapPoint], input: InputProviding, describer: ScreenDescribing
    ) -> ExploreStepResult? {
        // If the viewport matches any known graph node, we're still in the app.
        // This avoids false home-screen positives on root screens with grid-like layouts
        // (e.g. Santé dashboard: many short labels, no back chevron).
        if graph.findMatchingNodeWithContainment(elements: elements) != nil {
            return nil
        }

        let check = ExplorerUtilities.verifyAppContext(
            elements: elements, screenHeight: windowSize.height,
            appName: appName, input: input, describer: describer)
        switch check {
        case .ok: return nil
        case .recovered:
            graph.appendRecoveryEvent(PostActionVerifier.buildEvent(
                category: .appRelaunched,
                screenFingerprint: graph.currentFingerprint,
                description: "App escaped during BFS exploration, relaunched"
            ))
            // App was relaunched — reset to root and continue exploring
            graph.setCurrentFingerprint(graph.rootFingerprint)
            frontierManager.phase = .atRoot
            return .continue(description: "App escaped — relaunched and continuing from root")
        case .failed(let reason):
            graph.appendRecoveryEvent(PostActionVerifier.buildEvent(
                category: .appEscape,
                screenFingerprint: graph.currentFingerprint,
                description: "App escaped during BFS exploration: \(reason)"
            ))
            // Force-quit and relaunch the app to recover from stuck state
            DebugLog.log("bfs", "context escape failed — resetting app \(appName)")
            _ = input.launchApp(name: appName)
            usleep(EnvConfig.toolSettlingDelayUs)
            graph.setCurrentFingerprint(graph.rootFingerprint)
            frontierManager.phase = .atRoot
            return .continue(description: "App stuck — reset and continuing from root")
        }
    }

    /// Build a screen plan using component detection or legacy per-element classification.
    /// When the plan is empty and the session has APP.md tabs, injects tab elements
    /// as high-priority navigation targets (tab-driven navigation).
    func buildScreenPlan(
        classified: [ClassifiedElement], visitedElements: Set<String>
    ) -> [RankedElement] {
        var plan: [RankedElement]

        if componentDefinitions.isEmpty {
            DebugLog.log("bfs", "plan-path: LEGACY (0 component definitions)")
            plan = ScreenPlanner.buildPlan(
                classified: classified, visitedElements: visitedElements,
                scoutResults: [:], screenHeight: windowSize.height)
        } else {
            let rawComponents = classifier?.classify(
                classified: classified, definitions: componentDefinitions,
                screenHeight: windowSize.height
            ) ?? ComponentDetector.detect(
                classified: classified, definitions: componentDefinitions,
                screenHeight: windowSize.height)
            let components = ComponentDetector.applyAbsorption(rawComponents)
            DebugLog.log("bfs", "plan-path: COMPONENT (\(componentDefinitions.count) defs, " +
                "\(rawComponents.count) raw, \(components.count) absorbed)")
            let explorableCount = components.filter { $0.definition.exploration.explorable }.count
            DebugLog.log("bfs", "explorable: \(explorableCount)/\(components.count) components")
            plan = ScreenPlanner.buildComponentPlan(
                components: components, visitedElements: visitedElements,
                scoutResults: [:], screenHeight: windowSize.height)
        }

        // Tab-driven navigation: when APP.md declares tabs, inject matching OCR
        // elements as top-priority breadth navigation targets. This ensures tab
        // exploration works even in apps with non-standard UI (TikTok, Instagram)
        // where component detection can't classify the tab bar.
        let appTabs = session.currentAppDescription?.tabs ?? []
        if !appTabs.isEmpty {
            let allPoints = classified.map { $0.point }
            let tabTargets = findTabTargets(
                tabNames: appTabs, elements: allPoints, visitedElements: visitedElements)
            if !tabTargets.isEmpty {
                DebugLog.log("bfs", "tab-driven: injected \(tabTargets.count) tab targets " +
                    "from APP.md: \(tabTargets.map { $0.displayLabel })")
                plan = tabTargets + plan
            }
        }

        return plan
    }

    /// Find OCR elements matching APP.md tab names.
    ///
    /// Primary strategy: case-insensitive substring text match (works when tab bar has labels).
    /// Fallback: ordinal position mapping inside the tab-bar zone (for icon-only tab bars).
    /// The layout hint from APP.md (`TabLayout.orientation`, `TabLayout.edge`) decides which
    /// window edge to sample and whether to sort by X (horizontal bars) or Y (vertical rails).
    /// When no layout hint is given, the legacy bottom/horizontal convention applies.
    private func findTabTargets(
        tabNames: [String], elements: [TapPoint], visitedElements: Set<String>
    ) -> [RankedElement] {
        var results: [RankedElement] = []
        var matchedTabs: Set<String> = []

        // Strategy 1: text matching
        for tabName in tabNames {
            let tabLower = tabName.lowercased()
            guard let match = elements.first(where: { el in
                let elLower = el.text.lowercased()
                return (elLower == tabLower || elLower.contains(tabLower) || tabLower.contains(elLower))
                    && !visitedElements.contains(el.text)
                    && !visitedElements.contains(tabName)
            }) else { continue }

            results.append(RankedElement(
                point: match,
                score: ScreenPlanner.breadthRoleWeight + ScreenPlanner.highPriorityWeight,
                reason: "tab-driven(\(tabName))",
                displayLabel: tabName,
                isBreadthNavigation: true
            ))
            matchedTabs.insert(tabName)
        }

        // Strategy 2: ordinal mapping for unmatched tabs (icon-only tab bars).
        // Layout hint drives which window edge to sample and which axis to sort on.
        let unmatchedTabs = tabNames.filter { !matchedTabs.contains($0) }
        if !unmatchedTabs.isEmpty {
            let layout = session.currentAppDescription?.tabLayout
                ?? TabLayout(orientation: .horizontal, edge: .bottom)
            let zoneElements = elementsInTabZone(elements: elements, layout: layout)
            let sorted: [TapPoint]
            switch layout.orientation {
            case .horizontal:
                sorted = zoneElements.sorted { $0.tapX < $1.tapX }
            case .vertical:
                sorted = zoneElements.sorted { $0.tapY < $1.tapY }
            }

            if sorted.count >= tabNames.count {
                for (index, tabName) in tabNames.enumerated() {
                    guard !matchedTabs.contains(tabName),
                          index < sorted.count,
                          !visitedElements.contains(tabName) else { continue }
                    let target = sorted[index]
                    results.append(RankedElement(
                        point: target,
                        score: ScreenPlanner.breadthRoleWeight + ScreenPlanner.highPriorityWeight,
                        reason: "tab-ordinal(\(tabName)@\(index),\(layout.orientation.rawValue))",
                        displayLabel: tabName,
                        isBreadthNavigation: true
                    ))
                }
            }
        }

        return results
    }

    /// Filter OCR elements to those inside the declared tab-bar zone.
    /// Zone widths are 12% of the relevant axis — consistent with the existing
    /// bottom-edge convention used for iPhone portrait tab bars.
    private func elementsInTabZone(
        elements: [TapPoint], layout: TabLayout
    ) -> [TapPoint] {
        let band: Double = 0.12
        switch layout.edge {
        case .bottom:
            let minY = windowSize.height * (1 - band)
            return elements.filter { $0.tapY >= minY }
        case .top:
            let maxY = windowSize.height * band
            return elements.filter { $0.tapY <= maxY }
        case .right:
            let minX = windowSize.width * (1 - band)
            return elements.filter { $0.tapX >= minX }
        case .left:
            let maxX = windowSize.width * band
            return elements.filter { $0.tapX <= maxX }
        }
    }

    // MARK: - Calibration Pipeline

    /// Calibrate a screen: scroll full page, then build an exploration plan.
    ///
    /// When `skipComponentDetection` is false (default), runs the full pipeline:
    /// scroll → component detection → validation → component plan.
    /// When true, skips component matching and the validation gate, building
    /// the plan directly from classified elements (for vision describers).
    func calibrateScreen(
        fingerprint: String, describer: ScreenDescribing, input: InputProviding,
        skipComponentDetection: Bool = false
    ) -> CalibrationResult {
        // Stage 1: Scroll full page and collect all elements (always runs).
        let scrollData = scrollAndCollect(fingerprint: fingerprint, describer: describer, input: input)

        let allElements = graph.node(for: fingerprint)?.elements ?? []
        guard !allElements.isEmpty else {
            DebugLog.log("bfs", "calibration: no elements after scroll — skipping detection")
            return .ok(viewportMayHaveShifted: false)
        }

        let scrolledWithNovelContent = scrollData.scrollCount > 0 && scrollData.novelCount > 0

        // Stage 2: Classify all elements as interactive/non-interactive.
        let classified = ElementClassifier.classify(
            allElements, budget: budget, screenHeight: windowSize.height
        )

        // Fast path: skip component detection. Do NOT build a full-page plan here.
        // The explorer will build per-viewport plans from fresh OCR, processing one
        // viewport at a time (scroll → describe → classify → tap → next viewport).
        if skipComponentDetection || componentDefinitions.isEmpty {
            // Record viewpoint count for per-viewport processing.
            // +1 for the initial viewport captured before any scroll.
            frontierManager.resetViewports(total: scrollData.scrollCount + 1)

            DebugLog.log("bfs", "=== CALIBRATION: \(frontierManager.totalViewpoints) viewpoints, " +
                "\(allElements.count) total elements ===")
            DebugLog.log("bfs", "plan will be built per viewport (no full-page plan)")
            storeSummary(scrollData: scrollData, fingerprint: fingerprint)
            return .ok(viewportMayHaveShifted: scrolledWithNovelContent)
        }

        // Full path: component detection → validation → component plan.
        return calibrateWithComponents(
            fingerprint: fingerprint, scrollData: scrollData,
            classified: classified, scrolledWithNovelContent: scrolledWithNovelContent
        )
    }

    /// Full component detection pipeline: match elements to component definitions,
    /// validate classification quality, and build a component-based exploration plan.
    private func calibrateWithComponents(
        fingerprint: String, scrollData: ScrollCollectionData,
        classified: [ClassifiedElement], scrolledWithNovelContent: Bool
    ) -> CalibrationResult {
        let rawComponents = classifier?.classify(
            classified: classified, definitions: componentDefinitions,
            screenHeight: windowSize.height
        ) ?? ComponentDetector.detect(
            classified: classified, definitions: componentDefinitions,
            screenHeight: windowSize.height)
        let components = ComponentDetector.applyAbsorption(rawComponents)

        DebugLog.log("bfs", "calibration detect: \(componentDefinitions.count) defs, " +
            "\(rawComponents.count) raw → \(components.count) absorbed")

        // Register breadth_navigation labels (e.g. tab bar items) for global tracking.
        // These will be explored once and skipped on every subsequent screen.
        let breadthLabels = Set(
            components
                .filter { $0.definition.exploration.role == .breadthNavigation }
                .map { $0.displayLabel }
        )
        if !breadthLabels.isEmpty {
            graph.registerBreadthLabels(breadthLabels)
            DebugLog.log("bfs", "registered \(breadthLabels.count) breadth labels: \(breadthLabels.sorted())")
        }

        // Recipe matching: identify app archetype from detected component composition.
        // Runs on the first calibrated screen only (recipe is stored in the session).
        if session.currentRecipeMatch == nil && !recipes.isEmpty {
            let detectedKinds = Set(components.map { $0.kind })
            if let match = RecipeMatcher.bestMatch(
                detectedComponents: detectedKinds, recipes: recipes
            ) {
                session.setRecipeMatch(match)
                let (refined, _) = StrategyDetector.refineWithRecipe(
                    current: StrategyChoice(rawValue: session.currentStrategy) ?? .mobile,
                    detectedComponents: detectedKinds,
                    recipes: recipes
                )
                session.setStrategy(refined.rawValue)
                DebugLog.log("bfs", "recipe matched: '\(match.recipe.name)' " +
                    "(score=\(String(format: "%.1f", match.score)), " +
                    "nav=\(match.recipe.navigationModel.type), " +
                    "strategy=\(refined.rawValue))")
            }
        }

        // Validate: check unclassified ratio in content zone.
        let validation = CalibrationValidator.validate(
            components: components, screenHeight: windowSize.height
        )
        DebugLog.log("bfs", "calibration validation: \(validation.report)")

        if !validation.passed {
            storeSummary(
                scrollData: scrollData, fingerprint: fingerprint,
                componentCount: components.count,
                unclassifiedCount: validation.unclassifiedCount,
                validationPassed: false, validationReport: validation.report)
            return .failed(
                "Calibration failed: \(validation.unclassifiedCount)/\(validation.totalContentElements) " +
                "content elements unclassified (\(String(format: "%.0f", validation.unclassifiedRatio * 100))%). " +
                "New component definitions may be needed.\n\n\(validation.report)")
        }

        // Build exploration plan from matched components.
        let plan = ScreenPlanner.buildComponentPlan(
            components: components, visitedElements: [],
            scoutResults: [:], screenHeight: windowSize.height)
        graph.setScreenPlan(for: fingerprint, plan: plan)

        frontierManager.resetViewports(total: scrollData.scrollCount + 1)
        let explorableCount = components.filter { $0.definition.exploration.explorable }.count
        DebugLog.log("bfs", "=== CALIBRATION: \(frontierManager.totalViewpoints) viewpoints ===")
        DebugLog.log("bfs", "calibration plan: \(plan.count) items " +
            "(\(explorableCount) explorable / \(components.count) total components)")

        storeSummary(
            scrollData: scrollData, fingerprint: fingerprint,
            componentCount: components.count,
            unclassifiedCount: validation.unclassifiedCount,
            validationPassed: true, validationReport: validation.report)

        return .ok(viewportMayHaveShifted: scrolledWithNovelContent)
    }

    // MARK: - Plan Coordinate Resolution

    /// Resolve the next unvisited plan item against the current viewport's OCR elements.
    /// Two-pass approach: first try all items against the viewport (no scrolling),
    /// then try scrolling to find unresolved items. This prevents high-score below-fold
    /// items from blocking lower-score viewport-visible items.
    func resolveNextPlanItem<S: ExplorationStrategy>(
        currentFP: String,
        viewportElements: [TapPoint],
        describer: ScreenDescribing,
        input: InputProviding,
        strategy: S.Type
    ) -> RankedElement? {
        guard let plan = graph.screenPlan(for: currentFP) else { return nil }
        let visited = graph.node(for: currentFP)?.visitedElements ?? []
        let globalVisited = graph.globalVisitedLabels

        // Collect unvisited, non-skipped plan items
        var candidates: [RankedElement] = []
        for item in plan {
            if visited.contains(item.displayLabel) || globalVisited.contains(item.displayLabel) {
                continue
            }
            if strategy.shouldSkip(elementText: item.point.text, budget: budget) {
                graph.markElementVisited(fingerprint: currentFP, elementText: item.displayLabel)
                continue
            }
            candidates.append(item)
        }

        // Pass 1: Try each candidate against the current viewport (no scrolling).
        for candidate in candidates {
            let resolution = PlanCoordinateResolver.resolve(
                planItem: candidate, viewportElements: viewportElements
            )
            if case .found(let freshPoint) = resolution {
                return PlanCoordinateResolver.withFreshCoordinates(
                    planItem: candidate, freshPoint: freshPoint
                )
            }
        }

        // Pass 2: Scroll through viewpoints in calibration order to find candidates.
        // Each scroll advances one viewport. After scrolling, check ALL remaining
        // candidates against the fresh viewport — tap the first match found.
        guard !candidates.isEmpty else { return nil }
        let maxScrollAttempts = budget.scrollLimit
        for scrollIdx in 0..<maxScrollAttempts {
            guard let freshElements = scrollToReveal(input: input, describer: describer) else {
                break
            }
            DebugLog.log("bfs", "resolve: scroll \(scrollIdx + 1)/\(maxScrollAttempts), " +
                "\(freshElements.count) elements in viewport")
            for candidate in candidates {
                let resolution = PlanCoordinateResolver.resolve(
                    planItem: candidate, viewportElements: freshElements
                )
                if case .found(let freshPoint) = resolution {
                    return PlanCoordinateResolver.withFreshCoordinates(
                        planItem: candidate, freshPoint: freshPoint
                    )
                }
            }
        }

        DebugLog.log("bfs", "resolve: no candidates found after \(maxScrollAttempts) scrolls")
        return nil
    }

    // MARK: - Report Generation

    /// Generate a structured exploration report covering calibration, per-screen actions,
    /// and tap cache statistics.
    func generateReport() -> String {
        let report = reporter.snapshot()
        let currentStats = stats
        var screenSummaries: [ExplorationReportFormatter.ScreenSummary] = []

        let snapshot = graph.finalize()
        for (fp, node) in snapshot.nodes.sorted(by: { $0.value.depth < $1.value.depth }) {
            let actions = report.screenActions[fp] ?? []
            let cacheHits = report.cacheHitsPerScreen[fp] ?? 0
            let plan = graph.screenPlan(for: fp)
            screenSummaries.append(ExplorationReportFormatter.ScreenSummary(
                depth: node.depth,
                fingerprint: String(fp.prefix(8)),
                componentCount: plan?.count ?? 0,
                actionCount: actions.count,
                cacheHits: cacheHits,
                actions: actions
            ))
        }

        return ExplorationReportFormatter.formatExplorationReport(
            appName: appName,
            calibration: report.calibrationSummary,
            screens: screenSummaries,
            stats: currentStats,
            tapCacheTotal: report.totalCacheHits
        )
    }
}
