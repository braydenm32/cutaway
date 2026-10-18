// Installs or removes the Cutaway Virtual Camera system extension.
// Lives inside Cutaway.app (Contents/MacOS/cutaway-camera) so macOS knows
// which app the extension belongs to. Usage: cutaway-camera install|uninstall

import Foundation
import SystemExtensions

final class Delegate: NSObject, OSSystemExtensionRequestDelegate {
    func request(_ request: OSSystemExtensionRequest, actionForReplacingExtension existing: OSSystemExtensionProperties, withExtension ext: OSSystemExtensionProperties) -> OSSystemExtensionRequest.ReplacementAction {
        return .replace
    }
    func requestNeedsUserApproval(_ request: OSSystemExtensionRequest) {
        print("approval: Allow Cutaway in System Settings → General → Login Items & Extensions → Camera Extensions")
        fflush(stdout)
    }
    func request(_ request: OSSystemExtensionRequest, didFinishWithResult result: OSSystemExtensionRequest.Result) {
        print(result == .completed ? "done" : "done: restart needed")
        exit(0)
    }
    func request(_ request: OSSystemExtensionRequest, didFailWithError error: Error) {
        print("error: \(error.localizedDescription)")
        exit(1)
    }
}

let id = "app.cutaway.Cutaway.Camera"
let delegate = Delegate()
let request: OSSystemExtensionRequest
switch CommandLine.arguments.dropFirst().first {
case "uninstall": request = .deactivationRequest(forExtensionWithIdentifier: id, queue: .main)
default: request = .activationRequest(forExtensionWithIdentifier: id, queue: .main)
}
request.delegate = delegate
OSSystemExtensionManager.shared.submitRequest(request)
// Give up after two minutes (the user may never approve).
DispatchQueue.main.asyncAfter(deadline: .now() + 120) {
    print("error: timed out waiting for approval")
    exit(2)
}
dispatchMain()
