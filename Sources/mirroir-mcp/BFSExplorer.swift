// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: BFS (breadth-first) explorer that traverses app screens layer by layer.
// ABOUTME: Each step() call performs one action: navigate, tap an element, or return to root.

import Foundation
import HelperLib

/// BFS explorer: traverses app screens layer by layer. Each `step()` call
/// performs one action (navigate, tap, or return to root).
/// Session Accumulator pattern with NSLock protection.
final class BFSExplorer: @unchecked Sendable {

    let graph: any NavigationGraphing
    let session: ExplorationSession
    let budget: ExplorationBudget
    let windowSize: CGSize
    let appName: String
    let componentDefinitions: [ComponentDefinition]
    let classifier: (any ComponentClassifying)?
    let bridge: (any WindowBridging)?

    var frontier: [FrontierScreen] = []
    var frontierIndex: Int = 0
    var phase: BFSPhase = .atRoot
    var actionsOnCurrentScreen: Int = 0
    var startTime: Date = Date()
    /// Fingerprints of screens that have been calibrated (full-page scroll + component detection).
    var calibratedScreens: Set<String> = []
    /// Report data: calibration summary, collected during calibration phase.
    var calibrationSummary: ExplorationReportFormatter.CalibrationSummary?
    /// Report data: per-screen action entries, keyed by fingerprint.
    var screenActions: [String: [ExplorationReportFormatter.ActionEntry]] = [:]
    /// Report data: tap cache hit count per screen.
    var cacheHitsPerScreen: [String: Int] = [:]
    var actionCount: Int = 0
    var isFinished: Bool = false
    let coverageMonitor: CoverageMonitor
    /// AI advisor for plateau-phase guidance. Suggests which elements to tap
    /// when systematic exploration stalls.
    let advisor: (any ExplorationAdvising)?
    /// Seeded PRNG for deterministic exploration ordering. nil = system random.
    let rng: ExplorationRNG
    /// Skip component detection during calibration (scroll still runs).
    let skipCalibration: Bool
    /// Loaded screen recipes for archetype detection.
    let recipes: [ScreenRecipe]
    /// Total viewpoints discovered during calibration scroll.
    var totalViewpoints: Int = 0
    /// Current viewport index being processed (0-based, increments on scroll-down).
    var currentViewportIndex: Int = 0
    let lock = NSLock()

    init(
        session: ExplorationSession,
        budget: ExplorationBudget,
        windowSize: CGSize = CGSize(width: 410, height: 890),
        componentDefinitions: [ComponentDefinition] = [],
        classifier: (any ComponentClassifying)? = nil,
        bridge: (any WindowBridging)? = nil,
        seed: UInt64? = nil,
        skipCalibration: Bool = false,
        advisor: (any ExplorationAdvising)? = nil,
        coverageMonitor: CoverageMonitor = CoverageMonitor(),
        recipes: [ScreenRecipe] = []
    ) {
        self.session = session
        self.graph = session.currentGraph
        self.budget = budget
        self.windowSize = windowSize
        self.appName = session.currentAppName
        self.componentDefinitions = componentDefinitions
        self.classifier = classifier
        self.bridge = bridge
        self.rng = seed.map { ExplorationRNG(seed: $0) } ?? ExplorationRNG()
        self.skipCalibration = skipCalibration
        self.advisor = advisor
        self.coverageMonitor = coverageMonitor
        self.recipes = recipes
    }

    /// Record start time and seed frontier with the root screen. Call once after initial capture.
    func markStarted() {
        lock.lock()
        defer { lock.unlock() }
        startTime = Date()
        coverageMonitor.start()
        if graph.started {
            let rootFP = graph.rootFingerprint
            frontier = [FrontierScreen(fingerprint: rootFP, pathFromRoot: [], depth: 0)]
            frontierIndex = 0
        }
    }
    /// Perform one BFS exploration step. Dispatches to the current phase handler.
    func step<S: ExplorationStrategy>(
        describer: ScreenDescribing,
        input: InputProviding,
        strategy: S.Type
    ) -> ExploreStepResult {
        lock.lock()
        let finished = isFinished
        let elapsed = Int(Date().timeIntervalSince(startTime))
        let screenCount = graph.nodeCount
        lock.unlock()

        guard !finished else {
            return .finished(bundle: generateBundle())
        }

        // Budget check — depth is enforced at frontier insertion, check time/screens here
        if budget.isExhausted(depth: 0, screenCount: screenCount, elapsedSeconds: elapsed) {
            lock.lock()
            isFinished = true
            lock.unlock()
            return .finished(bundle: generateBundle())
        }

        // Coverage exhaustion — stop if no new screens for extended period
        if coverageMonitor.currentPhase == .exhaustion {
            lock.lock(); isFinished = true; lock.unlock()
            return .finished(bundle: generateBundle())
        }

        DebugLog.log("bfs", "step: phase=\(phase) screens=\(screenCount) elapsed=\(elapsed)s")
        switch phase {
        case .atRoot:
            return stepAtRoot(describer: describer, input: input, strategy: strategy)
        case .navigating(let target, let pathIndex):
            return stepNavigating(
                target: target, pathIndex: pathIndex,
                describer: describer, input: input, strategy: strategy
            )
        case .exploring(let screen):
            return stepExploring(
                screen: screen, describer: describer, input: input, strategy: strategy
            )
        case .returning(let depthRemaining):
            return stepReturning(
                depthRemaining: depthRemaining, describer: describer, input: input
            )
        }
    }

