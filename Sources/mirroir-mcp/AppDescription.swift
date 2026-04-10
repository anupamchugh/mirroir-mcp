// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Data model and parser for APP.md files — human-authored app structure descriptions.
// ABOUTME: Provides hints, skip lists, and obstacle rules to guide exploration sessions.

import Foundation
import HelperLib

/// A human-authored description of an iOS app's structure, navigation patterns,
/// and known obstacles. Loaded from APP.md files and injected into exploration
/// sessions to guide the AI with domain knowledge the developer already has.
struct AppDescription: Sendable {
    /// App name for matching (case-insensitive). From front matter `app:` field.
    let appName: String
    /// Optional locale (e.g. "fr_CA", "en_US"). When set, only matches that locale.
    let locale: String?
    /// Optional archetype name referencing a screen recipe (e.g. "dashboard", "social-feed").
    /// When set, bypasses recipe auto-detection and uses this recipe directly.
    let archetype: String?
    /// Obstacle handling mode: "auto", "hint", or "off".
    let obstacleMode: ObstacleMode
    /// Free-form sections concatenated as AI context (Structure, tab descriptions, Tips).
    let context: String
    /// Parsed obstacle rules: when trigger text appears, tap the action text.
    let obstacles: [ObstacleRule]
    /// Element text patterns that the explorer must never tap.
    let skipElements: [String]
    /// Credential key-value pairs (supports ${VAR} substitution).
    let credentials: [String: String]
    /// Raw exploration hints for inclusion in generated skills.
    let hints: [String]
}

/// How obstacle rules from APP.md are applied during exploration.
enum ObstacleMode: String, Sendable {
    /// Automatically dismiss matching obstacles (default).
    case auto
    /// Surface obstacle info to the AI as hints but don't auto-act.
    case hint
    /// Ignore obstacle rules entirely.
    case off
}

/// A rule for dismissing an app-specific obstacle (paywall, permission dialog, onboarding).
struct ObstacleRule: Sendable {
    /// Text that triggers this rule (substring match against screen elements).
    let trigger: String
    /// Text of the element to tap to dismiss (e.g. "Not Now", "Don't Allow", "×").
    let action: String
}

/// Parses APP.md files into AppDescription instances.
/// Pure transformation: all static methods, no stored state.
enum AppDescriptionParser {

    /// Parse an APP.md file's content into an AppDescription.
    /// Returns nil if required fields (app name, at least one section) are missing.
    static func parse(content: String) -> AppDescription? {
        let (frontMatter, sections) = extractFrontMatterAndSections(content: content)

        guard let appName = frontMatter["app"] else { return nil }
        let locale = frontMatter["locale"]
        let archetype = frontMatter["archetype"]
        let obstacleMode = ObstacleMode(rawValue: frontMatter["obstacle_mode"] ?? "auto") ?? .auto

        // Collect free-form context from Structure, tab descriptions, and Tips
        var contextParts: [String] = []
        if let structure = sections["Structure"] {
            contextParts.append("## Structure\n\(structure.trimmingCharacters(in: .whitespacesAndNewlines))")
        }
        // Collect tab description sections (any heading ending with "Tab")
        for (heading, body) in sections.sorted(by: { $0.key < $1.key }) {
            if heading.hasSuffix("Tab") || heading.hasSuffix("tab") {
                contextParts.append("## \(heading)\n\(body.trimmingCharacters(in: .whitespacesAndNewlines))")
            }
        }
        if let tips = sections["Tips"] {
            contextParts.append("## Tips\n\(tips.trimmingCharacters(in: .whitespacesAndNewlines))")
        }

        let context = contextParts.joined(separator: "\n\n")
        guard !context.isEmpty || sections["Obstacles"] != nil || sections["Skip"] != nil else {
            return nil
        }

        let obstacles = parseObstacles(sections["Obstacles"] ?? "")
        let skipElements = parseList(sections["Skip"] ?? "")
        let credentials = parseCredentials(sections["Credentials"] ?? "")
        let hints = parseList(sections["Tips"] ?? "")

        return AppDescription(
            appName: appName,
            locale: locale,
            archetype: archetype,
            obstacleMode: obstacleMode,
            context: context,
            obstacles: obstacles,
            skipElements: skipElements,
            credentials: credentials,
            hints: hints
        )
    }

    // MARK: - Section Parsers

