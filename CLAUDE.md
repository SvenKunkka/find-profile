# Find Profile Claude Code Rules

## Project Identity

Project name: Find Profile
App name: Find Profile
Platform: macOS first
Tech stack: Tauri + Rust + React + TypeScript + SQLite FTS5

Find Profile is a macOS file search app similar to Windows Everything, but with first-class support for indexing mounted NAS volumes under /Volumes.

## Core Product Principles

1. Search must query a local SQLite index, not scan the filesystem in real time.
2. MVP indexes file metadata only, not file contents.
3. NAS roots must never lose old indexed data just because the NAS is disconnected.
4. Local roots may use filesystem watching.
5. NAS roots use manual scan and scheduled scan.
6. Reliability is more important than advanced features.
7. UI should be dense, simple, fast, and utility-oriented.

## Development Rules

1. Work one stage at a time.
2. Do not implement future stages early.
3. Do not rewrite large parts of the app unless necessary.
4. Do not remove existing working tests.
5. Do not modify Rust backend during UI-only stages unless explicitly required.
6. Do not modify frontend UI during backend-only stages unless explicitly required.
7. Backend-only stages must not run npm run tauri dev unless explicitly asked.
8. Backend-only validation command:
   cd src-tauri && cargo test && cd ..
9. Frontend validation command:
   npm run build
10. Tauri runtime validation command:
   npm run tauri dev
11. Always explain changed files briefly after finishing.
12. Always run the requested validation command before saying the task is complete.

## Git Rules

Do not commit automatically unless explicitly asked.
After each successful stage, wait for the user to run or request git commit.

## Data Model

Use SQLite with FTS5.

Tables:
- roots
- files
- files_fts
- scan_jobs
- settings

Root kinds:
- local
- nas

Root statuses:
- available
- disconnected
- scanning
- error

Path detection:
- path starts with /Volumes/ => nas
- otherwise => local
- user-specified kind overrides automatic detection

## Backend Commands Needed Eventually

- add_index_root
- list_index_roots
- start_scan
- get_scan_status
- search_files
- open_file
- reveal_in_finder
- remove_index_root
- rescan_root

## Tauri Dialog Rule

Directory picker must use the real Tauri dialog API.
The add root button must be a real button element with onClick bound.
Never fake the directory picker with a text input in the final implementation.

## NAS Rules

1. NAS disconnected must mark root status as disconnected.
2. NAS disconnected must not delete old files from the index.
3. Opening an unavailable NAS file should show a friendly error.
4. Rescanning after reconnect should restore available status.

## UI Rules

1. The app must never white-screen because a Tauri command fails.
2. All invoke calls must use try/catch.
3. Initial render must not depend on backend calls succeeding.
4. The MVP page must always show:
   - Find Profile title
   - search box
   - Indexed Roots area
   - Results area
   - status bar
5. Buttons must have visible text during implementation, not icon-only, until behavior is verified.

## Testing Rules

Before marking a stage complete:
1. Run cargo test for Rust backend changes.
2. Run npm run build for frontend changes.
3. Run npm run tauri dev only when UI or Tauri command integration needs manual verification.
