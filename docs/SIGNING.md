# Signing

1.0.0 waits for signed installers (owner's decision, 25 October 2026). Today the Mac app is ad-hoc
signed, so macOS asks for confirmation on first launch, and Windows SmartScreen warns about an unknown
publisher. Linux packages need no signing.

## What the owner needs

**macOS: Apple Developer Program** (USD 99 a year, https://developer.apple.com/programs/).

1. Enrol as an individual. Approval can take a day or two.
2. In Xcode or the developer site, create a **Developer ID Application** certificate and export it
   with its private key as a `.p12` file, with a password.
3. Create an **app-specific password** for your Apple ID at https://account.apple.com (used to
   notarize: Apple scans the app and vouches for it, so it opens without the warning).
4. Note your **Team ID** (10 characters, on the membership page).

**Windows: a code-signing certificate.** Since 2023 the private key must live in hardware or a cloud
service, so a plain `.pfx` file is no longer sold. The two practical routes:

- **Azure Trusted Signing** (Microsoft's cloud signing, about USD 10 a month). Cheapest and works from
  CI, but Microsoft validates your identity first, and eligibility for individuals has varied by country:
  check it for Korea before paying.
- **An OV certificate from a certificate authority** (for example Certum, Sectigo or SSL.com, roughly
  USD 100–400 a year) with the authority's cloud signing service, so CI can sign without a USB token.

Either way SmartScreen may still warn for the first weeks, until the signed app has built up
reputation from downloads. No certificate removes that at once any more.

## What then changes in the project

Add these as repository secrets (Settings, Secrets and variables, Actions). The keys are what
`tauri-action` reads:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | the `.p12` file, base64-encoded (`base64 -i cert.p12 \| pbcopy`) |
| `APPLE_CERTIFICATE_PASSWORD` | its password |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <name> (<team id>)` |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | your Apple ID, the app-specific password, the Team ID |

Then, in one change:

- `tauri.conf.json`: drop `"signingIdentity": "-"`, so the Developer ID is used.
- `build.yml`: pass the secrets to `tauri-action`; check with `spctl --assess` and `xcrun stapler validate`
  instead of only `codesign --verify`; add the Windows signing step for the chosen route
  (`bundle.windows.signCommand`); publish SHA-256 checksums with every release.
- The download page and release notes lose the first-launch steps once signed builds are out.

Then a release candidate is built from `main`, checked on all three platforms, and tagged `v1.0.0`.
