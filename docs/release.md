# Find Profile · 发布与打包 (Release)

## 1. App 身份信息

| 字段 | 值 | 配置位置 |
| --- | --- | --- |
| Product / Display name | `Find Profile` | `src-tauri/tauri.conf.json::productName`、`app.windows[0].title` |
| Bundle Identifier | `com.findprofile.app` | `src-tauri/tauri.conf.json::identifier` |
| 版本 | `0.1.0` | `src-tauri/tauri.conf.json::version`、`Cargo.toml`、`package.json` |
| 默认窗口 | 1100 × 720 | `app.windows[0].width / height` |
| 最小窗口 | 900 × 560 | `app.windows[0].minWidth / minHeight` |
| 最低 macOS | 11.0 (Big Sur) | `bundle.macOS.minimumSystemVersion` |
| App data | `~/Library/Application Support/com.findprofile.app/find_profile.db` | 由 `tauri::path::app_data_dir` + `db::init_db` 决定 |
| Logs | `~/Library/Logs/com.findprofile.app/find_profile.log.<date>` | `tracing-appender::rolling::daily` |

## 2. 图标

`src-tauri/icons/` 内的图标目前是 **Tauri 官方模板占位**（带 Tauri 标志）。Apple App Store / 公开发布之前需要用真实品牌图标替换全部尺寸。

需要替换的文件：
```
src-tauri/icons/32x32.png
src-tauri/icons/128x128.png
src-tauri/icons/128x128@2x.png
src-tauri/icons/icon.icns       ← macOS 真实使用的多尺寸图标
src-tauri/icons/icon.ico        ← Windows 才用，可以先留占位
src-tauri/icons/icon.png        ← 1024×1024 母图，icns 由它生成
```

生成 `.icns` 的便捷方式（Tauri CLI 自带）：

```bash
# 准备 1024×1024 PNG 命名为 app-icon.png 放工程根目录
npm run tauri icon ./app-icon.png
# 生成的图标会自动放到 src-tauri/icons/
```

## 3. 开发运行

```bash
# 装 / 升级 npm 依赖
npm install

# 仅前端 dev server（Vite，热更新）
npm run dev

# 桌面应用 dev（Vite + cargo + 系统 webview）
npm run tauri dev
```

`tauri dev` 启动后桌面窗口标题与 dock 名都是 **Find Profile**。Cmd+Option+I 打开 WebView DevTools。

## 4. 测试命令

```bash
# Rust 后端单测（最权威的健康度信号）
cd src-tauri && cargo test && cd ..

# 前端类型检查 + 生产构建
npm run build
```

两者通过是任何 release 之前的硬门槛。日常 dev 也建议在 commit 前都跑一遍。

## 5. 打包命令

```bash
npm run tauri build
```

这条命令做的事：
1. 跑 `npm run build` 产 dist。
2. 用 `cargo build --release` 编译后端（首次约 5–15 分钟，后续增量 1–3 分钟）。
3. 调 macOS 系统工具 `iconutil` / `lipo` 等组装 `.app`。
4. 生成 `.dmg` 安装镜像。

可选：限定架构（默认 host 架构）

```bash
# 仅 Apple Silicon
npm run tauri build -- --target aarch64-apple-darwin

# 仅 Intel
npm run tauri build -- --target x86_64-apple-darwin

# Universal 二进制（需先 rustup target add 两个 target）
npm run tauri build -- --target universal-apple-darwin
```

## 6. 打包产物位置

```
src-tauri/target/release/bundle/
├── macos/
│   └── Find Profile.app          ← 双击运行；可拖到 /Applications
└── dmg/
    └── Find Profile_0.1.0_<arch>.dmg
```

`<arch>` 为 `aarch64` / `x64` / `universal`，取决于 build 时的 target。

`.app` 直接可双击启动。`.dmg` 是分发用安装镜像。

## 7. 常见打包错误