    /// Parse obstacle rules: lines like "- Health Access permission dialog → tap 'Allow'"
    /// or "- paywall → dismiss via 'Not Now'"
    private static func parseObstacles(_ text: String) -> [ObstacleRule] {
        var rules: [ObstacleRule] = []
        for line in text.components(separatedBy: .newlines) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("- ") else { continue }
            let body = String(trimmed.dropFirst(2))

            // Parse "trigger → action" format
            // Support both → and -> as arrow separators
            let separator: String
            if body.contains("→") {
                separator = "→"
            } else if body.contains("->") {
                separator = "->"
            } else {
                continue
            }

            let splitParts = body.components(separatedBy: separator)
            guard splitParts.count >= 2 else { continue }

            let trigger = splitParts[0].trimmingCharacters(in: .whitespaces)
            let action = splitParts.dropFirst().joined(separator: separator)
                .trimmingCharacters(in: .whitespaces)

            // Strip common prefixes: "tap ", "dismiss via ", etc.
            let cleaned = action
                .replacingOccurrences(of: "tap ", with: "", options: [.caseInsensitive, .anchored])
                .replacingOccurrences(of: "dismiss via ", with: "", options: [.caseInsensitive, .anchored])
                .replacingOccurrences(of: "dismiss ", with: "", options: [.caseInsensitive, .anchored])
                .trimmingCharacters(in: .whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'\u{201C}\u{201D}"))

            guard !trigger.isEmpty && !cleaned.isEmpty else { continue }
            rules.append(ObstacleRule(trigger: trigger, action: cleaned))
        }
        return rules
    }

    /// Parse a simple bulleted list into strings.
    private static func parseList(_ text: String) -> [String] {
        text.components(separatedBy: .newlines)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { $0.hasPrefix("- ") }
            .map { line in
                String(line.dropFirst(2))
                    .trimmingCharacters(in: .whitespaces)
                    .trimmingCharacters(in: CharacterSet(charactersIn: "\"'\u{201C}\u{201D}"))
            }
            .filter { !$0.isEmpty }
    }

    /// Parse credentials as key: value pairs.
    private static func parseCredentials(_ text: String) -> [String: String] {
        var creds: [String: String] = [:]
        for line in text.components(separatedBy: .newlines) {
            var stripped = line.trimmingCharacters(in: .whitespaces)
            if stripped.hasPrefix("- ") { stripped = String(stripped.dropFirst(2)) }
            guard let colonIdx = stripped.firstIndex(of: ":") else { continue }
            let key = String(stripped[..<colonIdx]).trimmingCharacters(in: .whitespaces)
            let value = String(stripped[stripped.index(after: colonIdx)...])
                .trimmingCharacters(in: .whitespaces)
            guard !key.isEmpty else { continue }
            creds[key] = value
        }
        return creds
    }

    // MARK: - Front Matter & Section Extraction

    /// Extract YAML front matter and markdown sections from APP.md content.
    private static func extractFrontMatterAndSections(
        content: String
    ) -> ([String: String], [String: String]) {
        var frontMatter: [String: String] = [:]
        var sections: [String: String] = [:]
        var currentSection: String?
        var currentBody: [String] = []
        var inFrontMatter = false
        var frontMatterDone = false

        for line in content.components(separatedBy: .newlines) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if trimmed == "---" {
                if !frontMatterDone && !inFrontMatter {
                    inFrontMatter = true
                    continue
                } else if inFrontMatter {
                    inFrontMatter = false
                    frontMatterDone = true
                    continue
                }
            }

            if inFrontMatter {
                let parts = trimmed.split(separator: ":", maxSplits: 1)
                if parts.count == 2 {
                    let key = parts[0].trimmingCharacters(in: .whitespaces)
                    let value = parts[1].trimmingCharacters(in: .whitespaces)
                    frontMatter[key] = value
                }
                continue
            }

            if trimmed.hasPrefix("## ") {
                if let section = currentSection {
                    sections[section] = currentBody.joined(separator: "\n")
                }
                currentSection = String(trimmed.dropFirst(3))
                currentBody = []
            } else if trimmed.hasPrefix("# ") {
                // H1 title — skip
            } else {
                currentBody.append(line)
            }
        }

        if let section = currentSection {
            sections[section] = currentBody.joined(separator: "\n")
        }

        return (frontMatter, sections)
    }
}
