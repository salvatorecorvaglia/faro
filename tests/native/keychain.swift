import Foundation
import Security

// Only this process disables prompts. A private disposable keychain is never
// made the user's default, and deletion removes it from the search list.
let directory = CommandLine.arguments[1]
let path = directory + "/faro-test.keychain-db"
let password = "faro-disposable-keychain"
var keychain: SecKeychain?
func fail(_ message: String) -> Never {
    if let keychain = keychain { SecKeychainDelete(keychain) }
    SecKeychainSetUserInteractionAllowed(true)
    fputs(message + "\n", stderr)
    exit(1)
}
func require(_ status: OSStatus, _ operation: String) {
    guard status == errSecSuccess else { fail("\(operation): OSStatus \(status)") }
}
require(SecKeychainSetUserInteractionAllowed(false), "Disable prompts in test process")
require(SecKeychainCreate(path, UInt32(password.utf8.count), password, false, nil, &keychain), "Create disposable keychain")
defer { SecKeychainDelete(keychain); SecKeychainSetUserInteractionAllowed(true) }
let service = "faro-disposable-validation"
let account = "test-only"
let secret = "disposable-test-value"
var item: SecKeychainItem?
require(SecKeychainAddGenericPassword(keychain, UInt32(service.utf8.count), service, UInt32(account.utf8.count), account, UInt32(secret.utf8.count), secret, &item), "Store")
func readStatus() -> OSStatus {
    var count: UInt32 = 0
    var bytes: UnsafeMutableRawPointer?
    let status = SecKeychainFindGenericPassword(keychain, UInt32(service.utf8.count), service, UInt32(account.utf8.count), account, &count, &bytes, nil)
    if status == errSecSuccess {
        defer { SecKeychainItemFreeContent(nil, bytes) }
        guard let bytes = bytes, String(data: Data(bytes: bytes, count: Int(count)), encoding: .utf8) == secret else { fail("Secret round trip failed") }
    }
    return status
}
require(readStatus(), "Read")
require(SecKeychainLock(keychain), "Lock disposable keychain")
let denied = readStatus()
guard denied != errSecSuccess else { fail("Locked keychain unexpectedly allowed access") }
guard SecKeychainUnlock(keychain, 5, "wrong", true) != errSecSuccess else { fail("Wrong password unlocked keychain") }
require(SecKeychainUnlock(keychain, UInt32(password.utf8.count), password, true), "Unlock disposable keychain")
require(readStatus(), "Read after unlock")
require(SecKeychainItemDelete(item!), "Delete disposable credential")
print("Disposable macOS keychain passed: round trip, locked access denied (\(denied)), wrong-password refusal, unlock, deletion")
