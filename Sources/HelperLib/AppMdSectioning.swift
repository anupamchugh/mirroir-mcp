// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Splits APP.md content into YAML frontmatter and H2 markdown sections.
// ABOUTME: Shared by mirroir-mcp's AppDescriptionParser and FakeMirroring's AppPackLoader.

import Foundation

/// Frontmatter + H2 section extraction for APP.md files.
/// Pure transformation, no I/O.
public enum AppMdSectioning {

    public struct Result: Sendable {
        public let frontMatter: [String: String]
        public let sections: [String: String]
    }

    /// Walk `content` line-by-line, populating frontmatter (between `---` delimiters)
    /// and a dictionary of H2 section bodies keyed by their heading text.
    public static func extract(content: String) -> Result {
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
        return Result(frontMatter: frontMatter, sections: sections)
    }
}
