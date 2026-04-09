// Copyright 2026 jfarcand@apache.org
// Licensed under the Apache License, Version 2.0
//
// ABOUTME: Tests for AppDescriptionParser and AppDescription integration.
// ABOUTME: Validates APP.md parsing, obstacle rules, skip lists, and credential handling.

import XCTest
@testable import mirroir_mcp

final class AppDescriptionTests: XCTestCase {

    // MARK: - Parser Tests

    func testParseValidAppDescription() {
        let content = """
        ---
        version: 1
        app: MyApp
        locale: en_US
        obstacle_mode: auto
        ---

        # MyApp

        ## Structure

        A simple app with two tabs.

        ## Home Tab

        Feed of posts.

        ## Obstacles

        - Permission dialog → tap "Don't Allow"
        - Paywall → dismiss via "Not Now"

        ## Skip

        - Delete Account
        - Sign Out

        ## Credentials

        - email: ${TEST_EMAIL:-test@example.com}
        - password: secret123

        ## Tips

        - The feed scrolls infinitely
        - Profile tab has settings
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertNotNil(desc)
        XCTAssertEqual(desc?.appName, "MyApp")
        XCTAssertEqual(desc?.locale, "en_US")
        XCTAssertEqual(desc?.obstacleMode, .auto)
    }

    func testParseObstacleRules() {
        let content = """
        ---
        app: TestApp
        ---

        # Test

        ## Description

        Test app.

        ## Structure

        Simple.

        ## Obstacles

        - Health Access permission → tap "Allow"
        - Paywall modal -> dismiss "Not Now"
        - Onboarding screen → tap 'Continue'
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertNotNil(desc)
        XCTAssertEqual(desc?.obstacles.count, 3)

        XCTAssertEqual(desc?.obstacles[0].trigger, "Health Access permission")
        XCTAssertEqual(desc?.obstacles[0].action, "Allow")

        XCTAssertEqual(desc?.obstacles[1].trigger, "Paywall modal")
        XCTAssertEqual(desc?.obstacles[1].action, "Not Now")

        XCTAssertEqual(desc?.obstacles[2].trigger, "Onboarding screen")
        XCTAssertEqual(desc?.obstacles[2].action, "Continue")
    }

    func testParseSkipElements() {
        let content = """
        ---
        app: TestApp
        ---

        # Test

        ## Structure

        Simple.

        ## Skip

        - Delete Account
        - "Sign Out"
        - Reset All Settings
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertNotNil(desc)
        XCTAssertEqual(desc?.skipElements.count, 3)
        XCTAssertTrue(desc?.skipElements.contains("Delete Account") ?? false)
        XCTAssertTrue(desc?.skipElements.contains("Sign Out") ?? false)
        XCTAssertTrue(desc?.skipElements.contains("Reset All Settings") ?? false)
    }

    func testParseCredentials() {
        let content = """
        ---
        app: TestApp
        ---

        # Test

        ## Structure

        Simple.

        ## Credentials

        - email: test@example.com
        - password: ${TEST_PASSWORD:-secret}
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertNotNil(desc)
        XCTAssertEqual(desc?.credentials["email"], "test@example.com")
        XCTAssertEqual(desc?.credentials["password"], "${TEST_PASSWORD:-secret}")
    }

    func testParseContextIncludesStructureAndTabs() {
        let content = """
        ---
        app: TestApp
        ---

        # Test

        ## Structure

        Dashboard with 3 tabs.

        ## Home Tab

        Summary cards.

        ## Settings Tab

        Drill-down rows.

        ## Tips

        - Tip one
        - Tip two
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertNotNil(desc)
        XCTAssertTrue(desc?.context.contains("Dashboard with 3 tabs") ?? false)
        XCTAssertTrue(desc?.context.contains("Home Tab") ?? false)
        XCTAssertTrue(desc?.context.contains("Summary cards") ?? false)
        XCTAssertTrue(desc?.context.contains("Settings Tab") ?? false)
        XCTAssertTrue(desc?.context.contains("Tip one") ?? false)
    }

    func testParseReturnsNilWithoutAppName() {
        let content = """
        ---
        version: 1
        ---

        # No App Name

        ## Structure

        Missing app field.
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertNil(desc, "Should return nil when app name is missing")
    }

    func testParseReturnsNilWithEmptyContent() {
        let content = """
        ---
        app: EmptyApp
        ---

        # Empty
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertNil(desc, "Should return nil when no sections have content")
    }

    func testDefaultObstacleModeIsAuto() {
        let content = """
        ---
        app: TestApp
        ---

        # Test

        ## Structure

        Simple app.
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertEqual(desc?.obstacleMode, .auto)
    }

    func testObstacleModeHint() {
        let content = """
        ---
        app: TestApp
        obstacle_mode: hint
        ---

        # Test

        ## Structure

        Simple app.
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertEqual(desc?.obstacleMode, .hint)
    }

    func testObstacleModeOff() {
        let content = """
        ---
        app: TestApp
        obstacle_mode: off
        ---

        # Test

        ## Structure

        Simple app.
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertEqual(desc?.obstacleMode, .off)
    }

    func testObstacleArrowSeparators() {
        let content = """
        ---
        app: TestApp
        ---

        # Test

        ## Structure

        Simple.

        ## Obstacles

        - Unicode arrow → tap "OK"
        - ASCII arrow -> tap "Cancel"
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertEqual(desc?.obstacles.count, 2)
        XCTAssertEqual(desc?.obstacles[0].action, "OK")
        XCTAssertEqual(desc?.obstacles[1].action, "Cancel")
    }

    func testHintsAreParsed() {
        let content = """
        ---
        app: TestApp
        ---

        # Test

        ## Structure

        Simple.

        ## Tips

        - First hint
        - Second hint
        - Third hint
        """

        let desc = AppDescriptionParser.parse(content: content)
        XCTAssertEqual(desc?.hints.count, 3)
        XCTAssertEqual(desc?.hints[0], "First hint")
    }

    // MARK: - Loader Integration Tests

    func testLoaderFindsSanteAppMd() {
        // This test requires the sibling mirroir-skills repo to be present
        let desc = AppDescriptionLoader.load(appName: "Sante")
        if desc == nil {
            // Log search paths for debugging
            let paths = AppDescriptionLoader.searchPaths()
            for path in paths {
                print("Search path: \(path)")
                let fm = FileManager.default
                if let enumerator = fm.enumerator(atPath: path) {
                    while let entry = enumerator.nextObject() as? String {
                        let filename = (entry as NSString).lastPathComponent.lowercased()
                        if filename == "app.md" {
                            let full = path + "/" + entry
                            print("  Found: \(full)")
                            if let content = try? String(contentsOfFile: full, encoding: .utf8) {
                                print("  Content length: \(content.count)")
                                if let parsed = AppDescriptionParser.parse(content: content) {
                                    print("  Parsed: app=\(parsed.appName)")
                                    let folded = parsed.appName.folding(
                                        options: [.diacriticInsensitive, .caseInsensitive],
                                        locale: .current)
                                    print("  Folded: \(folded)")
                                } else {
                                    print("  Parse FAILED")
                                }
                            }
                        }
                    }
                }
            }
        }
        XCTAssertNotNil(desc, "Should find Santé APP.md via diacritics-insensitive match")
        XCTAssertEqual(desc?.skipElements.isEmpty, false, "Should have skip elements")
        XCTAssertEqual(desc?.obstacles.isEmpty, false, "Should have obstacles")
    }
}
