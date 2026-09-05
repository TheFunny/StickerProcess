# 发布与升级（NSIS）

`dx bundle --release --platform windows --package-types nsis` 产出的安装包
（`target/dx/<name>/bundle/windows/nsis/*-setup.exe`）的升级语义，与未来
应用内升级方案的设计空间。生成器：dioxus-cli 0.7.10（`StickerProcess.nsi`
模板可在 `target/dx/.../nsis/` 下查看）。

## 现状：覆盖式升级（开箱即用）

生成的 NSIS 安装器**天然支持升级**——新版 setup.exe 直接运行即完成升级：

| 机制 | 行为 |
|---|---|
| 安装目录 | 固定 `$LOCALAPPDATA\Programs\StickerProcess`，新旧版本同目录 |
| 文件覆盖 | NSIS `File` 指令默认 `SetOverwrite on`，旧文件被新文件替换 |
| 注册表 | 卸载项固定 key `com.stickerprocess.app`，覆盖更新不产生重复的"添加/删除程序"条目；`DisplayVersion` 随 `Cargo.toml` 版本更新 |
| 卸载器 | 旧 `uninstall.exe` 被新版覆盖 |
| 用户数据 | `settings.toml` 在 `%APPDATA%\StickerProcess`，升级/卸载均不受影响 |

## 现状缺失的三块

1. **无应用内更新检查**：未接入 `dioxus-updater` 插件，不会自动提示新版本。
2. **无运行中防护**：`.nsi` 没有程序运行检测——升级时若 StickerProcess
   正在运行，覆盖 `StickerProcess.exe` 会弹 NSIS 的"文件被占用"
   （Retry/Abort）对话框。用户需先手动退出程序。
3. **无版本比较**：已装新版再运行旧安装包不会被拦截（会静默降级覆盖）。

## 升级方案选项（按实现成本排序）

### 1. 自定义 .nsi 模板（低成本，解决 2/3）

dx 支持通过 `Dioxus.toml` 提供自定义 NSIS 模板。在生成的模板基础上追加：

```nsis
; 升级前检测程序是否运行（需要 nsProcess 插件或 FindWindow 轮询）
!insertmacro MUI_PAGE_WELCOME

Section "Install"
    ; 关闭正在运行的实例（taskkill 需要用户确认或静默 /IM）
    nsExec::Exec 'taskkill /IM StickerProcess.exe /FI "PID gt 0"'
SectionEnd
```

加上版本比较宏（读注册表 `DisplayVersion` 与 `VIProductVersion` 比较，
已装更新版本则弹窗确认降级）。

### 2. 应用内"检查更新"提示（中成本，解决 1）

启动时（或手动按钮）GET 一个版本清单 URL（如 GitHub Releases 的
`latest.json`），比较 `DisplayVersion`，发现新版弹 toast + 打开下载链接。
不引入签名体系，几十行代码；适合 GitHub Releases 作为分发渠道。

### 3. dioxus-updater 插件（完整方案，解决 1/2/3）

Tauri 式更新：签名 manifest（ed25519）+ 增量下载 + 静默替换重启。
需要：
- 服务器托管 manifest 与安装包（GitHub Releases 可用）
- 签名密钥对管理与 CI 集成
- `dioxus-updater` 插件接入与端点配置

适合正式对外分发后使用；个人工具现阶段收益有限。

## 相关联的事

- 静态构建（`E6_INPROCESS_RESEARCH.md` §7）落地后，安装包已不再捆绑
  `ffmpeg.exe`（`Dioxus.toml` 的 `resources` 已移除，2026-09-06）——
  shared exe 缺 DLL 在干净机器上无法启动，捆绑无意义；sidecar 引擎改为
  启动探测可用性后按需启用。
- release 构建已隐藏终端窗口（`windows_subsystem`），升级体验无黑框。
