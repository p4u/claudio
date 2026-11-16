# New-session wizard: folder navigation and finding a conversation: design

Status: draft   Class: architectural (daemon listing, TUI wizard, web wizard; no wire change)
Owner decisions so far (2026-11-16):
- The web wizard's folder field must let a person navigate to any folder, including a new one
  with no claude history.
- "Browse folders…" must show every folder.
- A conversation search goes into the Alt+n wizard, in the TUI and the web ("Web + TUI").

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
  - Files and folders share the reply's byte budget (`MAX_FRAME − 256`), so files push folders
    out.
  - `claude_at` is never filled, so the "claude <age>" badge never shows from a listing.
- **TUI** (`crates/tui/src/wizard.rs`) already browses typed paths: `~/` and `/` list the
  parent's subfolders, with →/Tab to enter, ← to go up, "start here" and Alt+. for hidden
  folders. But a listing error is dropped silently (`interaction.rs:936`), so a folder it cannot
  read just looks empty.
- **Search** already finds past conversations, with their host, `cwd` and conversation id
  (`Msg::Search`, `SearchHit`; web `search` op over every host; TUI Alt+z s on the active host).
  It is not reachable from inside either wizard. The TUI's search refuses to run while a modal is
  open (`builtins.rs:73-78`).

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
3. **No folder goes missing.** The daemon lists folders first and sorts before it cuts. Past the
   cap, the reply says it was cut, and both UIs say "type to narrow".
4. **Errors are shown.** A folder that cannot be listed shows its error ("cannot list ~/x:
   permission denied") in both wizards instead of an empty list.
5. **Find a conversation, in both wizards.**
   - Typing at least two characters searches the conversations on every connected host.
   - Picking a hit focuses its live session if one is open; otherwise it starts a session on
     that host, in that folder, resuming that conversation.

## Non-goals

- No change to the wire protocol, the daemon's ops or the web protocol's ops. Every new behaviour
  uses `ListDir`/`fs.list`, `RecentProjects`/`fs.projects`, `Search`/`search` and the existing
  spawn.
- No fuzzy search over the whole filesystem, and no indexing.
- No change to the TUI's browse keys.
- No change to search's limits (`claude/search.rs`) or its matching.

## Design

### Daemon: `host::list_dir`

1. Read up to `MAX_SCAN = 50_000` entries. Past that, set `truncated`.
2. Sort the result: folders first (`dir == true`, symlinks to folders included), then files.
   Each group is ordered by name case-insensitively, then byte order as a tiebreak.
3. Keep the first `MAX_DIR_ENTRIES` (5000). Past that, set `truncated`.
4. The server's byte budget (`server.rs`) then cuts from the end, which is files first.

`claude_at` stays `None`; the dead "enriched below" comment goes. The reply's shape is unchanged.

### TUI

- `(ReplyTo::DirEntries, Err(e))` puts the error text, sanitised and one line, in the directory
  step's status line. The list is emptied. The next successful listing clears the error.
- When the browse list is truncated, the step shows "list cut: type to narrow".
- **Finding a conversation.**
  - On the first screen, any typed text of at least two characters adds a row "Search
    conversations for “<text>”". It sits after the host rows, like the existing "connect to
    <query>" row.
  - Enter on that row sends `Msg::Search{query, limit: 0}` to every connected host whose
    `Welcome.features` has search, at most one in flight per host.
  - The wizard then shows a **conversation step**: hits merged newest first, each as `<age>
    <host>  <~/cwd>  <title>`, with the snippet under the list.
  - "search busy" from a host is retried once. Other failures go in one line naming the hosts.
  - In the conversation step, ↑/↓ move, typing refines the query (debounced 300 ms), Enter picks
    and Esc goes back.
  - Picking a hit calls the existing `resume_conversation(host, cwd, conversation)` path
    (`builtins.rs`): it focuses a live tab, or spawns with `resume`.
  - Plain mode (`--plain`) has no search, as today.

### Web

- **The folder field is a combobox** (`role="combobox"`, a listbox under it):
  - Empty, or text not starting with `/` or `~`: the recent folders (`fs.projects`, up to 50,
    scrollable), filtered by the text (case-insensitive substring), as today but not capped at 8.
  - Text starting with `/` or `~`:
    - The rows are the parent's subfolders (`fs.list` of the text up to its last `/`, only when
      that parent changes), filtered by the last segment's prefix, case-insensitively.
    - A "▸ use <folder>" row comes first: the typed folder when it exists, else the parent.
  - **Keys:**
    - ↓/↑ move the highlight.
    - Tab or → (with the caret at the end) completes the highlighted folder into the field with a
      trailing `/`, which lists it.
    - Backspace right after a `/` removes the last segment.
    - Enter picks the highlighted row, or the typed text when nothing is highlighted.
    - Esc closes the list, then the dialog.
  - **Mouse:** a click on a folder row completes it, as Tab does; a click on "use …" picks.
  - `~` is accepted as typed and sent as the spawn `cwd`, because the daemon expands it
    (`daemon/mod.rs:543`). The warning "Type an absolute path" now reads "Type a path (it starts
    with / or ~)".
  - Hidden folders are hidden unless the "hidden" toggle is on or the fragment starts with `.`.
  - A listing error, or a cut list, shows one line under the field.
