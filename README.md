# Find Profile

Find Profile is a macOS file search app built with Tauri, React, and Rust. It indexes local folders and NAS paths so files can still be searched after a NAS is disconnected.

## Features

- Add local or NAS directories as searchable roots.
- Scan and rescan indexed roots with progress tracking and cancellation.
- Search indexed files and folders with source, type, extension, and sort filters.
- Keep NAS index data available when the NAS path is temporarily disconnected.
- Configure indexing behavior for hidden files, files, folders, and watcher startup.

## Development

Requirements:

- Node.js
- Rust
- Tauri system dependencies for macOS

Install dependencies:

```sh
npm install
```

Run the app in development:

```sh
npm run tauri dev
```

Build a release bundle:

```sh
npm run tauri build
```

## macOS Install

Download the DMG from the GitHub Release, open it, and drag Find Profile into Applications.

The current build is unsigned. On first launch, macOS may block the app. If that happens, open System Settings > Privacy & Security and choose Open Anyway, or Control-click the app and choose Open.
