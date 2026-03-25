// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Health-related scenario definitions for FakeMirroring.
// ABOUTME: Extracted from Scenarios.swift to stay under the 500-line limit.

import AppKit

extension ScenarioContent {

    /// Health dashboard with 6 summary cards (Steps, Heart Rate, Sleep, etc.)
    static func healthScenario() -> ScenarioData {
        let cardW: CGFloat = 378, cardH: CGFloat = 110
        let cardX: CGFloat = 16, gap: CGFloat = 12, startY: CGFloat = 180
        func cardRect(_ i: Int) -> CGRect {
            CGRect(x: cardX, y: startY + CGFloat(i) * (cardH + gap), width: cardW, height: cardH)
        }
        return ScenarioData(
            header: "Summary",
            rows: [],
            hasTabBar: true,
            cards: [
                CardData(title: "Steps", value: "8,432", subtitle: "Daily Average",
                         color: .systemRed, rect: cardRect(0)),
                CardData(title: "Heart Rate", value: "72 BPM", subtitle: "Resting",
                         color: .systemPink, rect: cardRect(1)),
                CardData(title: "Sleep", value: "7h 23min", subtitle: "Time in Bed",
                         color: .systemIndigo, rect: cardRect(2)),
                CardData(title: "Exercise Minutes", value: "32 min", subtitle: "Move Goal",
                         color: .systemGreen, rect: cardRect(3)),
                CardData(title: "Respiratory Rate", value: "15 brpm", subtitle: "Average",
                         color: .systemTeal, rect: cardRect(4)),
                CardData(title: "Mindful Minutes", value: "12 min", subtitle: "Weekly Total",
                         color: .systemOrange, rect: cardRect(5)),
            ]
        )
    }

    /// Health Summary with 3 viewports of scrollable content mimicking the Santé app.
    /// Viewport 1: summary cards (Activity, Workouts, Distance, Steps, Heart Rate)
    /// Viewport 2: health setup items (Medical Records, Get Started, Add Medication)
    /// Viewport 3: health articles (Hearing Loss, Hypertension, Sleep Tips)
    static func healthSummaryScenario() -> ScenarioData {
        let cardW: CGFloat = 378, cardH: CGFloat = 110
        let cardX: CGFloat = 16, gap: CGFloat = 12

        // Viewport 1: Summary cards (y: 130–770)
        let vp1StartY: CGFloat = 130
        func vp1Rect(_ i: Int) -> CGRect {
            CGRect(x: cardX, y: vp1StartY + CGFloat(i) * (cardH + gap),
                   width: cardW, height: cardH)
        }
        let cards: [CardData] = [
            CardData(title: "Activity", value: "1 cal", subtitle: "Move",
                     color: .systemRed, rect: vp1Rect(0)),
            CardData(title: "Workouts", value: "1h 44min", subtitle: "Yesterday",
                     color: .systemGreen, rect: vp1Rect(1)),
            CardData(title: "Walking Distance", value: "0.08 km", subtitle: "Today",
                     color: .systemOrange, rect: vp1Rect(2)),
            CardData(title: "Steps", value: "129", subtitle: "Today",
                     color: .systemOrange, rect: vp1Rect(3)),
            CardData(title: "Heart Rate", value: "72 BPM", subtitle: "Resting",
                     color: .systemPink, rect: vp1Rect(4)),
        ]

        // Viewport 2: Setup items as rows with chevrons (y: 810–1130)
        let vp2StartY: CGFloat = 810
        let rowH: CGFloat = 50
        let vp2Rows: [(String, CGPoint)] = [
            ("Medical Records", CGPoint(x: 200, y: vp2StartY)),
            ("Get Started", CGPoint(x: 200, y: vp2StartY + rowH + gap)),
            ("Add Medication", CGPoint(x: 200, y: vp2StartY + 2 * (rowH + gap))),
            ("Health Features", CGPoint(x: 200, y: vp2StartY + 3 * (rowH + gap))),
            ("Mental Wellbeing", CGPoint(x: 200, y: vp2StartY + 4 * (rowH + gap))),
        ]

        // Viewport 3: Articles as plain text (y: 1200–1600)
        let vp3StartY: CGFloat = 1200
        let articleH: CGFloat = 120
        let vp3Rows: [(String, CGPoint)] = [
            ("Hearing Loss", CGPoint(x: 200, y: vp3StartY)),
            ("Hypertension", CGPoint(x: 200, y: vp3StartY + articleH + gap)),
            ("Sleep Hygiene", CGPoint(x: 200, y: vp3StartY + 2 * (articleH + gap))),
            ("Nutrition", CGPoint(x: 200, y: vp3StartY + 3 * (articleH + gap))),
        ]

        return ScenarioData(
            header: "Summary",
            rows: vp2Rows + vp3Rows,
            hasTabBar: true,
            cards: cards
        )
    }
}
