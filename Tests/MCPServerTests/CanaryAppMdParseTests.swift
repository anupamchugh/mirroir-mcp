// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Smoke tests that load the canary APP.md files (Settings, Weather) and assert the simulator block parses.
// ABOUTME: Catches drift between the schema and the on-disk files that ship with mirroir-skills.

import XCTest
@testable import mirroir_mcp
@testable import HelperLib

final class CanaryAppMdParseTests: XCTestCase {

    // MARK: - Settings canary

    func testSettingsAppMdParsesWithSimulatorBlock() throws {
        let description = try loadAppMd(app: "settings")

        XCTAssertEqual(description.appName, "Settings")
        XCTAssertNotNil(description.simulator,
            "Settings APP.md must declare a Simulator block now")

        let sim = description.simulator!
        XCTAssertEqual(sim.spotlightName, "Settings")
        XCTAssertEqual(sim.icon, "⚙")
        XCTAssertEqual(sim.rootScreenID, "main")

        // Spot-check key screens exist
        let mainElements = sim.screens["main"]?.elements ?? []
        XCTAssertTrue(mainElements.contains { $0.displayText == "General" },
            "main screen must offer General row")
        XCTAssertNotNil(sim.screens["general"], "general screen must exist")
        XCTAssertNotNil(sim.screens["about"], "about screen must exist")

        // Back navigation chain
        XCTAssertEqual(sim.screens["general"]?.backTo, "main")
        XCTAssertEqual(sim.screens["about"]?.backTo, "general")
        XCTAssertNil(sim.screens["main"]?.backTo, "main is root, no back")

        // Obstacles declared in APP.md must be present
        let obstacleIDs = sim.obstacles.map { $0.id }
        XCTAssertTrue(obstacleIDs.contains("software_update"))
        XCTAssertTrue(obstacleIDs.contains("signin"))
    }

    // MARK: - Weather canary

    func testWeatherAppMdParsesWithSimulatorBlock() throws {
        let description = try loadAppMd(app: "weather")

        XCTAssertEqual(description.appName, "Weather")
        XCTAssertNotNil(description.simulator,
            "Weather APP.md must declare a Simulator block now")

        let sim = description.simulator!
        XCTAssertEqual(sim.icon, "☀")
        XCTAssertEqual(sim.rootScreenID, "main")

        XCTAssertNotNil(sim.screens["main"])
        XCTAssertNotNil(sim.screens["cities"])
        XCTAssertEqual(sim.screens["cities"]?.backTo, "main")

        let obstacleIDs = sim.obstacles.map { $0.id }
        XCTAssertTrue(obstacleIDs.contains("location_permission"))
    }

    // MARK: - Sante / TikTok / Instagram / Claude canaries

    func testSanteAppMdParses() throws {
        let description = try loadAppMd(app: "sante")
        XCTAssertEqual(description.appName, "Santé")
        XCTAssertEqual(description.simulator?.spotlightName, "Sante")
        XCTAssertEqual(description.simulator?.rootScreenID, "resume")
        let screens = description.simulator?.screens ?? [:]
        XCTAssertNotNil(screens["resume"])
        XCTAssertNotNil(screens["parcourir"])
        XCTAssertNotNil(screens["activite"])
        XCTAssertEqual(screens["activite"]?.backTo, "resume")
    }

    func testTikTokAppMdParses() throws {
        let description = try loadAppMd(app: "tiktok")
        XCTAssertEqual(description.appName, "TikTok")
        XCTAssertEqual(description.simulator?.rootScreenID, "pour_toi")
        XCTAssertNotNil(description.simulator?.screens["explorer"])
        // 5 tabs declared as elements on every tab-bar screen
        let pourToiTabs = description.simulator?.screens["pour_toi"]?.elements
            .compactMap { element -> String? in
                if case .tab(let text, _) = element { return text }
                return nil
            } ?? []
        XCTAssertGreaterThanOrEqual(pourToiTabs.count, 4)
    }

    func testInstagramAppMdParses() throws {
        let description = try loadAppMd(app: "instagram")
        XCTAssertEqual(description.appName, "Instagram")
        XCTAssertEqual(description.simulator?.rootScreenID, "accueil")
        XCTAssertNotNil(description.simulator?.screens["recherche"])
        XCTAssertNotNil(description.simulator?.screens["profil_ig"])
    }

    func testClaudeAppMdParses() throws {
        let description = try loadAppMd(app: "claude")
        XCTAssertEqual(description.appName, "Claude")
        XCTAssertEqual(description.simulator?.icon, "✦")
        XCTAssertEqual(description.simulator?.rootScreenID, "home")
        XCTAssertNotNil(description.simulator?.screens["conversations"])
        XCTAssertNotNil(description.simulator?.screens["composer"])
    }

    // MARK: - Drift checker

    /// Walk every APP.md in the skills directory and assert structural integrity:
    ///   1. Every leads_to and back reference points at a screen that exists.
    ///   2. Every obstacle has at least one button.
    ///   3. The root screen ID exists in the screens map.
    func testAllAppMdSimulatorBlocksAreInternallyConsistent() throws {
        let cwd = FileManager.default.currentDirectoryPath
        let appsDir = "\(cwd)/../mirroir-skills/patterns/apps"
        let fm = FileManager.default
        let entries = (try? fm.contentsOfDirectory(atPath: appsDir)) ?? []

        var packsChecked = 0
        for slug in entries.sorted() {
            let path = "\(appsDir)/\(slug)/APP.md"
            guard fm.fileExists(atPath: path),
                  let content = try? String(contentsOfFile: path, encoding: .utf8),
                  let description = AppDescriptionParser.parse(content: content),
                  let sim = description.simulator else { continue }

            let knownIDs = Set(sim.screens.keys)
            XCTAssertTrue(knownIDs.contains(sim.rootScreenID),
                "[\(slug)] root '\(sim.rootScreenID)' must exist in screens")

            for (id, screen) in sim.screens {
                if let backTo = screen.backTo {
                    XCTAssertTrue(knownIDs.contains(backTo),
                        "[\(slug)] screen '\(id)' back='\(backTo)' missing")
                }
                for element in screen.elements {
                    if let leadsTo = element.leadsTo {
                        XCTAssertTrue(knownIDs.contains(leadsTo),
                            "[\(slug)] screen '\(id)' element '\(element.displayText)' " +
                            "leads_to='\(leadsTo)' missing")
                    }
                }
            }
            for obstacle in sim.obstacles {
                XCTAssertFalse(obstacle.buttons.isEmpty,
                    "[\(slug)] obstacle '\(obstacle.id)' must have at least one button")
            }
            packsChecked += 1
        }
        XCTAssertGreaterThanOrEqual(packsChecked, 6,
            "Expected ≥6 packs (settings, weather, sante, tiktok, instagram, claude)")
    }

    // MARK: - Helpers

    private func loadAppMd(app slug: String) throws -> AppDescription {
        // Tests run with the package root as cwd. mirroir-skills is a sibling
        // checkout (per CLAUDE.md "Sibling Repositories"), so go up one level.
        let cwd = FileManager.default.currentDirectoryPath
        let appMdPath = "\(cwd)/../mirroir-skills/patterns/apps/\(slug)/APP.md"

        let content = try String(contentsOfFile: appMdPath, encoding: .utf8)
        guard let description = AppDescriptionParser.parse(content: content) else {
            XCTFail("AppDescriptionParser returned nil for \(slug)")
            throw NSError(domain: "test", code: 1)
        }
        return description
    }
}