| 现象 | 原因 / 处理 |
| --- | --- |
| `rustc could not link with cc` | macOS 缺 Xcode CLT。`xcode-select --install` |
| `error: failed to run custom build command for openssl-sys` 或类似 native dep | 不应出现 —— 我们的 deps（rusqlite bundled、notify、tauri）都不依赖系统 OpenSSL。出现说明意外引入了带 native 依赖的 crate，先看 `cargo tree` |
| `cannot find -licuuc` 等 ICU 链接错误 | macOS Sonoma+ 升级后偶发；在 Xcode CLT 重装后通常自愈 |
| `bundling ... CodeSign 失败` | 没装开发者证书时 `tauri build` 默认仍尝试 signing。把 `bundle.macOS.signingIdentity` 留空（默认即不签），或显式 `"signingIdentity": null` 跳过 |
| `app bundle missing icon.icns` | `src-tauri/icons/icon.icns` 不存在或损坏；用 `npm run tauri icon ./app-icon.png` 重新生成 |
| `App "Find Profile" can't be opened because the developer cannot be verified` | 未签名 / 未公证的 app 在新机首次打开时被 Gatekeeper 拦下。右键 → 打开 → 允许；或 `xattr -d com.apple.quarantine "Find Profile.app"` |
| `xcrun: error: invalid active developer path` | Xcode CLT 路径未设置：`sudo xcode-select --switch /Library/Developer/CommandLineTools` |
| 启动后白屏 | 检查 `~/Library/Logs/com.findprofile.app/find_profile.log.<date>`；常见因数据库目录权限或 app sandbox 配置错误 |
| dmg 体积异常大（数百 MB） | release build 通常 30–80MB；过大检查是否带了 debug symbols。`cargo build --release` 默认 strip 在 `[profile.release]` 配置 |

## 8. 签名与公证（后续步骤，MVP 不接）

MVP 输出的 `.app` 是**未签名 / 未公证**的，发给别人需要他们手动放行 Gatekeeper（见 §7 表）。正式发布前需要：

### 8.1 准备开发者账号 / 证书

1. Apple Developer Program 会员（$99/年）。
2. Xcode → Settings → Accounts 登入 Apple ID，下载 **Developer ID Application** 证书到本地 Keychain。
3. 在终端验证：
   ```bash
   security find-identity -v -p codesigning
   ```
   能看到 `Developer ID Application: <Your Name> (<TeamID>)` 即可。

### 8.2 配置 Tauri 自动签名

`src-tauri/tauri.conf.json`：
```json
{
  "bundle": {
    "macOS": {
      "minimumSystemVersion": "11.0",
      "signingIdentity": "Developer ID Application: Your Name (XXXXXXXXXX)",
      "entitlements": "entitlements.plist",
      "providerShortName": "XXXXXXXXXX"
    }
  }
}
```

`src-tauri/entitlements.plist`（按需，最小集合，启用 Hardened Runtime 必要）：
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.cs.allow-jit</key>
  <true/>
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
  <true/>
  <key>com.apple.security.cs.disable-library-validation</key>
  <true/>
</dict>
</plist>
```

### 8.3 公证 (notarization)

Apple 现行接口是 `notarytool`（取代旧的 `altool`）。

1. 创建 App-Specific Password：appleid.apple.com → Security → App-Specific Passwords。
2. 一次性把凭证存进 keychain：
   ```bash
   xcrun notarytool store-credentials AC_PASSWORD \
     --apple-id "you@example.com" \
     --team-id "XXXXXXXXXX" \
     --password "abcd-efgh-ijkl-mnop"
   ```
3. 公证 dmg：
   ```bash
   xcrun notarytool submit \
     "src-tauri/target/release/bundle/dmg/Find Profile_0.1.0_aarch64.dmg" \
     --keychain-profile AC_PASSWORD --wait
   ```
4. 公证通过后 staple ticket 到 dmg：
   ```bash
   xcrun stapler staple "src-tauri/target/release/bundle/dmg/Find Profile_0.1.0_aarch64.dmg"
   ```
5. 验证：
   ```bash
   spctl -a -t open --context context:primary-signature \
     "src-tauri/target/release/bundle/dmg/Find Profile_0.1.0_aarch64.dmg"
   ```

### 8.4 发布前自检

- `codesign -dvvv "Find Profile.app"` 输出包含 `Authority=Developer ID Application: ...` 与 `Sealed Resources version=2`。
- `spctl -a -vv "Find Profile.app"` 输出 `accepted source=Notarized Developer ID`。
- 用全新（没装过 dev cert 的）干净 macOS 账号双击 dmg → 装到 /Applications → 启动，应当无 Gatekeeper 警告。

### 8.5 不在 MVP 范围

- App Store 上架（需要额外的 entitlements + Sandbox + 不同的证书 "Mac App Store" + 隐私清单 PrivacyInfo.xcprivacy）。
- 自动更新（Tauri 有 updater 插件，需要服务端签名 + 公钥嵌入）。
- DMG 自定义背景 / 布局美化。

这些在正式 1.0 发布之前再处理。