    // MARK: - Phase: At Root

    /// Dequeue the next frontier screen and begin navigation or exploration.
    private func stepAtRoot<S: ExplorationStrategy>(
        describer: ScreenDescribing,
        input: InputProviding,
        strategy: S.Type
    ) -> ExploreStepResult {
        lock.lock()
        guard frontierIndex < frontier.count else {
            isFinished = true
            lock.unlock()
            return .finished(bundle: generateBundle())
        }
        let target = frontier[frontierIndex]
        frontierIndex += 1
        actionsOnCurrentScreen = 0
        lock.unlock()

        let pathDesc = target.pathFromRoot.map { $0.elementText }.joined(separator: " → ")
        DebugLog.log("bfs", "dequeue frontier[\(frontierIndex - 1)/\(frontier.count)] " +
            "depth=\(target.depth) path=[\(pathDesc)] fp=\(target.fingerprint.prefix(8))")

        if target.pathFromRoot.isEmpty {
            // Root screen (depth 0) — calibrate by scrolling through the full page
            // to discover all elements, run component detection, build plan, then
            // scroll back to top before exploring.
            graph.setCurrentFingerprint(target.fingerprint)
            phase = .exploring(screen: target)
            return stepExploring(
                screen: target, describer: describer, input: input, strategy: strategy
            )
        }

        // Navigate from root to the target screen
        phase = .navigating(target: target, pathIndex: 0)
        return stepNavigating(
            target: target, pathIndex: 0,
            describer: describer, input: input, strategy: strategy
        )
    }

    // MARK: - Phase: Navigating

    /// Tap one path segment to navigate toward the target frontier screen.
    private func stepNavigating<S: ExplorationStrategy>(
        target: FrontierScreen,
        pathIndex: Int,
        describer: ScreenDescribing,
        input: InputProviding,
        strategy: S.Type
    ) -> ExploreStepResult {
        let segment = target.pathFromRoot[pathIndex]

        // Tap the element at this path segment
        _ = input.tap(x: segment.tapX, y: segment.tapY)
        usleep(EnvConfig.stepSettlingDelayMs * 1000)

        lock.lock()
        actionCount += 1
        lock.unlock()

        // OCR to verify navigation succeeded
        guard ExplorerUtilities.dismissAlertIfPresent(
            describer: describer, input: input
        ) != nil else {
            // OCR failed — skip this frontier screen, return to root
            phase = target.depth > 1
                ? .returning(depthRemaining: pathIndex + 1) : .atRoot
            return .paused(reason: "OCR failed during navigation to depth-\(target.depth) screen")
        }

        let nextIndex = pathIndex + 1
        if nextIndex >= target.pathFromRoot.count {
            // Trust path replay: tapping the same element navigates to the same screen.
            // Structural verification was too strict — clock changes, scroll position
            // differences, and dynamic content cause false negatives on nearly identical screens.
            graph.setCurrentFingerprint(target.fingerprint)
            lock.lock()
            actionsOnCurrentScreen = 0
            lock.unlock()
            phase = .exploring(screen: target)
            return .continue(
                description: "Navigated to depth-\(target.depth) screen via \"\(segment.elementText)\""
            )
        }

        // More path segments to go
        phase = .navigating(target: target, pathIndex: nextIndex)
        return .continue(
            description: "Navigating: tapped \"\(segment.elementText)\" " +
                "(step \(nextIndex)/\(target.pathFromRoot.count))"
        )
    }

}
