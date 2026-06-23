// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Resolves the consumer-repo .mirroir/ root where the iOS oracle leg is emitted.
// ABOUTME: Walks up from cwd for an existing .mirroir/, else falls back to <cwd>/.mirroir.

import Foundation

/// Locates the consumer repository's `.mirroir/` dotfile directory — the runner's
/// consumer root, distinct from the Swift MCP's `~/.mirroir-mcp/` home. Mirrors the
/// runner's own walk-up discovery (`runner/src/mirroir/discover.rs`).
enum MirroirRootLocator {

    /// Directory name of the consumer-side mirroir tree.
    static let dirName = ".mirroir"

    /// Resolve the `.mirroir/` directory to emit into. Resolution order:
    /// 1. `explicitDir` (a `generate_skill output_dir` arg), if given → `<explicitDir>/.mirroir`.
    /// 2. The nearest existing `.mirroir/` walking up from `start` (the process cwd).
    /// 3. `<start>/.mirroir`.
    ///
    /// Returns `nil` when the resolved target would be `~/.mirroir` — that is the
    /// runner's pack home (`~/.mirroir/skills/`), not a consumer dotfile, so the
    /// caller must pass an `output_dir` or run from a consumer repo. Does not create
    /// the directory.
    static func resolve(explicitDir: String? = nil, start: URL? = nil) -> URL? {
        let target: URL
        if let explicitDir, !explicitDir.isEmpty {
            target = URL(fileURLWithPath: explicitDir, isDirectory: true)
                .appendingPathComponent(dirName, isDirectory: true)
        } else {
            let base = start ?? URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            target = walkUp(from: base) ?? base.appendingPathComponent(dirName, isDirectory: true)
        }
        let homeMirroir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(dirName, isDirectory: true).standardizedFileURL
        return target.standardizedFileURL.path == homeMirroir.path ? nil : target
    }

    /// Walk up from `base` returning the nearest existing `.mirroir/`, or nil.
    private static func walkUp(from base: URL) -> URL? {
        var current = base
        while true {
            let candidate = current.appendingPathComponent(dirName, isDirectory: true)
            var isDir: ObjCBool = false
            if FileManager.default.fileExists(atPath: candidate.path, isDirectory: &isDir), isDir.boolValue {
                return candidate
            }
            let parent = current.deletingLastPathComponent()
            if parent.path == current.path { return nil } // reached the filesystem root
            current = parent
        }
    }
}
