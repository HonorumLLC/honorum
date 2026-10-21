#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../../.."

mode=${1:-verify}
case "$mode" in
  verify|archive|upload) ;;
  *) echo "Usage: $0 [verify|archive|upload]" >&2; exit 2 ;;
esac

# The project holds the version this release is for. An override has to look
# like a version, so a typo cannot quietly ship a different one than intended.
project=apps/ios/Honorum.xcodeproj/project.pbxproj
version=${HONORUM_IOS_VERSION:-$(sed -n 's/.*MARKETING_VERSION = \([^;]*\);.*/\1/p' "$project" | head -1)}
build=${HONORUM_BUILD_NUMBER:-1}
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]] || [[ ! "$build" =~ ^[1-9][0-9]*$ ]]; then
  echo "Use a numeric HONORUM_IOS_VERSION (1.0 or 1.0.0) and a positive integer HONORUM_BUILD_NUMBER." >&2
  exit 2
fi

signing=(CODE_SIGNING_ALLOWED=NO)
if [ "$mode" != verify ]; then
  if [[ ! "${HONORUM_APPLE_TEAM_ID:-}" =~ ^[A-Z0-9]{10}$ ]]; then
    echo "Set HONORUM_APPLE_TEAM_ID to your paid Apple Developer team's ten-character ID." >&2
    echo "Sign in first in Xcode > Settings > Accounts, and register app.honorum.ios." >&2
    exit 2
  fi
  if [ -z "${HONORUM_BUILD_NUMBER:-}" ]; then
    echo "Set HONORUM_BUILD_NUMBER to a build number this App Store Connect version has not used." >&2
    exit 2
  fi
  signing=("DEVELOPMENT_TEAM=$HONORUM_APPLE_TEAM_ID" CODE_SIGN_STYLE=Automatic -allowProvisioningUpdates)
fi

mkdir -p artifacts/ios
release_dir=$(mktemp -d "$PWD/artifacts/ios/$mode-$version-$build.XXXXXX")
archive="$release_dir/Honorum.xcarchive"
echo "Building Honorum $version ($build). Logs: $release_dir"
git rev-parse HEAD > "$release_dir/source-commit.txt"
git status --short > "$release_dir/source-status.txt"
if ! xcodebuild -project apps/ios/Honorum.xcodeproj -scheme Honorum \
  -configuration Release -destination 'generic/platform=iOS' \
  -derivedDataPath "$release_dir/DerivedData" -archivePath "$archive" \
  "MARKETING_VERSION=$version" "CURRENT_PROJECT_VERSION=$build" \
  "${signing[@]}" archive > "$release_dir/archive.log" 2>&1; then
  tail -60 "$release_dir/archive.log" >&2
  exit 1
fi
python3 apps/ios/scripts/verify-archive.py "$archive" "$version" "$build"
if [ "$mode" = verify ]; then
  echo "Unsigned Release archive verified: $archive"
  echo "This verifies packaging only. It cannot be installed or uploaded to TestFlight."
  exit 0
fi

codesign --verify --deep --strict "$archive/Products/Applications/Honorum.app"
echo "Signed archive: $archive"
if [ "$mode" = archive ]; then
  echo "Open this archive in Xcode, then choose Distribute App > App Store Connect."
  exit 0
fi

# Distribution signing happens here, not in the archive: automatic signing
# archives with the development identity and re-signs on export.
python3 - "$release_dir/ExportOptions.plist" "$HONORUM_APPLE_TEAM_ID" <<'PY'
import plistlib, sys
with open(sys.argv[1], "wb") as output:
    plistlib.dump({
        "method": "app-store-connect",
        "destination": "upload",
        "teamID": sys.argv[2],
        "signingStyle": "automatic",
        "manageAppVersionAndBuildNumber": False,
        "uploadSymbols": True,
        "stripSwiftSymbols": True,
    }, output)
PY
if ! xcodebuild -exportArchive -archivePath "$archive" \
  -exportOptionsPlist "$release_dir/ExportOptions.plist" \
  -exportPath "$release_dir/Export" -allowProvisioningUpdates \
  > "$release_dir/upload.log" 2>&1; then
  tail -60 "$release_dir/upload.log" >&2
  exit 1
fi
echo "Upload completed. Wait for processing in App Store Connect > Honorum > TestFlight."
