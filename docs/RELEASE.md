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
2. ~~**无运行中防护**~~ → **已解决（2026-09-21）**，见下节「升级防护」。
3. ~~**无版本比较**~~ → **已解决（2026-09-21）**，见下节「升级防护」。

## 升级防护（2026-09-21 落地）

`build/nsis-hooks.nsh` 通过 `Dioxus.toml` 的 `[bundle.windows.nsis] installer_hooks`
被 dx `!include` 进生成的 `installer.nsi`（**不 fork 模板**——dx 升级模板时不用跟着改；
若将来 dx 自己定义 `.onInit`，会编译期报重复定义，是大声失败）。钩子只有一个入口
`.onInit`（任何 Section 之前运行，晚了就没意义）：

| 检查 | 行为 |
|---|---|
| 程序正在运行 | `taskkill`（不带 `/F`，即 WM_CLOSE 优雅退出）→ 等 800ms 复查 → 仍在则交互式询问 OK/Cancel（OK 才 `/F` 强杀，Cancel 中止安装）。**静默安装不弹窗，直接 `/F`**。<br>检测不靠窗口标题（`FindWindow` 会随标题改动失效），只看 `taskkill` 退出码（0 = 确实结束了进程，128 = 没找到）。 |
| 已装版本更新 | 读注册表 `DisplayVersion` 与**安装器自身**的 FileVersion 比较（模板把 `{{version}}` 写进了 VERSIONINFO）。已装更新 → 交互式确认"仍要装旧版吗"，**静默安装直接拒绝并以退出码 3 结束**（CI/无人值守不会把旧包盖上去）。读不到任一版本就放行——不为防守卡死正常安装。 |

版本比较直接用 NSIS 自带 `WordFunc.nsh` 的 `${VersionCompare}`（字段数不同按缺位补 0，
故已装 `0.1.0` 与安装包 `0.1.0.0` 判为相等），没有手写解析。

**两个易踩的点**（都已在 hooks 里规避）：

- 钩子文件**不经过 handlebars 渲染**，所以里面不能写 `{{version}}`；新版本号只能从
  安装器自身的 VERSIONINFO 读（`${GetFileVersion} "$EXEPATH"`）。
- NSIS 默认按 ANSI 代码页读脚本；hooks 里的中文注释要求 `makensis` 以 UTF-8 读入
  （dx 自己调用时已带该参数——本项目实测 `dx bundle --package-types nsis` 通过）。

**验证**（2026-09-21 实测，非推测）：

- 版本比较 8/8 用例（含 `0.1.0` vs `0.1.0.0` 判等、`0.10 > 0.1` 的数字比较）：
  用抽取出的 `STP_IsDowngrade` 编一个最小安装器跑静默安装，结果写文件读回。
- 降级拦截端到端：伪造 HKCU 测试键（`…\Uninstall\com.stickerprocess.guardtest`）
  `DisplayVersion=9.9.9` → 静默安装**退出码 3 且未写任何文件**；`0.0.1` / `0.1.0` /
  无值 → 正常继续。测试键结束后删除，真实键未被动过。
- 运行中防护端到端：把 app 复制成 `StickerProcessGuardTest.exe` 跑起来 → 静默安装
  rc 0 且继续，2 秒内该进程消失（supervisor 报 exit code 0，即优雅退出而非强杀）；
  无实例时 1.1s 直接放行。

## 升级方案选项（按实现成本排序）

### 1. ~~自定义 .nsi 模板~~ → 用 installer_hooks 实现（✅ 已完成 2026-09-21，解决 2/3）

原计划 fork 自定义模板；实际 dx 的模板已支持 `installer_hooks`（`[bundle.windows.nsis]`），
注入一个 `.nsh` 就够——**不必维护模板副本**（fork 的话 dx 每次升级模板都要手动跟）。
实现与验证见上面「升级防护」。

### 2. 应用内"检查更新"提示（中成本，解决 1 — 未做，见 docs/BACKLOG.md A1）

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
