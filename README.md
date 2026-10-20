<!-- SVENKUNKKA-README-NAV:BEGIN -->
[中文项目说明](#project-overview-zh) · [English overview](#project-overview-en) · [维护者 X / Maintainer on X](https://x.com/Svenu1s5)
<!-- SVENKUNKKA-README-NAV:END -->

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


---

<!-- SVENKUNKKA-PROJECT-INFO:BEGIN -->

<a id="project-overview-zh"></a>
## 中文项目说明

基于 Tauri、React 和 Rust 的 macOS 文件搜索应用，为本地目录与 NAS 路径建立索引，提供来源、类型、扩展名和排序过滤，并支持扫描进度与取消。

**使用入口：** 准备 Node.js、Rust 和 macOS 的 Tauri 依赖，运行 `npm install` 与 `npm run tauri dev`；用 `npm run tauri build` 构建安装包。

**范围说明：** NAS 断开后仍可搜索已有索引，不代表离线时能打开远端文件。README 标注当前构建未签名，首次启动需按 macOS 提示处理。

<a id="project-overview-en"></a>
## English project overview

A macOS file-search app built with Tauri, React and Rust, indexing local folders and NAS paths with source, type, extension and sort filters, plus scan progress and cancellation.

**Getting started:** Install Node.js, Rust and Tauri system dependencies for macOS, then run `npm install` and `npm run tauri dev`. Build a release bundle with `npm run tauri build`.

**Scope:** A disconnected NAS can still be searched through its existing index; this does not make remote files available offline. The README identifies the current build as unsigned, so first launch may require macOS approval.

**仓库维护者 / Repository maintainer:** [SvenKunkka](https://github.com/SvenKunkka) · [X / Twitter](https://x.com/Svenu1s5)

<!-- SVENKUNKKA-PROJECT-INFO:END -->
