#!/bin/bash
# One-off: hands the Mac signing credentials to GitHub, so CI signs and notarizes every build.
#
# Before running it (see docs/SIGNING.md):
#   1. Xcode > Settings > Accounts > Manage Certificates > + > Developer ID Application.
#   2. An app-specific password from https://account.apple.com.
#
# What it does:
#   - finds the Developer ID Application certificate in the login keychain;
#   - exports it with its private key to a temporary .p12 under a random password;
#   - checks the Apple ID and app-specific password with Apple's notary service;
#   - stores the six APPLE_* repository secrets with `gh`, then deletes the .p12.
#
# Nothing secret is written to the project or printed. Run it in a terminal: macOS asks for
# your login password once, to allow the private key to be exported.
set -euo pipefail

REPO="yusungchang/pdf-epsilon"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

die() { echo "Error: $*" >&2; exit 1; }

command -v gh >/dev/null || die "the GitHub CLI (gh) is not installed."
gh auth status >/dev/null 2>&1 || die "gh is not logged in. Run: gh auth login"

# --- 1. The certificate -------------------------------------------------------------------------
# Lines look like:  1) 0123ABCD... "Developer ID Application: Name (TEAMID1234)"
ids=$(security find-identity -v -p codesigning "$KEYCHAIN" | grep '"Developer ID Application:' || true)
[ -n "$ids" ] || die "no Developer ID Application certificate in the login keychain.
Create one in Xcode: Settings > Accounts > Manage Certificates > + > Developer ID Application."

if [ "$(printf '%s\n' "$ids" | wc -l)" -gt 1 ]; then
  # Several (for example an old one and a renewed one): let the owner pick.
  echo "Several Developer ID Application certificates were found:"
  printf '%s\n' "$ids" | sed -E 's/^ *[0-9]+\) [0-9A-F]+ /  /' | nl
  read -rp "Which one? (number) " pick
  ids=$(printf '%s\n' "$ids" | sed -n "${pick}p")
  [ -n "$ids" ] || die "no such entry."
fi

IDENTITY=$(printf '%s' "$ids" | sed -E 's/^[^"]*"(.*)"$/\1/')
TEAM_ID=$(printf '%s' "$IDENTITY" | sed -E 's/.*\(([A-Z0-9]{10})\)$/\1/')
[[ "$TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || die "could not read the Team ID from: $IDENTITY"
echo "Certificate: $IDENTITY"
echo "Team ID:     $TEAM_ID"

# --- 2. Export it as .p12 -----------------------------------------------------------------------
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
P12="$TMP/cert.p12"
P12_PASSWORD=$(/usr/bin/openssl rand -base64 24)

# `security export` cannot pick one identity: it exports every identity in the keychain. Use it
# only when the Developer ID is the only one, so no unrelated private key ends up on GitHub.
# Otherwise the owner exports just that certificate from Keychain Access.
all=$(security find-identity "$KEYCHAIN" | awk '/identities found/ { print $1; exit }')
if [ "$all" = "1" ]; then
  echo "Exporting the certificate (macOS asks for your login password)..."
  security export -k "$KEYCHAIN" -t identities -f pkcs12 -P "$P12_PASSWORD" -o "$P12" >/dev/null
else
  echo "The login keychain holds $all identities, so export only the Developer ID by hand:"
  echo "  Keychain Access > login > My Certificates > right-click \"$IDENTITY\" > Export, as .p12."
  echo "  (Keychain Access names it Certificates.p12 unless you rename it.)"
  read -rp "Path of the exported .p12 (or its folder): " path
  # Accept what Terminal produces for a dragged-in file: surrounding quotes, "\ " for spaces,
  # a trailing space. Then expand a leading ~.
  path=$(printf '%s' "$path" | sed -E "s/^[[:space:]]*['\"]?//; s/['\"]?[[:space:]]*\$//; s/\\\\ / /g")
  path="${path/#\~/$HOME}"
  if [ -d "$path" ]; then
    # A folder: use the .p12 in it when there is exactly one.
    found=$(find "$path" -maxdepth 1 -iname '*.p12' -type f)
    [ -n "$found" ] || die "no .p12 file in $path"
    [ "$(printf '%s\n' "$found" | wc -l)" -eq 1 ] || die "several .p12 files in $path; give the full path of one:
$found"
    path=$found
    echo "Using $path"
  fi
  [ -f "$path" ] && [ -s "$path" ] || die "no file at $path"
  read -rsp "Its password: " P12_PASSWORD; echo
  cp "$path" "$P12"
fi
[ -s "$P12" ] || die "the export produced no file."
# Import it into a throwaway keychain, as CI does, to confirm that the password opens it, that it
# holds exactly one signing identity (so nothing unrelated goes to GitHub), and that it is the
# Developer ID found above. This avoids openssl, which may not read every format Keychain writes.
CHECK_KC="$TMP/check.keychain-db"
security create-keychain -p "$(/usr/bin/openssl rand -hex 16)" "$CHECK_KC"
trap 'security delete-keychain "$CHECK_KC" 2>/dev/null; rm -rf "$TMP"' EXIT
security import "$P12" -k "$CHECK_KC" -f pkcs12 -P "$P12_PASSWORD" >/dev/null 2>&1 \
  || die "the .p12 does not open with that password."
inside=$(security find-identity -p codesigning "$CHECK_KC")
n=$(printf '%s\n' "$inside" | awk '/identities found/ { print $1; exit }')
[ "$n" = 1 ] || die "the .p12 holds ${n:-no} signing identities; export only \"$IDENTITY\"."
printf '%s\n' "$inside" | grep -F "\"$IDENTITY\"" >/dev/null \
  || die "the .p12 does not hold \"$IDENTITY\"."
echo "The .p12 holds the Developer ID certificate and its key."

# --- 3. Apple ID and app-specific password ------------------------------------------------------
# Asked for, never written in this file: the repository is public.
read -rp "Apple ID (email): " APPLE_ID
[ -n "$APPLE_ID" ] || die "no Apple ID given."
read -rsp "App-specific password (xxxx-xxxx-xxxx-xxxx): " APPLE_PASSWORD; echo

# A harmless call to the notary service proves the three values work together, so a typo
# shows up now instead of as a failed release build.
echo "Checking with Apple's notary service..."
xcrun notarytool history --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$TEAM_ID" >/dev/null \
  || die "Apple refused the Apple ID, app-specific password or Team ID."

# --- 4. Repository secrets ----------------------------------------------------------------------
# Values go through stdin, never the command line, so they do not show in the process list.
put() { printf '%s' "$2" | gh secret set "$1" --repo "$REPO" >/dev/null && echo "  set $1"; }
echo "Storing secrets in $REPO..."
put APPLE_CERTIFICATE "$(base64 -i "$P12" | tr -d '\n')"
put APPLE_CERTIFICATE_PASSWORD "$P12_PASSWORD"
put APPLE_SIGNING_IDENTITY "$IDENTITY"
put APPLE_ID "$APPLE_ID"
put APPLE_PASSWORD "$APPLE_PASSWORD"
put APPLE_TEAM_ID "$TEAM_ID"

echo "Done. The next build on main is signed and notarized."
echo "Start one now with: gh workflow run build.yml --ref main"
