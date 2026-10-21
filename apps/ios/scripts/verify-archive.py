"""Check the assembled device archive, including metadata actually shipped to Apple."""
import json
import pathlib
import plistlib
import subprocess
import sys

archive = pathlib.Path(sys.argv[1])
app = archive / "Products/Applications/Honorum.app"
with (app / "Info.plist").open("rb") as source:
    info = plistlib.load(source)

def require(condition, message):
    if not condition:
        raise SystemExit(f"Invalid archive: {message}")

require(info["CFBundleIdentifier"] == "app.honorum.ios", "unexpected bundle identifier")
require(info["CFBundleShortVersionString"] == sys.argv[2], "marketing version did not expand")
require(info["CFBundleVersion"] == sys.argv[3], "build number did not expand")
require(info.get("CFBundleSupportedPlatforms") == ["iPhoneOS"], "expected a device build")
require(info.get("ITSAppUsesNonExemptEncryption") is False, "missing encryption declaration")
require(info.get("UIDeviceFamily") == [1, 2], "expected iPhone and iPad support")
require(info.get("CFBundleIcons", {}).get("CFBundlePrimaryIcon", {}).get("CFBundleIconName") == "AppIcon",
        "missing compiled app icon")
for name in ("Assets.car", "PrivacyInfo.xcprivacy"):
    require((app / name).is_file(), f"missing {name}")

# Apple requires a privacy manifest, and reads it rather than the app's word.
with (app / "PrivacyInfo.xcprivacy").open("rb") as source:
    privacy = plistlib.load(source)
require(privacy.get("NSPrivacyTracking") is False, "unexpected tracking declaration")
require(privacy.get("NSPrivacyCollectedDataTypes") == [], "unexpected collected data declaration")
reasons = {entry["NSPrivacyAccessedAPIType"] for entry in privacy.get("NSPrivacyAccessedAPITypes", [])}
require("NSPrivacyAccessedAPICategoryUserDefaults" in reasons,
        "the app reads UserDefaults and must declare a reason for it")

# Icon Composer produces both native layers and flattened images for older iOS.
# Inspect the compiled archive, including the opaque App Store rendition.
assets = json.loads(subprocess.check_output(
    ["xcrun", "assetutil", "--info", str(app / "Assets.car")], text=True))
icons = [asset for asset in assets if asset.get("Name") == "AppIcon"]
require(any(asset.get("AssetType") == "IconImageStack" for asset in icons),
        "missing layered app icon")
store_icons = [asset for asset in icons if asset.get("AssetType") == "Icon Image"
               and asset.get("PixelWidth") == 1024 and asset.get("PixelHeight") == 1024]
require(store_icons, "missing 1024 by 1024 App Store icon")
require(all(asset.get("Opaque") is True for asset in store_icons),
        "App Store icon must be opaque")

binary = app / info["CFBundleExecutable"]
architectures = subprocess.check_output(["xcrun", "lipo", "-archs", str(binary)], text=True).split()
require(architectures == ["arm64"], "expected arm64 device code")
require((archive / "dSYMs/Honorum.app.dSYM").is_dir(), "missing crash symbols")
print(f"Verified {info['CFBundleIdentifier']} {sys.argv[2]} ({sys.argv[3]}), arm64, iPhone/iPad, icon, privacy and symbols.")
