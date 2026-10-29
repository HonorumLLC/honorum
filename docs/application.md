# Application behavior and data

This reference defines the planner behavior, accounts, privacy, and client interfaces.
Read [architecture](architecture.md) for API contracts and server limits.
Read [deployment](deployment.md#1-configuration) for settings.

## 1. Local planning and catalogs

The Svelte client stores schedules and tasks in IndexedDB. Each saved section
includes the meeting details needed to render the calendar without waiting for
a catalog download. Seat counts come from `GET /api/seats?inst=&term=&ids=`,
together with each section's published exam sittings, so the planner keeps
saved sections current without the catalog.

The server publishes independently versioned structure, seat, and description
JSON files with gzip and Brotli copies
([catalog publication](architecture.md#44-catalog-publication)).
The client caches them in IndexedDB and searches them in
`apps/web/src/lib/catalog.worker.ts`. A term's catalog loads when a student
picks the school, edits classes, or opens a class, not on every planner visit.
Cached catalogs become searchable while the worker checks for updates. With no
saved copy, seat counts download after the structure that makes the term
searchable. Conflict detection runs in the browser, and a
separate worker searches for alternative section combinations with limits on
search time and result count.

Snapshots omit repeated course, term, and institution fields. Descriptions and
prerequisites load when a student opens a class. `/api/config` provides independent
content hashes and section counts. Cached seat data preserves offline open-seat
filtering, and missing counts are unknown.

Every public file has an immutable URL based on its content hash.
The server rebuilds a missing current file from the catalog database when a request needs it. Search pages
contain at most 40 section rows, with Previous and Next controls for the
remaining matches.

## 2. Collection and alerts

Each institution has separate catalog and seat jobs. The server allows two
catalog jobs and four seat jobs at once, with paced requests to providers.
Catalog jobs release capacity between terms; ended terms refresh weekly after
a two-week grace period. Missing dates retain the ordinary cadence.

TAMU has
a five-minute minimum seat interval, and institution settings can increase it.
Failed jobs back off for up to an hour; one term failing does not stop seat
checks for the school's other terms. Watched sections share availability
checks across subscribers. The limit is 200 watched sections per account, or
per notification destination without an account.

The server stops seat checks for ended terms.
The hourly sweep removes their alerts.
For schools without term dates, it removes alerts after one year.

A known change from closed to open creates delivery jobs in the same database
transaction as the seat update. Alerts use push notifications. A guest watch names one browser subscription.
An account watch reaches every signed-in device with notifications on.

Browsers use Web Push; iPhones use APNs. Each push device has its own retry state. A
crash after the push service accepts a notification but before the database records
success can cause a duplicate. Push responses of 404 or 410 remove expired
subscriptions; other delivery failures can retry.

If a browser's push keys change, the client can replace its subscription and
move watches and reminders through `POST /api/push/retarget`.

## 3. Tasks and imports

### 3.1 Manual tasks

New manual tasks on the web default to today at 11:59 pm in the campus time
zone. Students can change the deadline or clear the due date before saving.

### 3.2 Guest Canvas imports

Guest Canvas imports use `POST /api/lms/canvas`. The student supplies a Canvas URL and
personal access token from Account > Settings > New Access Token. The server
relays requests to Canvas and returns assignments with course names, due dates,
and completion state. Course calendar events also arrive as tasks.

Their due time is the event start.
An all-day event starts at campus midnight.
Tasks at Their Due Time places these tasks on the calendar hours. `POST /api/lms/canvas/complete` sends completion changes.

The web client requests a stream.
Courses and planner items become usable tasks before module completion checks finish.
The final tasks follow those checks. Clients that do not ask for the stream, such as the iOS
app, receive the finished tasks in one response. Completion writes are not
automatically retried. The stream format and request budgets are in
[guest endpoints](architecture.md#62-guest-endpoints).

The first event makes assignments usable and finishes Canvas connection setup.
Existing tasks and module metadata remain until a complete response permits
reconciliation. An interrupted completion check leaves the initial assignments
visible, preserves earlier tasks, and leaves manual refresh available. Saved edits, open
editing drafts, and checkbox changes survive the final update.

Checkbox writes
made during import wait for module mappings while the local check changes
immediately. Unacknowledged checkbox changes stay on the saved task and retry
after the next successful Canvas import, including after a reload or tab
handover. A failed web completion write reports a possible expired Canvas token.
Reconnect opens Settings > Assignments so the student can replace it.

Canvas authentication failures do not reliably distinguish
expiration from a deleted token or other authorization failures. When the
server cannot be reached, the message gives that reason instead. Pending changes
request a refresh even within the freshness window.

### 3.3 Calendar feeds

Guest calendar imports use `POST /api/lms/feed`. The server fetches an
iCalendar feed, and the browser parses it into tasks. For accounts, the server saves an encrypted calendar link.
It imports tasks for every device and refreshes them in the background.
Removal of the calendar also removes its imported tasks. The connection dialog explains how to find feeds for the
supported assignment services.

### 3.4 Refresh scheduling

Connecting a source fetches only that source. Ordinary refreshes fetch Canvas
and calendars independently, with up to three calendar requests at once. A
calendar collection is replaced only after all its feeds succeed. Concurrent
refresh triggers share existing imports.

The active tab checks freshness at startup, on return to the foreground, and after network recovery.
It also checks once a minute while visible.
Successful sources refresh automatically at most once every five minutes.
A tab handover preserves that window.

Manual refresh always checks
connected sources. Tab ownership changes cancel imports and invalidate their late
results.

### 3.5 Credentials and account connections

Guest Canvas tokens and calendar feed URLs stay on the device where they were
entered and pass through the relay when used. The relay accepts only public
HTTPS destinations and reads at most 4 MiB per response.

When signed in, connecting Canvas saves an encrypted token with the account.
Every signed-in device can then refresh assignments and send completion changes.
The server coordinates imports and publishes tasks through account document sync;
devices receive connection status, never the token. Connecting explains this storage
change.

Existing local connections are not uploaded automatically: reconnect Canvas
using "Use Canvas on All Devices". A shared account connection takes precedence over
an existing local connection. Guest use remains available.

Account Canvas refreshes share a five-minute freshness window; manual refresh checks
Canvas immediately while keeping recent module mappings. A check uploads pending edits first.
It requests Canvas and calendar refreshes together. Each response includes the connection state.

It downloads documents only after an import or an explicit student request. The server refreshes account connections about once an hour while an account device has recent activity.
Assignments and reminders can then arrive while the app stays closed. An ordinary refresh reads the assignment planner and course list.

The slower module scan runs on connect, once a day, or after an explicit `rescan_modules` request. The web Sync button and iOS
pull-to-refresh fetch assignments without forcing this scan. Device-local Canvas
connections scan modules on every import. A complete import preserves notes,
course choices, changed due dates, and pending completion edits.

A course
choice belongs to the connection that supplied the course, so the same Canvas
course id on another site never inherits it. Duplicate imports become labeled
manual recovery copies.
Disconnecting an account connection removes its credential on the server and
stops access for every device, while preserving assignments. Reconnecting to
another Canvas origin requires disconnecting first; old tasks remain as
recovery copies.

A guest disconnect removes that device's imported tasks.

### 3.6 Reconciliation

Canvas pagination, including redirects carrying tokens, stays on the original
origin, and each operation has bounded pages, requests, bytes, and time
([relay rules](architecture.md#62-guest-endpoints)). Failed or incomplete
collections cannot delete existing local tasks. The browser allows thirty-five
seconds for the Canvas response and rejects incomplete streams.

Imports match tasks by upstream ID, preserve course and due date overrides,
and remove open items that disappear upstream after a complete import. Canvas's
completion wins unless a tick is still waiting to reach Canvas; a calendar feed
only adds completion, so a personal tick stays. An open item an import removed
comes back if the source lists it again; one the student deleted does not.
Floating and whole-day feed times use the item's `TZID`, or the campus time zone when it has none.

All displayed due times use campus time. A due date
the student changes stays until the source moves its own, which then replaces
it. Canvas keeps the existing window of seven days back through 120 days ahead;
calendar imports ignore items due more than two weeks ago. A finished task stays after the source stops listing it.

The student must delete it or clear completed tasks to remove it.

Background imports and notifications use persistent queues in production;
queued work resumes after a worker restart. Provider limits can delay
automatic imports under load.

## 4. Reminders

When reminders are enabled for an account, the server derives them from the
account's own documents, so they continue while no device opens the app. A
guest browser uploads meeting patterns, campus time zones, and upcoming task
due times instead, and renews that window while it is open. The server sends
class reminders 15 minutes before a meeting and task reminders an hour before
the due time. Turning reminders off syncs the preference or deletes the guest
registration. Web and iOS offer one Reminders switch for both kinds, which says
when each arrives.

## 5. Classes from Canvas

The server matches a student's Canvas courses to the school catalog.
Guests use `POST /api/lms/canvas/classes` with a device token.
Accounts use `POST /api/me/canvas/classes` with the stored token.
It reads the student's active courses with their sections, terms, and
teachers.

Schools use different course names, such as "CS 1213-010", "2026FA-CS-1213-010", "01:198:111:03", or "COMPSCI 161 LEC A (34250)".
The match reads all Canvas names for course codes, section numbers, and five-digit registration numbers.
It accepts different separators. Most catalogs use registration numbers as section IDs.

The student's own section
names count before the course's, since a course combining several sections
names all of them. Without a named section, the match uses the instructor, then a unique course section.
The student chooses unresolved alternatives and linked labs. Courses the catalog lacks, such as orientation
sites, are left out, as are courses from another term.

The match first uses a named term, such as "Fall 2026", "2026 Fall", or "FA26".
It then tries term dates, then the displayed planner term.

The confirmation lists each class with a switch, section numbers, times, and menus for unresolved choices.
A resolved section enables its class. A menu selection can also enable it. Adding puts the sections on the term's open schedule and matches each Canvas course
to its class, so its assignments take the class's color. On the web,
Settings > Schedule offers Add Classes from Canvas whenever Canvas is
connected, for a new term.

## 6. Academic calendars

`crates/server/src/closures.json` records school holidays, breaks, and replacement class
days, with source URLs. The checked-in calendars cover Fall 2026 through Spring
2027, including quarters where applicable. They need updates as schools publish
new calendars. Some days off are announced outside the registrar's calendar, such
as Oklahoma's Fall Student Holiday, which the Provost sets by memo each August.
An institution's `closures` setting can replace its built-in list.

Cancelled class days do not appear in calendar exports or trigger class
reminders. Personal events still appear. A `follows` entry substitutes another
weekday's classes, such as Monday classes on a Tuesday. An `after` entry
cancels only the classes that start at or after that campus time, in minutes
after midnight, such as Oklahoma's 3 p.m. closing for a campus concert; earlier
classes meet, including one still in session at that time.

The web calendar shades the hours after the closure time.
Its chip reads "from 3:00 PM". The phone heading reads "No classes from 3:00 PM". Home's milestones leave such
afternoons out, since the rest of the day goes on.

## 7. Exams

Some schools publish exam sittings with sections.
Texas A&M supplies dated Examination rows. Oklahoma ClassNav supplies an exam column.
Rutgers supplies final exam notes.

Banner supplies exam meeting rows with one date. Adapters turn these into `exams` on the section (a
date, start and end minutes, and a location); the other adapters have no
published exam data. Snapshots and saved sections include exam sittings.
Seat responses and catalog loads refresh them, so later exam changes appear in saved schedules.

An exam appears on its date in the class color with a ruled edge and a label such as "CS 101 Exam".
Class details also list it. Exam
sittings are dated facts about a class, so campus closures never cancel them
and they never count as overlaps or affect Rearrange Sections. Hide This Exam
takes one sitting off the calendar (a `removed` entry keyed by
`<section>:exam:<index>`), with undo. Exams are exported as single calendar
events and announced like class meetings by web, iOS, and account reminders.

## 8. Personal events

An event happens once on a date or every week on chosen days, with a name,
times, a location, and notes. An event can use its class color or a selected palette color.
Otherwise, it uses the neutral event color. Schedules store events as `blocks` with `note`, `course`, and `color` fields.
A one-time event keeps its date's weekday letter in `days`, because the planner and reminders place an event only on the days it lists.

Web and iOS editors offer the same choices. Calendar exports use notes as the event description. Events never count as overlapping classes.

## 9. Google Calendar

A signed-in account can connect one Google account.
Each of the two independent directions requests its own Google permission on first use. Nothing is edited in both
places: Honorum owns the calendar it publishes, and Google owns the events
it shows, so the two never need reconciling.

Add Classes to Google Calendar creates a calendar named Honorum.
The `calendar.app.created` permission reaches only calendars that Honorum created.
The calendar follows every term's primary schedule, including classes, exams, and personal events.
Include Due Dates also adds open tasks as free half-hour events that end at their due time.

Class meetings become weekly series in campus time and end with the term.
They exclude removed days and closures.
Substitute weekdays and exams use single events. A term needs a few dozen events.

The server publishes changes within about half a minute and checks the calendar daily.
It restores changed or deleted Honorum events. It leaves student-created events alone.
Deleting the Honorum calendar in Google turns the switch off with a note, and
turning the switch off deletes the calendar.

Show Google Events reads the student's calendar list and the events of the
calendars they choose (`calendar.calendarlist.readonly` and
`calendar.events.readonly`). The main calendar is chosen at first. The server copies events from two weeks back to four months ahead.
Google expands repeated events.

The server omits cancelled or declined events and names untitled events Busy. Google notifications trigger a fresh calendar fetch within seconds.
A six-hour poll also checks calendars with notifications.
Without notifications, the server polls every fifteen minutes while a device views the calendar.

Devices poll every minute while visible and fetch at once
when the live stream reports a new version. Web timed events use a pale calendar color and an edge bar.
They split at campus midnight and appear beside classes, including during Edit Classes.
They do not count as class overlaps.

All-day events stay in Google Calendar. Selecting one shows
its time, place, calendar, and Open in Google Calendar.

The switches sit on Settings > Schedule under Google Calendar, with the
connected Google account, Disconnect, Include Due Dates, and a switch for each
calendar. A guest sees Sign In instead. A switch moves only after the server
agrees; turning Add Classes off asks first, because it deletes the calendar in
Google. Disconnecting deletes the Honorum calendar, stops notifications,
revokes Honorum's access at Google, and removes everything stored.

Settings reports revoked access, expired credentials, and removed permissions.
Connect Again requests consent for every enabled direction together.
Clearing a permission on Google's consent screen leaves its switch off with a
note. The iOS app does not offer Google Calendar yet; an account's Honorum
calendar still follows edits made there, since the server publishes it.

## 10. Accounts and sync

Accounts are optional. Sign-in requires a mobile number and a six-digit SMS
code. Each device has a session that the student can revoke in Settings.
Twilio Verify sends and checks codes; Honorum owns accounts and sync. Sign-in
finishes after local ownership is saved, and background sync retries independently.

Each schedule, task, and preferences document has its own revision. The client
combines independent edits, including personal events added or changed on
different devices. If two devices change the same field, it keeps the local
edit and saves the remote copy for recovery in Settings. Device clocks do
not choose the winner. Pending edits persist in an outbox alongside saved data.

The web app syncs after saves, on focus, after network recovery, and once a minute while visible.
Its live stream announces other devices' changes within a second. Unchanged polls leave saved state alone.
Deletion records remain until the account is deleted so an offline device
cannot restore a deleted document. The server keeps each replaced version of a
document for thirty days; Settings can restore an earlier version of the open
schedule. Accounts and local caches stay isolated during sign-in and sign-out.

Share links encrypt a schedule with a new secret before uploading it. The link
contains that secret, and opening it imports a copy. The server stores encrypted
share contents; account sync stores JSON documents. Endpoints, ownership
checks, request limits, and client requirements are in
[document sync](architecture.md#64-document-sync-and-live-events) and
[local state and sync](architecture.md#7-local-state-and-sync).

## 11. Progress rings

Web Tasks starts with a Progress card once a task exists. In the side pane it
stays in view above the task search while the list scrolls.
On iOS, the rings appear on Home instead. On
the web the card is always open, with one even inset around its heading and
rings. The streak is the heading, set off from the rings by a hairline across
the card. Below it, three nested rings each have a row beside them: Today
outside, This Week, and This Term inside.

On the web each row leads with its label and count and ends with its
percentage at the card's edge. The rings take the room the rows leave, between
84 and 120 pixels, so they scale smoothly with the card. Long counts wrap only
once the rings are at their smallest.
Today holds tasks due today and This Week tasks due in the calendar week,
which starts on Monday or
Sunday like the calendar. Both also hold overdue tasks that are still open or
were finished during the period, so late work stays in view until it is done.

This Term holds tasks due between the term's first and last days. Each device keeps a record for each account, school, and term.
It preserves the last task state after an import omits old tasks.
Canvas omits open items one week after their deadline; feeds omit them after two weeks.

A task that disappears within one week of its deadline counts as an upstream removal.
It leaves the count. Student-deleted tasks also leave the count. Tasks without a due date are not counted.

The rings follow the class the task filter picks. A period with nothing due
shows a full ring and "Nothing due". Choosing a ring or its row shows that ring
alone with its count in the middle; choosing it again or Escape brings back all
three. The last completion shows "Day complete", "Week complete", or "All done" for the term.

A closed ring expands, glows briefly, and draws a check.
On the web, light mode uses dusty red for Today, pale apricot for This Week,
and soft blue for This Term. The landing
page's preview uses the same palette. Dark mode uses coral, amber, and slate blue.

The heading reads like "30 day streak" in the card's title type, with this
week's days as dots at the card's edge in the calendar's week order and today
outlined. While tasks are overdue, today's dot turns red; the heading does not
change. The heading's tooltip gives the best streak, today's dot's tooltip
gives the overdue count, and screen readers hear both. Guests see Keep a
Streak and Sign In; an account whose streak has not arrived yet sees Connect
to update your streak. Every state has the same height, so the rings never
move. The flame, the heading's capitals, and the dots share one center line.
The flame's widest point meets the rings' outer edge, and the last dot or Sign
In ends where the percentages' ink does.
On the web, clean-day dots and today's outline use apricot `#eba363` in light
mode.

Dark mode keeps the warm accent dots and lighter accent outline.

iOS nests the rings beside the Home date.
Each ring also heads Today, This week, or the term, with completed and total counts.
They count the same way for every class, on the planner's Sunday-to-Saturday
week, and the term record lives in the app's defaults. A closed ring draws a
check. Account's profile card shows the streak page: the same count, best,
overdue count, and four-week dots for a signed-in account.

Card animations settle within about one second.
Ring arcs appear from the outer ring inward. Streak days appear in date order.
Numbers count up to their values. Text after a count holds still while it
runs; when the final value is wider or narrower, the text glides there on the
count's curve.

A new streak day moves
today's ring to the next dot as the count goes up. A new week's days arrive
afresh. Falling behind turns today's dot red for two slow breaths. Reduce
Motion shows the final state at once.

## 12. Streaks

An account streak counts consecutive days that ended with nothing overdue.
By account-local midnight, every task due that day or earlier must be done. The server works it out from the account's task documents, so
every device shows the same streak and visits do not count. A day with nothing
due counts.

Today counts while nothing is overdue, and a task that becomes
overdue can still be finished before midnight to keep the day. Web and iOS record completion times.
A completion after midnight loses the deadline day, including offline completions that sync later. A completion an
import reports, such as a Canvas submission, carries no time and counts as on
time.

A finished day permits late sync for two more days.
The server then records it permanently. Later imports cannot change it by omitting old tasks. Days before the
account's first streak check never count against it.

Guests have no streak.

The iOS Account profile shows the current and best streak, the current overdue
task count, and four weeks of days. Web Progress shows the current streak and
this week's days, with the best streak and the overdue count in its tooltips.
Web Settings shows the current and longest streak.
iOS shows the current streak beside Home's Account button in one glass capsule.
The capsule opens Account, which contains the full streak page. Every streak flame is the web's
drawing: a gradient from a red base to an amber tip with a lighter core. On iOS
the count uses a numeric transition and the flame flares briefly as it rises,
respecting Reduce Motion.

Each account uses its school time zone, selected on the first check-in, or UTC
if no school is known. All devices use that same day boundary, even when traveling.
Deleting the account deletes its streak history.

## 13. Privacy and retention

Honorum does not require a university password. Connecting Canvas requires a
personal access token. The app has no third-party analytics, fingerprinting,
or activity tracking.

### 13.1 What is stored where

| Data | Location |
| --- | --- |
| Guest schedules and tasks, and the record behind the This Term ring (every owner) | The browser or device only |
| Guest seat alerts | Server: watched sections and notification destinations |
| Guest reminders | Server: meeting patterns, campus time zones, and upcoming task due times |
| Account data | Server: phone number, device sessions, schedules, tasks, preferences, watches, and reminder data. Account documents are stored as JSON |
| Account Canvas credentials and account calendar feed links | Server, encrypted, outside ordinary account documents |
| Google Calendar connection | Server: the Google account's email address and an encrypted refresh token, and, while events are shown, copies of their titles, times, locations, and links, which leave with the calendar that holds them |
| Guest Canvas tokens and calendar feed URLs | The device only; they pass through the relay when used |
| Share links | Server: contents encrypted by the client |
| Streaks | Server: a fixed time zone and whether each finished day ended with anything overdue. Visit dates recorded by earlier releases remain until account deletion; nothing reads them |
| Operator metrics | Server: daily counts such as accounts, active accounts, requests, and imports. They hold no account, device, phone number, or address |

### 13.2 Retention

| Record | Retention |
| --- | --- |
| SMS consent (number, confirmation time, notice version) | 1 year, independently of account deletion |
| Sessions | Until 1 year without use |
| SMS codes | Expiry controlled by Twilio Verify |
| Replaced document versions | 30 days, for restoration |
| Share blobs | 180 days after upload; never replaceable |
| Completed delivery records | 30 days |
| Operator metrics | 2 years |
| Backups | Their configured retention policy |

The public Privacy Policy and Terms at `/legal.html` describe this retention
and are available offline.

### 13.3 Deletion and sign-out

- Account deletion first deletes the Honorum calendar in Google and revokes Google access.
  It then removes server data and local sync revisions. Local plans remain available and can be
  uploaded if the account is created again.
- Ordinary sign-out shows an empty guest planner. Pending edits remain in the
  account's local cache and return after signing into that account.
- Removal of browser site data deletes local caches.
  Open tabs stop saves and ask for a reload. This prevents restoration from old memory. This does not delete server-side account data.

Use the [security policy](../SECURITY.md) to report a security defect.
For institution data requests, use the installation's configured `contact` address.

## 14. Web interface

The desktop app shows a weekly calendar with a side pane for tasks or item details.
The header holds the week controls at the start and its actions at the end: Add Event, Edit Classes, Tasks, and the main menu.
The menu holds share actions, Settings, and Keyboard Shortcuts.
Add Event uses the selected day as its default.

The calendar and panes have separate scroll areas.
A side pane keeps its head in place (Progress and the task search, or the
class search) and scrolls only the list beneath it. The list holds to the
pane's width and its thin scrollbar draws in the right inset, so nothing
reflows as a scrollbar comes and goes and no room is kept for one that never
shows.
Controls and cards stay within the available width.

Item details in the side pane use as few words as possible and no subtitles.
A back arrow returns to Tasks; then come the item's color dot and code, and one
line for when and where. A class's seats, with a bell for the seat alert, its
color, and a Details fold sit in one box. Details holds the title,
instructors, waitlist, exams, credits, dates, CRN, terms, and description.
Skip (the clicked day), Edit, and Remove end the pane. An exam offers Show
Class and Hide; a Google event opens in Google Calendar.

The app uses warm neutral surfaces, a brown accent, Poppins display text,
and the system font for body text.
Shared controls are in `apps/web/src/app.css`.
`Dialog`, `Menu`, and `Icon` are shared components.
Brand sources are in `assets/brand`. Web copies are in `apps/web/public`.
The inline wordmark paths are in `apps/web/src/wordmark.js`.

### Navigation and motion

The desktop class finder opens on the left. Tasks and details stay on the right.
Done returns to the planner. A selected section appears as a calendar preview.

Overlapping classes keep their colors, gain red outlines, and show a conflict label.
A banner offers Rearrange Sections. An empty edit view shows Nothing Scheduled.

On phones, the bottom bar switches between Week, Classes, and Tasks.
Week shows one day at a time. The chosen weekday stays selected across week changes.
Schedules appear as chips on Classes. Item details open in a sheet.
Appearance follows the system.

| Shortcut | Action |
| --- | --- |
| `/` | Enter or leave Edit Classes |
| F9 | Show or hide tasks |
| F10 | Open the main menu |
| Ctrl+, | Open Settings |
| Alt+PgUp and Alt+PgDn | Select the previous or next week |
| Alt+T | Return to this week |
| Ctrl+P | Print |
| Escape | Close the active pane, menu, or dialog |

The primary menu omits shortcut hints. Keyboard Shortcuts lists them.
Routes use fragments: `#classes`, `#classes/<section>`, `#tasks`,
`#settings`, `#settings/<page>`, and `#connect`.
Browser Back returns to the previous view.
Desktop Settings page changes replace the current history entry.
On phones, Back returns from a Settings page to its page list.

Surfaces open in about one third of a second and close slightly faster.
Their contents move with them. A reversed action starts from the current position.
Menus expand from their buttons in about one sixth of a second.

Carets and chevrons reflect the open state.
Descriptions expand in place where the browser supports height animation.
Reduce Motion shows the final state immediately.

### Tasks

Due dates appear in a Due row above the week.
Tasks at Their Due Time moves them onto the hours and hides the Due row.
Each task then occupies a half-hour block that ends at its deadline.
The block uses the class color and shares columns with overlapping items.
This device setting does not change the task or its deadline.
Both forms open the task editor.

The task pane starts with [Progress](#11-progress-rings).
With no tasks and no connected source, it shows No Tasks in its middle with Connect Canvas (or Import Tasks) and Add Task.
The list shows overdue tasks and the rest of this week, and nothing past the week's end.
This week uses the calendar's Monday or Sunday start.
When nothing is due this week, the list says so.

Coming Up holds Next Week, Later, and No Due Date as one card each, always whole.
Each card shows its count. Next Week also shows daily load marks in class colors.

Later and No Due Date show a bar split by class and a count per class.
A fold expands in place and closes when the list leaves the screen.
Search shows all matching tasks.

Tasks use daily headings through next week, then weekly headings.
Overdue comes first in red and holds only open tasks. No Due Date comes last.

Show Completed keeps completed tasks under their due day.
Those due before today sit above the list, under daily headings back to the start of last week and weekly headings before that.
Turning it on or off keeps today where it is on screen, so earlier days are a scroll back.
A list that opens with them shown opens at today.
On a phone the toolbar stays at the top of the page as it scrolls.

Rows show the class-colored tick, title, and time, except at 11:59 pm.

Weekly groups add the weekday. Overdue rows add the date.
The accessible name and tooltip include the class.
A source button appears on hover or keyboard focus and stays available in the editor.

Rows reserve space for the button so long titles cannot cover it.

Filter and Sort contains Show Completed, class filters, and Sort By.
Sort By orders tasks within each heading.
Sync assignments refreshes connected sources.

### Settings and schedules

Settings has four pages: General, Account, Schedule, and Assignments.
Desktop shows the page list in a sidebar. Phones show it before a selected page.
The dialog keeps one size; long pages scroll.
It focuses the selected page and remembers it for the session.
Each page controls only its own settings.

| Page | Controls |
| --- | --- |
| General | Notifications, Keyboard Shortcuts, privacy policy in a new tab |
| Account | Phone number, sync state, streak, conflict recovery, Devices, Sign Out, Delete Account; Sign In for guests |
| Schedule | Tasks at Their Due Time, Google Calendar, Manage Classes, Earlier Versions for accounts |
| Assignments | Canvas, calendar feeds, new sources, Canvas Courses; a connect action when empty |

Manage Classes can remove saved classes offline.
Add Classes closes Settings and opens class search.
Earlier Versions restores an earlier copy of the open schedule through ordinary sync.
Devices loads its own menu with retry controls and a current-device label.
Removal of another device needs confirmation.

The schedule menu lists the term's schedules.
It also offers New Schedule, Duplicate, Rename, Make Primary, Rearrange Sections,
School, Term, and Delete Schedule.

### First visit and setup

A new browser receives the landing page at `/`.
Open Honorum uses `/app` to set the `hn_app` cookie and enter the app.
A browser with that cookie or a session receives the app directly.
The landing page forwards fragment targets for share links and seat alerts.

Query links, including Google returns, reach the app directly.
Removal of site data restores the landing page. `/about` also serves it.

The first app visit starts setup.
The welcome page lists schools only when there is a choice.
Select a school before Continue when several schools are available.
At a Canvas school, the next page asks for a token and finds classes with assignments.

Get a Token opens Canvas settings. The page gives four steps to create the token.
It asks for the Canvas address when the school configuration has none.
The student confirms the [matched classes](#5-classes-from-canvas) or skips them.

Other schools first offer an assignment import.
Their final Search Classes action ends setup in class search.
Phone sign-in appears after classes or assignments when the server supports it.
It is also available on the welcome page.

Sign-in explains automatic sync, offline edits, and optional accounts.
Every step after school selection can be skipped.
Saved plans skip setup. The account's `onboarded` preference syncs across devices.

Each step keeps the title, content area, and footer in the same frame.
Back and step progress stay at the top. The content scrolls independently.
The footer keeps a primary action and space for Skip, Not Now, or Sign In.

Phones stack these actions. Desktop shows one fixed-size card.
Canvas requests show placeholder rows and progress on the primary button.
The footer shows a border only while content can scroll beneath it.

## 15. iOS interface

The native app starts on Home. Other screens use one navigation stack above it.
Sheets use inline titles; pushed screens use large titles.
Poppins display text, warm surfaces, white cards, class colors, chips,
and capsule buttons form the shared visual kit.
Appearance follows iOS. Controls support Dynamic Type and VoiceOver.

### Home

Home shows tasks, the schedule, and term progress in that order.
The navigation bar stays visible during scroll.
It holds progress rings beside the campus date and the Account button.
A signed-in account also shows its streak in the same glass capsule.
Each ring heads the section it counts and shows completed and total counts.

| Section | Behavior |
| --- | --- |
| Today | Overdue first, then shared deadlines; completed tasks fold into one row |
| This week | Remaining tasks after today in the Sunday-to-Saturday week; All Tasks opens the full list |
| Schedule | Week strip with class dots; the selected day's classes, exams, and events appear on a rail |
| Term | Date bar, week number, school breaks, exams, next three milestones, and a tile per class |

Today rows contain a class-colored tick and title.
The tick completes a task. The title opens its editor.
The current schedule rail marks now, folds past classes, and shows time until the next class.
Calendar or a class opens the planner on that day.
Each class tile shows its term progress and next open task.

Home has Search and New at the bottom.
On iOS 26, system toolbar search expands above the keyboard.
The date and Account stay in place.
Home stays visible until the query contains text.

Results show open matches before completed matches, by title, class code, or class name.
A result opens its editor above search. Closure of search returns to Home.
Before iOS 26, search opens the same results in a sheet.

New opens one task and event sheet.
The Task and Event switch preserves a typed title.
Tasks uses New Task; the planner uses New Event.
Class setup stays in Account. An empty term links to it from Home.

### Editors and account

Editors show the title, class chips with None first, and item fields.
A new task has no due date.
Deadline chips offer No date, Today, Tomorrow, and the next four weekdays.
They preserve the chosen time.

Date and time pickers use the campus time zone.
Events offer Once or Every week, start and end times, and class or custom colors.
Existing tasks offer Completed, Delete, and a source action when available.
Existing events offer Delete.

Add creates an item; Done saves its edits.

Account starts with a profile card.
Guests see rings, Sign In, and the account features.
Accounts see their phone number, sync state, and full streak page.

| Group | Contents |
| --- | --- |
| Classes | My Classes, Find Classes, Schedules, Earlier Versions for accounts, School, Term |
| Tasks | Assignment Sources, Canvas Courses, Tasks at Their Due Time |
| Notifications | Reminders and Seat Alerts |
| Sync, for accounts | Devices, conflict recovery, Sync Now |
| About | Privacy & Terms and version |
| Final actions | Clear Completed Tasks, Reset Plan, Sign Out, Delete Account |

Sign Out asks for confirmation without destructive styling.
A disabled notification permission explains why delivery cannot work and links to system Settings.
Failed account actions and calendar exports show one alert.
Confirmations explain destructive actions.
The app has no keyboard shortcuts.

My Classes lists colored class cards, their next meetings, and remove controls.
It also shows Find Classes, credits, school, and term.
Find Classes searches the catalog and marks saved classes.
A course shows section cards with meetings, location, instructor, seats, and an add control.

Full sections offer Watch Seat.
Seat Alerts turns a section's tile green when a seat opens.
Schedules shows class colors and marks the primary schedule.

### Planner and notifications

The phone planner shows one day. The iPad planner shows a week.
A phone heading shows the weekday, date, and any full or partial closure.
The month remains the navigation title for VoiceOver and back controls.

The week strip marks the selected day and today's date, with class dots beneath.
Today and weekday buttons provide alternatives to gestures.
VoiceOver can move the heading one day at a time.

The timeline uses warm surfaces and class-colored cards with 14-point corners.
Class cards show the course title, code, location, and end time.
Narrow week columns omit the code. Short cards show only the title.
The phone omits hour rules.
The toolbar holds Today and a menu with Schedules and Share Schedule.

One vertical timeline contains a native horizontal pager with day alignment.
Pages keep stable identities, load near the viewport, and clip their content.
The system handles velocity, cancellation, and deceleration.
The heading and week strip change together when a page crosses its midpoint.

Day changes preserve the vertical position and do not rebuild occurrence layouts during a drag.
Today and weekday jumps go directly to the destination with a short fade.
Reduce Motion shortens the fade and disables programmatic travel.
Direct swipes still follow the finger.

On return from the background, a partial page settles onto the most visible day.
A drag on an item cancels its tap action.

A vertical flick moves at least two hours and settles on an hour.
A stronger flick keeps its momentum and uses the nearest natural stop.
A second flick continues from the previous target.
A finger that stops before release leaves the timeline at that position.

The initial hour is 7 AM. An hour uses 80 points at default text size.
Hour height, the gutter, and day circles scale with Dynamic Type.
The hour label beside the now marker hides beneath its pill.

A class or event opens a sheet with its colored block and details.
Actions include Skip This Day, Remove from Schedule, or Edit and Delete for events.
Accessibility groups the selected day's cards.
On iPad, the week strip aligns with the day columns and chevrons move one week.

The phone leaves deadlines to Home and Tasks by default.
The iPad shows a scrollable Due row of task capsules above the week.
Tasks at Their Due Time replaces that row with half-hour task blocks.
Midnight deadlines occupy 00:00–00:30.

Task blocks do not create class conflicts.
Both forms open the task editor. The setting stays on the device.

A signed-in phone registers for APNs when the server supports it.
The server then delivers account reminders and seat alerts.
The app stops local reminders for that account and still checks its device watchlist.
Class reminders open their day. Task reminders open their editor.
Seat alerts open Account.

### Tasks and setup

Tasks uses Home's cards with All, class, and No class filters.
Due-date order groups open tasks as Overdue, Today, Tomorrow, This Week, Later,
and No Due Date.
Completed tasks fold into one row, newest completion first; undated completions come last.
Other sorts use one card. Sort and Reverse Order stay on the device.
Rows show only dates or times absent from their group heading.

Completion records `completedAt` as ISO 8601.
Reopen clears it. Another completion records a new time.
Imports without completion metadata stay undated.

An account import that changes completion clears its old timestamp.
The tick fills immediately. The task moves after 450 ms, or immediately with Reduce Motion.
Deletion asks for confirmation in the editor and long-press menu.

Schedule deletion also asks for confirmation and has no full-swipe action.
VoiceOver reads each tick's action and task title.

Open in Canvas tries Canvas Student with the `canvas-courses` scheme.
If the app cannot open, iOS opens the original website.
The destination keeps its domain, encoded path, query, and fragment.
Feed sources open their website.

Tasks without a valid source URL omit this action.
Class colors use saved overrides, then the schedule's ordered unique courses.
Unmatched tasks use the neutral event color.
Exam sittings use the class color.

Hide This Exam removes one sitting.

First launch follows welcome, school selection, optional account, and class setup.
School choice and sign-in appear only when applicable.
Canvas class review separates unmatched courses and explains uncertain matches.
An unresolved section choice must be selected before the class counts.

Other schools offer assignment import and then class search above My Classes.
The welcome symbol sits inside three progress rings.
Later steps use pastel tiles and bottom actions with progress dots.
Steps slide from their direction of travel; Reduce Motion uses a crossfade.

Configuration failure offers Try Again. Existing plans skip setup.

Phone entry accepts international AutoFill and formats North American numbers during entry.
A separate field supports one-time-code AutoFill.
A refused code request uses the server's resend delay.
Assignment Sources shows each source's error separately.

The keyboard moves from address to token to Connect. Pasted tokens lose outer whitespace.
Empty class search distinguishes filters from absent matches and offers Clear Filters.

Launch reads the saved configuration, session, plan, and last streak before it shows Home.
Source details and native test commands are in the [iOS guide](../apps/ios/README.md).
