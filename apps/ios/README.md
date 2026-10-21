# Honorum for iOS

Honorum is a native SwiftUI app for iPhone and iPad.
It uses the shared HTTP API and owns its interface, local storage, and background work.
The [application reference](../../docs/application.md#15-ios-interface) defines its behavior.

## Build and run

Open `Honorum.xcodeproj` in Xcode. Select the **Honorum** scheme.
Run the app on a simulator or device.
Run repository commands from the repository root:

```sh
make test-ios
```

The app targets iOS 18.0 with Swift 6 and complete concurrency checks.
Its bundle identifier is `app.honorum.ios`. It has no package dependencies.

`make test-ios` uses an iPhone 17 Pro simulator.
It checks dates, occurrences, catalog search, merge rules, imports, phone entry,
class colors, task source links, and Home counts.
UI tests cover setup, navigation, filters, editors, and scroll gestures.
They use synthetic data and separate local storage. They send no SMS requests.

If Xcode is not the active developer directory, select it for the command:

```sh
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer make test-ios
```

### Debug fixtures

Use these launch arguments for local tests and screenshots:

| Argument | Effect |
| --- | --- |
| `-uiTesting -demoFixture` | Uses a synthetic offline week |
| `-accountFixture` | Adds an offline account fixture |
| `-startTab schedule`, `tasks`, or `account` | Opens that screen above Home |
| `-homeClock 10:42` | Holds Home at that campus time |
| `-homeScroll schedule` or `progress` | Opens Home at that section |
| `-screen NAME` | Opens a debug screen; see `Honorum/DebugScreens.swift` |
| `-welcomePage school` | Opens that setup step |

## Release

The [iOS workflow](../../.github/workflows/ios.yml) ships two channels:

| Branch | Channel | What happens |
| --- | --- | --- |
| `beta` | TestFlight | App changes are built, signed, uploaded, and offered to the **Honorum Internal** group |
| `main` | App Store | Promoting beta submits that same TestFlight build for review; it goes live on approval |

The workflow runs when `apps/ios/Honorum`, the Xcode project, or
`apps/ios/release-notes.txt` changes. It can also be started by hand.

- Raise `MARKETING_VERSION` on beta when the next release starts.
  An approved version cannot take another build.
- Build numbers are the UTC date and that day's count, such as `2026102103`.
  CI chooses the next one above every earlier upload.
- Each TestFlight build records its source in **What to Test**: the commit and
  a hash of the app and project trees. Promotion submits the build whose hash
  matches main. Promote only after beta's iOS run has finished.
- Updates need `apps/ios/release-notes.txt`, which becomes **What's New** in
  every App Store localization. The first release has none.
- A version that is waiting for or in review cannot be replaced by CI.
  Cancel it in App Store Connect first, or raise the version.

The workflow uses these repository secrets:

| Secret | Contents |
| --- | --- |
| `APP_STORE_CONNECT_KEY_ID` | App Store Connect API key ID |
| `APP_STORE_CONNECT_ISSUER_ID` | The key's issuer ID |
| `APP_STORE_CONNECT_KEY` | The `.p8` key file's contents |
| `IOS_DISTRIBUTION_P12` | Base64 of an exported Apple Distribution certificate and key |
| `IOS_DISTRIBUTION_P12_PASSWORD` | The `.p12` export password |

The API key needs the Admin role, so it can create the App Store profile for
a new certificate. [The App Store Connect script](scripts/appstore.mjs)
finds or creates that profile, picks build numbers, labels builds, and submits.

### Local releases

The [release script](scripts/release.sh) has three modes:

| Mode | Effect |
| --- | --- |
| `verify` | Builds and checks an unsigned Release archive |
| `archive` | Builds a signed device archive |
| `upload` | Builds and uploads to TestFlight |

```sh
make verify-ios-release
HONORUM_APPLE_TEAM_ID=XXXXXXXXXX HONORUM_BUILD_NUMBER=2 make archive-ios
HONORUM_APPLE_TEAM_ID=XXXXXXXXXX HONORUM_BUILD_NUMBER=2 make upload-ios
```

Verification checks the bundle identifier, version, build number, arm64 device code,
iPhone and iPad support, encryption declaration, icon, privacy manifest, and symbols.
It rejects an App Store icon with transparency.
An unsigned archive cannot be installed or uploaded.

`MARKETING_VERSION` in the project supplies the version.
`HONORUM_IOS_VERSION` can override it.
Set a new `HONORUM_BUILD_NUMBER` for each upload.
App Store Connect rejects a repeated build number for the same version.

Sign in under Xcode > Settings > Accounts before a signed archive.
CI sets `HONORUM_PROVISIONING_PROFILE` and the `HONORUM_ASC_*` key variables
instead, which switch the script to manual distribution signing.
Pass the team and build number through the environment.
Keep private keys and certificates out of Git.
Automatic export uses Xcode-managed distribution credentials.

`Honorum/AppIcon.icon` uses Icon Composer with the shared Honorum symbol.
iOS supplies the mask and Liquid Glass appearance.
Xcode generates tinted and older-system renditions.
Verification checks the compiled icon and opaque 1024-pixel App Store image.

## Layout

| Path | What lives there |
| --- | --- |
| `Honorum/Model/` | Wire and saved types, `JSON`, campus civil dates, occurrences, the local library, Keychain, and `AppState` |
| `Honorum/Network/` | `HonorumAPI`: configuration, catalogs, seats, accounts, documents, and the LMS relay |
| `Honorum/Catalog/` | `CatalogIndex`, the actor that downloads, indexes, and searches a term |
| `Honorum/Sync/` | The document layer and the exchange with the account |
| `Honorum/Account/` | Phone sign-in, devices, and recovery copies |
| `Honorum/Alerts/` | Seat alerts: the watchlist and the checking |
| `Honorum/LMS/` | Canvas and calendar-feed imports, and the iCalendar reader |
| `Honorum/Notifications/` | Class and task reminders, and the seat announcement |
| `Honorum/Home/` | Home: the rings, today's and the week's tasks, the schedule, and the term |
| `Honorum/Schedule/` | The planner, multiple schedules, and iCalendar export |
| `Honorum/Classes/` | Class search, course detail, section rows, seat badges |
| `Honorum/Tasks/` | The task list, task rows and cards, the New sheet, and the task editor |
| `Honorum/Settings/` | Account's menu, My Classes, school, and term |
| `Honorum/Onboarding/` | First launch |
| `Honorum/Design/` | The palette, the shared kit (type, cards, rings, tiles, chips, buttons, the floating bar), and formatting helpers |
| `HonorumTests/` | Swift Testing suites |
| `HonorumUITests/` | Native UI tests |

## Behavior and contracts

The [application reference](../../docs/application.md#15-ios-interface) defines the iOS interface.
The [architecture reference](../../docs/architecture.md#7-local-state-and-sync) defines local saves and account sync.
Use the [shared fixtures](../../contracts/README.md) to check planner rules.

The app saves each owner's plan and pending edits in one atomic envelope.
It keeps session tokens and device-local LMS credentials in the Keychain.
Account connections use server storage. The app never receives account Canvas tokens.
Campus civil dates determine class occurrences and deadlines.

Screens use `AppState.preview()` for offline Xcode previews.
The interface currently uses English strings in the source files.
See [native work remaining](../../docs/architecture.md#101-work-remaining) for platform gaps.

The bundled Poppins font uses the [SIL Open Font License](../../assets/brand/OFL-Poppins.txt).
