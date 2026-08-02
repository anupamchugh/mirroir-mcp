import XCTest
@testable import mirroir_mcp

final class ScreenCapturePermissionTests: XCTestCase {
    func testDoesNotRequestWhenCaptureIsAlreadyPermitted() {
        var requested = false

        let granted = ScreenCapturePermission.requestIfNeeded(
            preflight: { true },
            request: { requested = true; return true }
        )

        XCTAssertTrue(granted)
        XCTAssertFalse(requested)
    }

    func testRequestsWhenCaptureIsNotPermitted() {
        var requested = false

        let granted = ScreenCapturePermission.requestIfNeeded(
            preflight: { false },
            request: { requested = true; return true }
        )

        XCTAssertTrue(granted)
        XCTAssertTrue(requested)
    }
}
