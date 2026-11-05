# Signing

1.0.0 waits for a signed and notarized Mac app (owner's decision, 25 October 2026). Today the Mac app
is ad-hoc signed, so macOS asks for confirmation on first launch. Windows is not signed (owner's
decision, 5 November 2026, see below). Linux packages need no signing.

The owner joined the Apple Developer Program on 5 November 2026, with a separate Apple ID kept for this work.

## macOS

Every build is signed and notarized again, because the signature covers the exact bytes of that build.
CI does this on every push to `main` and every tag, so a release needs no manual signing step.

### One-off setup (owner)

1. **Certificate.** Xcode, Settings, Accounts: add the Apple ID, select the team, Manage Certificates,
   **+**, **Developer ID Application**. The certificate and its private key land in the login keychain.
   If Xcode says "only the Account Holder" (it did on 5 November 2026, right after enrolment), use the
   website instead: Keychain Access (in `/System/Library/CoreServices/Applications/`), Certificate
   Assistant, Request a Certificate From a Certificate Authority, saved to disk; then Certificates, **+**,
   Developer ID Application, G2 Sub-CA, upload, download, and import the `.cer`. If
   `security find-identity -v -p codesigning` then lists no valid identity, the Apple intermediates are
   missing: import `DeveloperIDG2CA.cer` and `AppleWWDRCAG3.cer` from https://www.apple.com/certificateauthority/.
2. **Export it.** The login keychain holds other identities (ExpressVPN, localhost), so export only
   this one: Keychain Access, login, My Certificates, right-click "Developer ID Application: …",
   Export, as `.p12` with a password. Keep it outside the project.
3. **App-specific password.** https://account.apple.com, Sign-In and Security, App-Specific Passwords,
   **+** (name it "PDF Epsilon notarize").
4. **Run `tools/signing/setup-mac.sh`** in a terminal. It reads the certificate name and Team ID from the
   keychain, asks for the `.p12` path and password, the Apple ID and the app-specific password, checks
   them with Apple's notary service, and stores these repository secrets with `gh`:

   | Secret | Value |
   |---|---|
   | `APPLE_CERTIFICATE` | the `.p12`, base64-encoded |
   | `APPLE_CERTIFICATE_PASSWORD` | its password |
   | `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <name> (<team id>)` |
   | `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | the Apple ID, the app-specific password, the Team ID |

5. Start a build (`gh workflow run build.yml --ref main`) and try the downloaded `.dmg` on a Mac.

### Upkeep

- The membership (USD 99, in Korea ₩129,000 a year; individual, Team ID `65YC7X7G72`) must be active
  to sign and notarize a new build. It renews on 6 November 2027. Versions already released keep
  opening if it lapses.
- The certificate lasts 5 years. On renewal, repeat steps 1, 2 and 4.
- Changing the Apple ID password revokes app-specific passwords: repeat steps 3 and 4.

### How CI uses it (`build.yml`)

- "Mac signing secrets" hands the secrets to Tauri only when they exist. Without them, builds stay
  ad-hoc signed through `"signingIdentity": "-"` in `tauri.conf.json` (the `APPLE_SIGNING_IDENTITY`
  variable overrides it; checked 5 November 2026). A `v*` tag without the secrets fails, so a release
  is never unsigned.
- Tauri signs the app with the Developer ID, sends it to Apple's notary service and staples the ticket,
  then builds the `.dmg` around it.
- "Check the Mac signature" checks the app and the copy inside the `.dmg`: `codesign --verify`, the
  Developer ID authority, `xcrun stapler validate`, and `spctl --assess` must report "Notarized
  Developer ID" when Gatekeeper is on.

### Once the first signed build is confirmed

- The download page (`site/`), README and release notes lose the macOS first-launch steps. The
  Windows steps stay.
- Not yet done: SHA-256 checksums published with every release.

Then a release candidate is built from `main`, checked on all three platforms, and tagged `v1.0.0`.

## Windows: not signed

Owner's decision (5 November 2026): less hassle without it. Since 2023 a Windows certificate needs a
hardware token or a paid cloud signing service (Azure Trusted Signing, or an OV certificate with a CA's
cloud signing), and SmartScreen still warns a new app for weeks until it builds up reputation, so the
gain is small. SmartScreen keeps showing "Windows protected your PC"; the download page and README keep
the More info, Run anyway steps. If this is revisited, the routes are those two, wired in through
`bundle.windows.signCommand`.
