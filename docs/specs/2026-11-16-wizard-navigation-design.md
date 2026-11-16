# New-session wizard: folder navigation and finding a conversation: design

Status: audited (2026-11-16)   Class: architectural (daemon listing, TUI wizard, web wizard; no wire change)
Owner decisions so far (2026-11-16):
- The web wizard's folder field must let a person navigate to any folder, including a new one
  with no claude history.
- "Browse folders…" must show every folder.
- A conversation search goes into the Alt+n wizard, in the TUI and the web ("Web + TUI").
- TUI: the search row in the wizard opens the existing search-hits Picker (audit F15, "Picker").

## Problem

The owner's report: "in the local filesystem, it is quite hard to use if the folder is new and
not a previous session … it asks 'path to project', but I cannot easily navigate among my
folders. And there is a 'browse folders' which opens another explorer, but I don't see all of
the existing folders." They also want the session search in the wizard, "so we can find an
existing session on which we might not know the exact folder."

What the code does today:

- **Web folder field** (`addons/web/web/app/ui/wizard.js:87-139`):
  - It takes an absolute path only (:101) and offers 8 recent folders (:96, `fs.projects` with
    limit 50). Those folders come from claude history (`claude/projects.rs:830`), so a new folder
    is never offered.
  - Typing a path gives a single "Use <path>" row, with no listing of what is inside.
- **Web "Browse folders…"** (:124-139):
  - It opens a separate panel that starts at the *parent of the newest project* (:110), not at
    home.
  - It has no type-ahead and no keyboard navigation.
  - A click both selects a folder and enters it (:135).
  - "The list is cut. Type the path to go further." appears when the reply was truncated.
- **Daemon listing** (`daemon/host.rs:223-250`, `server.rs:718-748`):
  - `read_dir` keeps the first `MAX_DIR_ENTRIES` (5000) entries *in directory order*, before
    sorting. In a big folder an arbitrary subset is shown.
  - The reply's byte budget (`MAX_FRAME − 256`) cuts from the end, after folders-first
    sorting, but the 5000 cut above runs before any sorting, so files push folders out there.
  - The budget's size estimate (`name.len() + 24`) undercounts the real JSON (about `name + 82`
    bytes an entry), so a folder of long names can produce a frame over `MAX_FRAME`; the write
    fails and the client loses its daemon connection.
  - `claude_at` is never filled, so the "claude <age>" badge never shows from a listing.
- **TUI** (`crates/tui/src/wizard.rs`) already browses typed paths: `~/` and `/` list the
  parent's subfolders, with →/Tab to enter, ← to go up, "start here" and Alt+. for hidden
  folders. But a listing error is dropped silently (`interaction.rs:936`), so a folder it cannot
  read just looks empty.
- **Search** already finds past conversations, with their host, `cwd` and conversation id
  (`Msg::Search`, `SearchHit`; web `search` op over every host; TUI Alt+z s on the active host).
  It is not reachable from inside either wizard. The TUI's search reply will not open its list over
  an open modal (`builtins.rs:74-78`).

## Goals

1. **Web navigation as you type.**
   - The web wizard's folder field navigates any host's filesystem as the person types.
   - A value starting with `/` or `~` lists the parent's subfolders, filtered by the last
     segment.
   - The person moves with the keyboard or the mouse, enters a folder, goes up, and picks the
     typed or highlighted folder, as the TUI does.
2. **The browser starts at home.** The web's "Browse folders…" opens that navigator at the
   host's home folder.
   - Folders only, sorted case-insensitively.
   - Hidden folders sit behind a toggle, and also show when the typed fragment starts with `.`.
3. **No folder goes missing below the cap.** The daemon lists folders first and sorts before it
   cuts, so a folder is only left out when its parent has more than 5000 subfolders or the reply's
   byte budget is reached. Then the reply says it was cut, both UIs say "list cut: type the
   folder's full name", and a fully typed name is accepted even though it is not listed.
