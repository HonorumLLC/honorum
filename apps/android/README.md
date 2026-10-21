# Honorum for Android

The Android app is planned. This directory has no executable app yet.
Use Kotlin and Jetpack Compose for the native client.
Keep its interface, local storage, background work, and platform integrations separate from the other clients.

Read the [application reference](../../docs/application.md) and
[API contracts](../../docs/architecture.md) before implementation.
Use the [shared planner fixtures](../../contracts/README.md) and
[brand assets](../../assets/brand/README.md).

Preserve optional accounts, offline plans, atomic local saves, pending sync edits,
conflict recovery, and campus dates.
Keep guest LMS credentials and feed URLs on the device.
Use platform-protected storage for session tokens.
Account Canvas tokens stay encrypted on the backend and never return to clients.

Start with a committed Gradle wrapper, pinned dependencies, an explicit minimum SDK,
and unit and UI tests.
Choose the application ID before distribution. Keep private signing material out of Git.

The API supports bearer authentication and document sync.
The server supports Web Push and APNs. Android FCM support still needs backend work.
