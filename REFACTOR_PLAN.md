# 重构优化计划

> 状态: Phase B/C 完成后、Phase D 开工前的代码健康评估 (2026-08-25)。
> 依据: 逐文件核查 (~2400 行 Rust)，编译警告、重复定义、测试盲区。

## P1 — D 开工前执行（小改动、低风险）

### 1. 死代码清理 ✅
- [x] `media.rs`: `sticker` 字段 / `sticker()` 方法 / `StickerType` 枚举删除
      （D 预览需要 Static/Animated 区分时再以真实用途回归）
- [x] `transcoder.rs` 未使用的 `StickerType` import 删除
- [x] `app.rs` `VIDEO`/`IMAGE` 的 `#[allow(dead_code)]` 移除（被第 2 项真实消费）

### 2. 扩展名列表统一 ✅
- [x] `SUPPORTED` 从 `VIDEO`/`IMAGE` 编译期拼装
- [x] `FILE_ACCEPT` 改为从 `SUPPORTED` 派生 (`file_accept()`)
- [x] 单测 `supported_extensions_are_consistent`：SUPPORTED 每项可识别 +
      VIDEO/IMAGE 与 SUPPORTED 一致 + 未支持扩展被拒

### 3. number_field 泛型化 ✅
- [x] 合并为泛型 `NumberInput<T>`（`NumEdit` trait：parse/clamp/pretty），
      调用点改为类型参数形式，行为不变（编辑态模式保留）

### 4. 纯函数抽取 + 单测 ✅
原为零覆盖的纯逻辑（码率公式、时长→系数查表、重试缩放），现已抽出并测试：
- [x] `target_bitrate_bps` / `default_factor` / `quantized_bitrate` /
      `shrunk_factor`(pub, runner 使用) 抽取到 transcoder.rs
- [x] 单测覆盖区间端点、量化取整、shrink 公式、进度时间解析（+7 测试）

## P2 — 结构性重构 ✅ 已完成 (2026-08-26)

- [x] **5. `transcoder.rs` 拆分** → `src/transcoder/`：
      `mod.rs`（类型+编排）、`command.rs`（命令生成+码率纯函数+单测）、
      `steps.rs`（webm 时长补丁、oxipng 图片管道）、`error.rs`（TranscodeError）；
      `check_input` 以 `MediaFile::probe` 归还 `media.rs`
- [x] **6. `runner.rs run_all` 分解**：拆出 `run_single_task() -> TaskOutcome`；
      取消 watcher 改为每任务一个（跨重试存活，函数出口统一 cancel）
- [x] **7. 镜像写入口统一**：`with_task`（派生字段）与 `touch_entry`（UI-only 字段）
      两个入口 + AGENTS.md 契约文档化；runner/probe/接收循环全部改道
- [x] **8. 错误处理枚举化 (E1)**：`TranscodeError`（thiserror）贯穿
      transcoder→runner→mirror/toast；取消用 `matches!(e, Cancelled)` 判定
- [x] **9. 进度事件节流**：接收端 100ms interval 合并同任务进度再写镜像

## P3 — 暂缓项可行性结论 (2026-08-26 复核，D 完成后)

### E2 去 `Arc<Mutex>` — **不建议做**
D 完成后逐点核查锁的使用：
- 预览渲染期锁已消除（输出路径进镜像 `output_path`，附带修复了
  "预览开着转码时 UI 冻结" 的实际 bug）
- 剩余锁临界区都很短：`with_task` 刷镜像、probe、cancel_flag clone
- factor 编辑在 running 时本就禁用，不存在 UI 与 worker 并发写

去掉 `Arc<Mutex>` 要求 runner 独占任务列表，与"运行中允许拖入新文件"
的既有语义冲突（需要额外的消息桥）。收益趋近于零，改动面大。
**结论：保留现状。** 若将来做 E4 并行转码，每任务独立的 `Arc<Mutex<Transcoder>`
反而是天然的任务分发单元——届时更没有去除的理由。

### 魔法数常量化 — **常量部分已随 P2 完成，"进设置"仍缓**
`BITRATE_BASE_BYTES` 已是 `command.rs` 具名常量。剩余 crf 26 / bufsize ×1.5
提为常量即可（纯命名，无行为变化）；"码率基准进设置"仍是行为变更
（码率会随上限变化），与"超限时只重试不改预算"的现有语义冲突，维持暂缓。

### config 防抖退出丢写 — **建议改为即时保存（待实施）**
settings.toml 只有 ~200 字节，写盘亚毫秒级。500ms 防抖的全部收益
抵不过退出丢写的边界风险。改为 `update_settings` 内直接
`spawn_blocking(config::save)`，删除 SAVE_GEN 代数计数器。

## P3 执行清单（更新于 2026-08-26）

- [ ] crf 26 / bufsize ×1.5 提为具名常量（command.rs）
- [ ] update_settings 防抖改为即时落盘（删除 SAVE_GEN）
- [x] ~~E2 去 Arc<Mutex>~~ — 复核后否决，理由见上
- [x] 预览渲染期锁消除（output_path 镜像，D 阶段遗留 bug）

## 已核查无需动

- 锁纪律：无跨 `.await` 持锁
- cancel watcher 所有 break 路径均有 abort
- `run_all` 启动时快照 Settings：运行中改设置下次 Run 生效——有意语义，已文档化

## P1.5 — 立即可修的样式缺陷（2026-08-26 反馈）✅

### 5. 亮色主题下 btn-primary/btn-danger hover 白字不可见
- [x] 新增主题变量 `--success-hover` / `--danger-hover`：
      亮色取更深同色系（`#166b2e` / `#a40e26`），深色取微亮（`#4ac269` / `#ff6a63`）
- [x] `.btn-primary:hover` / `.btn-danger:hover` 改用变量，删除 `filter: brightness(1.08)` 写法

## P4 — Phase D 后用户反馈项（2026-08-26）

- [ ] **1. 右键菜单（暂时保持不变）**：当前是 WebView2 默认菜单。根因：dioxus-desktop 的
  `disable_context_menu` 默认 `!debug_assertions`——仅 debug 构建出现，release 本就没有。
  方案 A（推荐）：`main.rs` 加 `.with_disable_context_menu(true)` 一行彻底关闭；
  方案 B（仅当需要"复制图片"等自定义动作）：注入 JS `contextmenu` preventDefault 自绘菜单
- [ ] **2. 日志增强**：runner 决策点目前零日志（重试、factor 调整、取消、完成均无提示）。
  在 Decision::Retry、factor.set 前后、取消、Done 处补 `log::info/warn`
  （含 old→new factor、excess 倍率、输出大小）。与 E1 相互独立，可先行；
  可选延伸：应用内活动日志面板（toast 已覆盖错误场景，优先级低）
- [ ] **3. 主题跟随系统**：Settings.theme 增加 `"system"` 档：
  CSS `@media (prefers-color-scheme: dark)` 兜底 +
  JS `matchMedia('(prefers-color-scheme: dark)')` 监听切换回写 `data-theme`；
  设置面板 Theme 下拉加 "System" 并作为默认值
- [ ] **6. 任务行显示输出文件名**：`TaskEntry` 增加镜像字段
  `output_file_name: Option<String>`（`with_task` 从 `get_output().file_name()` 同步），
  任务行在输出大小旁展示文件名（悬停 tooltip 已含完整路径）

## P5 — 远期规划

- [ ] **4. i18n**（原 B 表低优先项）：界面文案现散落在各组件 rsx 中；
  引入 fluent 或简单字符串表前先盘点文案数量，建议随 E7 文档阶段一起做