- **"Browse folders…"** sets the field to `~/` and opens the list. The separate panel is removed.
- **Finding a conversation.**
  - A "Find a conversation…" field at the top of the wizard runs the existing `search` op,
    debounced 280 ms, at least 2 characters, as `ui/search.js` does.
  - It shows hits under the field: title, `host · ~/cwd · age`, and the snippet.
  - Picking one sets host, folder, kind `claude` and the conversation to resume, then focuses
    Start.
  - If a live session already has that conversation, picking focuses that session and closes the
    wizard, as `openConversation` in `ui/search.js` does.
  - The hit list and the picking logic live in one pure module shared with `ui/search.js`, with
    node tests.
- testids:
  - kept: `wizard-dir-input`, `wizard-browse`;
  - new: `wizard-dir-list`, `wizard-dir-row` (`data-path`), `wizard-dir-use`, `wizard-dir-error`,
    `wizard-find`, `wizard-find-row` (`data-conversation`).
- DESIGN.md governs every new piece: tokens only.

### Alternatives considered

| Option | Why not |
|---|---|
| Keep the separate browse panel and add type-ahead to it | Two ways to do one thing was the confusion reported; the TUI's typed browsing already works. |
| A new daemon op that lists folders only | Changes the wire; sorting folders first and cutting files first gets the same result within today's reply. |
| A button in the wizard that opens the existing search dialog | The owner asked for the search *in* the wizard; inline hits keep the person in one dialog. |

## Compatibility and rollout

- **No protocol change.** An older daemon still lists in directory order: the UIs then show
  what it sent, as today.
- **Old pages:** the web page and server ship together.
- **Rollback:** an older binary.

## Failure modes and security

- **Paths are untrusted:**
  - The web checks the length (`PATH_MAX` 4096, `ws.rs:412`) as today.
  - Listing goes through the daemon as today, so the person can only list what their user can.
  - Folder names are rendered as text.
- **A huge folder:** at most `MAX_SCAN` entries are read, and the reply stays within its budget.
- **Search:**
  - One search runs per host (the daemon's slot), and "search busy" is retried once.
  - A host that fails is named, never blocking the others.
  - Snippets are rendered as text.

## Acceptance criteria

- [ ] AC1: daemon unit test: a folder with 6,000 files and 10 subfolders lists the 10
  subfolders first, sorted case-insensitively, with `truncated` true. A folder of mixed-case
  names sorts case-insensitively.
- [ ] AC2: TUI unit test: a `DirEntries` error shows in the step and clears on the next listing.
  A manager test: a folder with mode 000 shows "cannot list".
- [ ] AC3: TUI tests:
  - typed text adds the "Search conversations" row;
  - Enter sends `Search` to every connected host with the feature;
  - the hits list merges newest first;
  - Enter on a hit resumes, focusing a live tab when one exists.
  - A manager test runs it end to end with a fake transcript under the harness's
    `~/.claude/projects`.
- [ ] AC4: node tests for the pure helpers: the parent/fragment split of a typed path, the
  filtering and ordering of rows, and the merging of hits and the pick action.
- [ ] AC5: web e2e (`wizard-nav.spec.mjs`):
  - typing `~/` lists the home's subfolders;
  - typing a prefix filters them;
  - Tab enters a folder, Backspace after `/` goes up;
  - Enter starts a session in the folder;
  - "Browse folders…" opens the list at `~/`;
  - an unreadable folder shows its error;
  - "Find a conversation" finds a fake transcript and starts the session with `resume`.
  - Zero CSP violations.
- [ ] AC6: `make check`, `make matrix`, the manager suite, the web suites, the node tests and
  `make test-web` pass.

## Constraints

- ACCEPTANCE.md:
  - A5: shared logic in pure modules;
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
- Commits: `type(scope): lowercase summary`, with no AI attribution and no `Co-Authored-By`
  trailers.

## Open questions

None.
