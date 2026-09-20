# 待办清单（规划中尚未落地的项）

> 2026-09-21 汇总。各计划文档仍是细节出处，这里只做**索引 + 状态 + 优先级**。
> 完成一项就把对应行移到底部「已完成」或直接删掉（各计划文档里的原文不动）。
> 判定"未做"的依据都写在各行里（代码里搜什么、文档第几节）。

## A. 发布 / 分发（出处：docs/RELEASE.md「现状缺失的三块」）

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| A1 | 应用内"检查更新"提示 | 未做 | RELEASE.md §升级方案 2：启动/手动 GET Releases 的版本清单，比 `CARGO_PKG_VERSION` 新就提示 + 打开下载页。不引新依赖、几十行；适合 GitHub Releases 分发渠道 |
| A2 | NSIS 运行中防护 + 版本比较 | ✅ 已做（2026-09-21） | `build/nsis-hooks.nsh` + `[bundle.windows.nsis] installer_hooks`：`.onInit` 里先结束在跑的实例，再拒绝降级覆盖（交互式确认 / 静默退出码 3）。见 RELEASE.md §升级防护 |
| A3 | `dioxus-updater` 签名式完整更新（静默替换重启） | 未做 | RELEASE.md §升级方案 3。判"个人工具阶段收益有限"，要发正式分发渠道再说 |

## B. 功能 / 工程（出处：docs/REFACTOR_PLAN.md P5 + docs/MIGRATION_PLAN.md）

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| B1 | E4 并行转码（`tokio::sync::Semaphore` + 设置项"并行任务数"） | 未开始 | MIGRATION_PLAN §3 E4；全仓搜 `Semaphore` 无命中。前置：内存/CPU 实测（wasm 端无 Worker 时无意义，桌面才有价值） |
| B2 | i18n（fluent 或字符串表） | 未做 | REFACTOR_PLAN P5 #4（文档里唯一的空 checkbox 之一）。UI 已全英文化，建议随文档阶段盘点文案数量后再做 |
| B3 | CI 发布矩阵第 3 变体：完整版 + 随包静态 `ffmpeg.exe` | 未做 | REFACTOR_PLAN P5 #5(c)。现有 `-setup-webview` / `-setup-no-webview` / `-portable` 三件已覆盖 (a)(b)+便携。**与 C1 方向相反**：若决定弃用 sidecar，此项直接作废 |
| B4 | 任务行缩略图 | 暂缓 | MIGRATION_PLAN D4；`task_list.rs` 无缩略图代码 |
| B5 | 设置项：目标码率基准 / ffmpeg 路径 / 并行任务数 | 暂缓 / 预留 | MIGRATION_PLAN §3 B 表三个未勾行。前两项与既有语义冲突（超限只重试不改预算；探测走链接的 libav），第三项依赖 B1 |
| B6 | 应用内活动日志面板 | 仍缓 | REFACTOR_PLAN P4 可选延伸；错误场景已由 toast 覆盖 |

## C. 转码核心（出处：docs/E6_INPROCESS_RESEARCH.md）

| # | 项 | 状态 | 说明 |
|---|---|---|---|
| C1 | E6 第二阶段：删 `ffmpeg-sidecar` 依赖与 sidecar 分支 | 未做 | E6 §4/§7.6。**先决定 sidecar 去留**：它仍是 `Settings::default().engine`，且是用户可见的第二引擎（有 ffmpeg 的机器上更省内存/更快启动）。决定弃用后再删 `gen_command`、图片 stdout 管道、`Spawn`/`ReadOutput` |
| C2 | 实验：webm 时长补丁是否可删 | 未做 | E6 §3.4：libav muxer 直接写流时长，`44 89 88` hack 可能不再必要——需 Telegram 实测（贴纸时长检测）。补丁目前默认开启（`Settings::webm_duration_patch`） |

## D. 有意排除（已判定"不做"，不是欠账）

| 项 | 判定出处 |
|---|---|
| Web 端动画 webp alpha（双流 in-block 封装） | WebCodecs 收 I420A 但不产 alpha 位流，spike 判死（docs/WEB_DEMO_FINDINGS_C.md） |
| ffmpeg.wasm MT core（`-sUSE_PTHREADS`，需 COOP/COEP，Pages 不支持） | exec 已 1.2s，收益微小（FINDINGS_C §遗留） |
| Compatibility Report 的 "Copy report" 按钮 | 决策：v1 只展示（docs/COMPAT_REPORT_PLAN.md §8） |
| 报告里的 WebView2 版本号 / ffmpeg core 运行期加载状态 / 遥测 | 不可得或要 32MB 下载（同文档 §3） |
| Toast 堆叠折叠动画、弹窗退场动画、进度条改 `transform: scaleX`、`will-change` | docs/MOTION_REVIEW.md §3（实测无收益 / 复杂度不划算） |
| E2 去 `Arc<Mutex>` | REFACTOR_PLAN P3 复核否决（与"运行中拖入新文件"语义冲突） |
| Web 端 core 的 IndexedDB 缓存 | 与 HTTP 缓存等价（docs/WEB_PLAN.md 风险节） |
| 调试版右键菜单 | 有意保持现状（release 本就无） |