4. **Errors are shown.** A folder that cannot be listed shows its error ("cannot list ~/x:
   permission denied") in both wizards instead of an empty list.
5. **Find a conversation, in both wizards.**
   - Typing at least two characters searches the conversations on every connected host.
   - Picking a hit focuses its live session if one is open; otherwise it starts a session on
     that host, in that folder, resuming that conversation.
   - In the TUI the hits show in the existing search Picker (owner decision, F15).

## Non-goals

- No change to the wire protocol, the daemon's ops or the web protocol's ops. Every new behaviour
  uses `ListDir`/`fs.list`, `RecentProjects`/`fs.projects`, `Search`/`search` and the existing
  spawn.
- No fuzzy search over the whole filesystem, and no indexing.
- No change to the TUI's browse keys.
- No change to search's limits (`claude/search.rs`) or its matching.

## Design

### Daemon: `host::list_dir`

1. Read up to `MAX_SCAN = 50_000` entries, keeping only the name and the kind from
   `DirEntry::file_type()` (no extra syscall on Linux and macOS); a symlink is resolved with one
   `metadata()` call. Past `MAX_SCAN`, stop and set `truncated`.
2. Sort: folders first (symlinks to folders included), then files; each group by name
   case-insensitively, then byte order.
3. Keep the first `MAX_DIR_ENTRIES` (5000); past that, set `truncated`. Only now fill `git`
   (kept folders only; files get `None`), `symlink` and `hidden`.
4. `host::list_dir` returns `(path, entries, truncated)`.
5. The reply is built by `proto::dir_entries_reply(path, entries, truncated)`, which measures the
   real JSON as `search_reply` does and drops entries from the end (files first) until it fits
   `MAX_FRAME − 256`, setting `truncated`. `dir_entry_encoded_size` goes, and the byte-budget cut
   moves out of `server.rs` into that function, so `server.rs` shrinks.

`claude_at` stays `None`; the dead "enriched below" comment goes. The reply's shape is unchanged.

### TUI

- `(ReplyTo::DirEntries, Err(e))` puts the error text, sanitised and one line, in the directory
  step's status line. The list is emptied. The next successful listing clears the error.
- When the browse list is truncated, the step shows "list cut: type the folder's full name", and
  Enter on a typed, unlisted name picks that name.
- **Finding a conversation** (owner decision F15: the existing Picker, no new wizard step).
  - On the first screen, typed text of at least two characters adds a row "Search conversations
    for “<text>”". Row order: host rows, then the search row, then "connect to <text>". When no
    host or folder row matches, the search row is the default; "connect to" is never the default
    while the search row shows.
  - Enter on the search row closes the wizard and emits `Effect::SearchAll { gen, query }`. The
    event loop (`lib.rs`) runs the core multi-host search (below) over its connections and sends
    one event back, `SearchAllDone { gen, hits, failed }`; each hit carries its host and that
    host's home (from `Welcome`) for the `~/` abbreviation. `App` drops a reply whose `gen` is not
    the latest. One `SearchAll` runs at a time.
  - The hits open in the existing search Picker (`builtins.rs`), each label `<age>  <host>  <~/cwd>
    <title>` with the snippet as detail; failures are one notice naming the hosts. With no
    connected host that has the feature: "no connected host can search (run `claudio daemon
    restart` there)".
  - Enter on a hit calls `resume_conversation(host, cwd, conversation)`: it focuses a live tab, or
    spawns with `resume`. To change the query the person reopens Alt+n.
  - Plain mode (`--plain`) has no search, as today.
- **The multi-host search lives once, in claudio-core** (`app/search.rs`): given the connected
  clients, it asks every one whose `Welcome.features` has `search`, one request per host, a 10 s
  deadline per host, one more try 500 ms after `search busy`, hits merged newest first with their
  host, failures as `(host, error)`. `addons/web/src/ws.rs` `search`/`search_host` call it (its
  per-host queue stays in the web); behaviour and `docs/web-protocol.md` §search unchanged.
- **Where the code lives (A8).** `wizard.rs`, its `on_key`/`refilter` functions,
  `interaction.rs::on_reply`, `lib.rs::run_effect` and `daemon/server.rs` are at their size
  baseline and must not grow. The directory step's error and cut state, and the search row, are
  added after first moving code out of `wizard.rs` (the host step to `wizard_host.rs`, as
  `ui/wizard_host.rs` already is) in a commit that only moves. `on_reply`'s `DirEntries` arms call
  one helper. The new effect is run by a function outside `run_effect`.

### Web

- **The folder field is a combobox** (`role="combobox"`, a listbox under it):
  - Empty, or text not starting with `/` or `~`: the recent folders (`fs.projects`, up to 50,
    scrollable), filtered by the text (case-insensitive substring), as today but not capped at 8.
  - Text starting with `/` or `~`:
    - The rows are the parent's subfolders (`fs.list` of the text up to its last `/`, only when
      that parent changes), filtered by the last segment's prefix, case-insensitively.
    - A "▸ use <folder>" row comes first. It always names the folder the field's text denotes:
      the text itself, minus a trailing `/` (for `~/src/` it is `~/src`; for `~/src/cla` it is
      `~/src/cla`). It never names the parent in place of the typed text. When the listing of the
      parent has arrived, is not cut, has fewer than 5000 entries, and has no folder of that name,
      the row reads "▸ use <folder> (not found)" and is disabled; the daemon's own "working
      directory … does not exist" stays the final check.
    - Highlight: with an empty last segment the "use" row is highlighted; with a fragment, the
      first matching folder row is highlighted, or the "use" row when none matches (the TUI's
      rule, `wizard.rs` `refilter`).
  - **Keys** (handled only while the list is open; otherwise the browser's defaults):
    - ↓/↑ move the highlight; ↓ on a closed list opens it.
    - Tab, or → with the caret at the end and no selection, completes the highlighted *folder* row
      into the field with a trailing `/`. With the "use" row highlighted, or the list closed, Tab
      moves focus as usual and → moves the caret. Shift+Tab always moves focus.
    - Backspace with the caret at the end, no selection, and the text ending in `/`: removes the
      last segment (`~/a/b/` → `~/a/`). At `~/` it becomes the home's parent as an absolute path
      with a trailing `/`; at `/` it does nothing. In every other case Backspace deletes one
      character.
    - Enter on a folder row completes it into the field (as Tab does) and starts nothing. Enter
      on the "use" row sets the folder, closes the list and moves focus to Start; it starts
      nothing. With the list closed, Enter in the field starts the session, as today. The
      combobox handles its own Enter and stops it from reaching the wizard's Enter-starts handler
      (`ui/wizard.js` `key`).
    - Esc closes the list and stops there (it does not reach the dialog); a second Esc closes the
      dialog.
  - **Focus:** the folder field keeps `data-autofocus`. The list opens on typing, on ↓ and on
    "Browse folders…", not on focus and not when the value is set by code (`openWizard({cwd})`, a
    picked conversation). It closes on blur, on "use", and on Esc.
  - **Mouse:** a click on a folder row completes it, as Tab does; a click on "use …" picks.
  - `~` is accepted in the field, but the page never sends it as a `cwd`. The page learns the
    host's home from `fs.list {path: "~"}` (the reply's `path` is the expanded folder), asked once
    per host when the wizard opens or the host changes, and resolves `~`/`~/…` to an absolute path
    before `claude.sessions` and `session.spawn`, as the TUI does (`wizard.rs` `expand`). Until the
    home is known, or if that listing fails, a `~` path cannot be started and the error line says
    why. `~user` is not expanded (the daemon does not either). The warning "Type an absolute path"
    now reads "Type a path (it starts with / or ~)".
  - Hidden folders are hidden unless the "hidden" toggle is on or the fragment starts with `.`.
  - A listing error, or a cut list, shows one line under the field.
- **"Browse folders…"** sets the field to `~/` and opens the list. The separate panel is removed.
- **Finding a conversation.**
  - A "Find a conversation…" field at the top of the wizard runs the existing `search` op, at
    least 2 characters (checked by the page before it asks; the daemon refuses shorter queries),
    debounced as `ui/search.js` does. It handles its own keys (↓/↑, Enter picks the highlighted
    hit, Esc clears it) and stops Enter from reaching the wizard's Enter-starts handler.
  - It shows hits under the field: title, `host · ~/cwd · age`, and the snippet.
  - Picking one sets host, folder, kind `claude` and the conversation to resume, then focuses
    Start.
  - If a live session already has that conversation, picking focuses that session and closes the
    wizard, as `openConversation` in `ui/search.js` does.
  - The hit list and the picking logic live in one pure module shared with `ui/search.js`, with
    node tests.
- testids:
  - kept: `wizard-dir-input`, `wizard-browse`; removed: `wizard-dirs`, `wizard-dir`, `wizard-browser`;
  - new: `wizard-dir-list`, `wizard-dir-row` (`data-path`), `wizard-dir-use`, `wizard-dir-error`,
    `wizard-find`, `wizard-find-row` (`data-conversation`).
- DESIGN.md governs every new piece: tokens only.

### Alternatives considered

| Option | Why not |
|---|---|
| Keep the separate browse panel and add type-ahead to it | Two ways to do one thing was the confusion reported; the TUI's typed browsing already works. |
| A new daemon op that lists folders only | Changes the wire; sorting folders first and cutting files first gets the same result within today's reply. |
| A button in the wizard that opens the existing search dialog | The owner asked for the search *in* the wizard; inline hits keep the person in one dialog. |
| TUI: a new conversation step inside the wizard (hits in place, live refine) | Owner chose the existing Picker (F15): no new wizard step, no debounce; the person reopens Alt+n to change the query. |

## Compatibility and rollout

- **No protocol change.** An older daemon lists an arbitrary 5000 entries without saying it cut
  them. Both UIs sort and filter what they get themselves, and a folder that is typed but not
  listed is still accepted (the "use" rule above); "(not found)" is shown only when the reply has
  fewer than 5000 entries and `truncated` is false.
- **Old pages:** the web page and server ship together.
- **Rollback:** an older binary.

## Failure modes and security

- **Paths are untrusted:**
  - The web checks the length (`PATH_MAX` 4096, `ws.rs:412`) as today.
  - Listing goes through the daemon as today, so the person can only list what their user can.
  - Folder names are rendered as text.
- **A huge folder:** at most `MAX_SCAN` entries are read, cheaply (no per-file git check), and
  the reply is cut by its real encoded size, so it always fits a frame.
- **Stale replies.**
  - Web listings: each `fs.list` carries a request sequence kept by the page (as `ui/search.js`
    does); only the reply to the latest request for the current host is applied. Requests are
    debounced 120 ms and sent only when the parent changes.
  - Web find: as `ui/search.js` (sequence guard, debounce).
  - TUI find: one `SearchAll` at a time; a reply whose `gen` is not the latest is dropped.
  - Changing the host in the web wizard clears the listing, the error and the home.
- **Search:**
  - One search runs per host (the daemon's slot), and "search busy" is retried once.
  - A host that fails is named, never blocking the others.
  - Snippets are rendered as text.

## Acceptance criteria

- [ ] AC1: daemon unit test: a folder with 6,000 files and 10 subfolders lists the 10
  subfolders first, sorted case-insensitively, with `truncated` true. A folder of mixed-case
  names sorts case-insensitively. A folder of 6,000 files lists without one `detect_git_branch`
  call per file (a unit test on the scan step: the enrichment runs over the kept folders only).
  5000 folders with 200-byte names give a reply whose frame is within `MAX_FRAME`, `truncated`
  true.
- [ ] AC2: TUI unit test: a `DirEntries` error shows in the step and clears on the next listing.
  A manager test: listing a path under a regular file (`<file>/x`, "Not a directory") shows "cannot
  list" (a mode-000 folder is listable when the suite runs as root).
- [ ] AC3: TUI tests:
  - typed text adds the "Search conversations" row, ordered and defaulted as above;
  - Enter on it closes the wizard and emits `SearchAll`; a `SearchAllDone` opens the Picker with
    the hits newest first and their hosts; an old `gen` is dropped;
  - Enter on a hit resumes, focusing a live tab when one exists.
  - A manager test runs it end to end with a fake transcript under the harness's claude projects
    root (`$CLAUDE_CONFIG_DIR/projects` when set, else `$HOME/.claude/projects`).
- [ ] AC4: node tests for the pure helpers: the parent/fragment split of a typed path, the
  filtering and ordering of rows, and the merging of hits and the pick action.
- [ ] AC5: web e2e (`wizard-nav.spec.mjs`):
  - typing `~/` lists the home's subfolders;
  - typing a prefix filters them;
  - Tab enters a folder, Backspace after `/` goes up;
  - Enter on "use" picks the folder and focuses Start; a second Enter (or a click on Start)
    starts the session there; typing `~/<missing>` and pressing Enter before or after the listing
    arrives never starts a session in `~`;
  - "Browse folders…" opens the list at `~/`;
  - an unreadable folder shows its error;
  - "Find a conversation" finds a fake transcript and starts the session with `resume`.
  - Zero CSP violations.
- [ ] AC7: hidden folders: node test and e2e — `.cfg` is absent from `~/`, present after the
  toggle, and present when the fragment is `.`; the TUI unit test that covers it is kept.
- [ ] AC8: cut lists: TUI unit test — a `DirEntries` with `truncated: true` shows "list cut: type
  the folder's full name" and Enter on a typed, unlisted name picks that name; node test for the
  same rule in the page.
- [ ] AC9: old daemon: node and TUI unit tests feed an unsorted listing (files before folders,
  mixed case); both show folders only, sorted case-insensitively by the client.
- [ ] AC10: stale replies: node test — a listing reply for an earlier request is ignored; TUI unit
  test — a `SearchAllDone` with an old `gen` is dropped.
- [ ] AC11: find, web e2e: picking a hit whose conversation is live focuses that session and closes
  the wizard; core unit test for the shared search helper: a `search busy` host is retried once, a
  failing host is listed in `failed` and the others' hits are returned.
- [ ] AC6: `make check`, `make matrix`, the manager suite, the web suites, the node tests and
  `make test-web` pass.

## Constraints

- ACCEPTANCE.md:
  - A5: the multi-host search (fan-out, retry, merge) lives once, in claudio-core, used by the web
    server and the TUI; each front-end keeps only its own navigation state;
  - A6: `app.rs` stays pure, and listing and search requests are effects;
  - A7: the console TUI stays plain;
  - A8: size budgets;
  - C2: inputs bounded;
  - F1: docs in the same branch.
- CLAUDE.md "Web":
  - DESIGN.md tokens;
  - no inline scripts;
  - pure browser modules import no bare specifiers;
  - testids are stable.
- Docs:
  - docs/manager-design.md §3.2 (the wizard, today dated);
  - docs/web.md: the wizard and the parity table;
  - README: the wizard paragraph;
  - docs/extensions.md: search is also reachable from the wizard.
  - `docs/web-protocol.md` DirEntry and `proto.rs` `DirEntry::claude_at`: say it is always
    null/`None` today; the page's "claude <age>" badge on listings goes. `dev/mock-server.mjs`
    `fs.list` answers per path (a small fixed tree, `~` expanded) so the navigator works on the
    mock.
- Commits: `type(scope): lowercase summary`, with no AI attribution and no `Co-Authored-By`
  trailers.

## Open questions

None.
