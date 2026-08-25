# StickerProcess 迁移与功能规划

> 状态: 规划草案 (2026-08) · 目标: 从 iced 0.14 迁移到 Dioxus 0.7 desktop，并落地预览/设置/视觉增强

## 0. 背景与已验证结论

- 现有 Rust 实现: iced 0.14 GUI (`src/main.rs`) + 框架无关核心 (`media.rs`, `transcoder.rs`)
- **拖拽验证通过** (`spike/dioxus-dnd-demo`, Dioxus 0.7.10): Windows 下 `ondrop` → `evt.data.files()` → `FileData.path()` 返回完整路径（中文/空格/括号文件名均正确）
- iced 在 Windows 0.6.3 的拖拽 bug (issue #2954) 在 0.7.x 已修复
- 迁移核心原则: **`media.rs` / `transcoder.rs` 零改动或最小改动，只重写 UI 层**

## 1. 技术选型

| 项 | 选择 | 理由 |
|---|---|---|
| GUI | dioxus 0.7 (desktop, wry/WebView2) | 原生 `<img>`/`<video>` 预览；已验证拖拽路径 |
| 文件对话框 | dioxus 原生 `<input type="file">` (内部走 rfd) | 无需额外依赖 |
| 异步 | tokio + dioxus `spawn`/`use_future` | 与现有 `spawn_blocking` 兼容 |
| 配置持久化 | serde + toml, 存 `%APPDATA%/StickerProcess/settings.toml` | 简单可靠 |
| 打包 | dioxus-cli `dx bundle` (tauri-bundler) | 出 NSIS/MSI 安装包 |
| 日志 | log + dioxus-logger, 可选写文件 | 保留现有 `log::*` 调用 |

依赖变更 (Cargo.toml): 移除 `iced`/`iced_aw`; 新增 `dioxus 0.7`、`serde`、`toml`; 保留 `ffmpeg-the-third`、`ffmpeg-sidecar`、`oxipng`、`tokio`、`chrono`、`log`。

## 2. 迁移步骤 (Phase A: 功能对等)

### A1. 工程改造
- [x] 原地迁移: 替换 Cargo.toml 依赖，保留 git 历史
- [x] `media.rs`/`transcoder.rs` 原样保留（后续只做小改动: 见 D1）
- [x] 新建 `main.rs` (bootstrap) + 模块拆分:
  - `app.rs` — 根组件、全局信号
  - `runner.rs` — 异步转码循环 + 重试 + 取消
  - `components/` — 工具栏、任务行、拖拽区、进度、设置面板、预览、toast
    （Phase A 已落地工具栏/任务行/拖拽区/进度/数值输入；设置面板、预览、toast 随 B/C/D 落地）

### A2. 状态模型 (iced Message → dioxus Signal)

```rust
// iced 的 App/Message/update 架构 → dioxus 信号 + 异步任务
struct TaskEntry {
    transcoder: Arc<Mutex<Transcoder>>,  // 保留共享模式，零逻辑改动
    input_size: u64,
    input_duration: Option<f64>,         // 添加时异步探测
}

signals:
  tasks: Signal<Vec<TaskEntry>>,    output_dir: Signal<String>,
  max_retry: Signal<u8>,            running: Signal<bool>,
  overall_progress: Signal<f32>,    cancel: Signal<bool>,
  toasts: Signal<Vec<Toast>>,       settings: Signal<Settings>,
```

### A3. 转码执行流 (消息链 → 异步循环)

现有 `SelectRun → CurrentProcess → NextProcess → Done` 改为一个 async 任务:

```rust
async fn run_all(state) {
  for i in 0..n {
    let mut retry = 0;
    loop {
      set_status(i, Processing);
      let res = spawn_blocking(|| tasks[i].lock().run()).await;
      // 与现在完全相同的尺寸检查 + 系数调整 (factor /= excess * 0.96)
      // 超限且 retry < max_retry → 继续重试; 否则进入下一个任务
      if cancel { break }
    }
    overall_progress.set((i+1) as f32 / n as f32);
  }
}
```

- 保留 `Arc<Mutex<Transcoder>>` 先跑通，重构优先级放后面 (D2)
- 转码期间禁止编辑系数: 与 iced 相同 (running 时禁用输入)

### A4. UI 组件对等移植

- 工具栏: 添加文件、清空完成、输出目录输入+选择、重试次数 NumberInput、运行按钮
- 任务列表: 状态徽标 + 文件名 + 输出大小 (颜色区分超限) + 系数 NumberInput
- 进度: 总体进度条 (沿用 `(index+1)/len` 语义)
- 拖拽: 已验证的 `ondrop` + 悬停高亮挂 `ondragover` (Windows 不触发 dragenter)
- 文件对话框: 隐藏 `<input type="file" multiple accept=...>` 由按钮触发

### A5. 验收 (与 iced 版行为对等)

- [ ] 三种输入 (mp4/gif/apng/jpg/png) 转码结果与现版一致
- [ ] 尺寸重试逻辑 (256KB/512KB 上限、系数调整) 一致
- [ ] 拖拽添加、输出目录选择、清空完成一致

## 3. 新增功能

### B. 设置选项 (Phase B) ✅ 已完成 (2026-08)

持久化配置 `settings.toml` (`%APPDATA%/StickerProcess/`), 设置面板 (工具栏 Settings 按钮):

| 设置项 | 默认 | 状态 |
|---|---|---|
| 输出目录 | `./output` | [x] 已持久化 |
| 最大重试次数 | 3 | [x] 已持久化 |
| 视频大小上限 | 256 KB | [x] 已持久化 |
| 图片大小上限 | 512 KB | [x] 已持久化 |
| GIF 系数乘数 | 0.75 | [x] 已移除 (码率 patch 不再需要) |
| 重试缩放系数 | 0.96 | [x] 已持久化 (`factor = factor / excess * shrink`) |
| 时长→系数表 | <1s:1.2 ... ≥8s:0.7 | [x] 已持久化 (区间固定, 系数可编辑) |
| 强制帧率 | 0 (自动) | [x] 已持久化 (>0 时追加 `-r`) |
| 目标码率基准 | 256*1024*8 | [ ] 高级选项, 暂缓 |
| ffmpeg 路径 | 自动 (PATH/FFMPEG_DIR) | [ ] 暂缓 (探测走链接的 libav, 仅覆盖 CLI 路径语义不完整) |
| 主题 | 浅色 | [x] 已持久化 (use_effect 同步 data-theme) |
| 语言 | 中文 | [ ] 低优先, 暂缓 |
| 并行任务数 | 1 | [ ] 预留 (见 D4) |

- 实现: `config.rs` (load/save/默认值/回填输出目录 + 往返单测) + `settings_panel.rs` 模态弹窗
- 保存时机: 变更即写 (500ms 防抖, 代数计数器保证最新一次落盘)

### C. 优化功能与视觉提示 (Phase C) ✅ 已完成 (2026-08)

1. [x] **每任务实时进度**: `ffmpeg-sidecar` 的 `iter()` 直接解析 ffmpeg stderr 进度行
   （无需 `-progress pipe:1`），`transcoder.rs` 增加 `run_with_progress(callback)`，
   进度经 mpsc 通道写入任务镜像；任务行内进度条 + 转码耗时统计
2. [x] **状态视觉**: 徽标颜色 + Processing 旋转指示器 (Pending 灰 / Probing 灰 /
   Processing 蓝+旋转 / Done 绿 / Alert 红 / SizeExcess 橙)
3. [x] **Toast 通知**: 错误、完成、尺寸重试、取消提示 (右上角堆叠, 4 秒自动消失)；
   替代原 rfd 错误弹窗（rfd 仅保留目录选择）
4. [x] **主题系统**: `app.css` CSS 变量, 浅色/深色切换 (`data-theme` 属性, 工具栏按钮)；
   持久化随 Phase B 设置落地
5. [x] **取消能力**: cancel 信号 → watcher → `Transcoder.cancel_flag` (Arc<AtomicBool>)
   kill ffmpeg 进程；任务回到 Pending，队列停止；工具栏 Cancel 按钮
6. [x] **拖拽覆盖层**: 全窗虚线高亮 + 提示文案 (Phase A 已有，Phase C 换主题变量)
7. [x] **任务行工具提示**: 悬停显示完整路径 + 最近错误详情
8. [x] **添加时异步探测**: 后台线程 probe，显示 "Probing…"；探测期间禁止 Run

### D. 预览效果 (Phase D) — 迁移的最大动机

1. **图片/GIF 预览**: `<img src="data:image/png;base64,...">` (贴纸 ≤512KB, data URL 完全够用)
2. **视频预览**: `<video src="data:video/webm;base64,..." controls loop muted autoplay>`
   - 输出 ≤256KB, data URL 可行; 大文件需自定义协议 + Range 支持 (后续)
3. **点击任务行 → 预览弹窗/侧栏**: 原图 vs 输出并排, 显示尺寸对比 (输入 X KB → 输出 Y KB, 达标 ✓/超限 ✗)
4. **缩略图 (可选)**: 图片直接缩略; 视频用 ffmpeg 抽首帧缓存
5. **预览与转码联动**: 转码完成后自动刷新预览 (输出文件已生成)

### E. 重构与优化 (Phase E, 持续)

- **E1 错误处理**: `transcoder.rs` 的 `Result<(), &str>` → `thiserror` 枚举, 错误详情直达 UI
- **E2 状态精简**: 评估去掉 `Arc<Mutex>`, 由 runner 独占任务列表 (信号读写同步) — 仅在 UI 与 runner 无并发编辑冲突时做
- **E3 模块化**: 组件/逻辑/配置分层, 单文件 ≤ ~300 行
- **E4 并行转码 (可选)**: 设置项控制并发数, `tokio::sync::Semaphore` + 独立进度
- **E5 打包发布**: `dx bundle` (NSIS/MSI); ffmpeg 随包分发或首次运行自动下载 (ffmpeg-sidecar 支持)
- **E6 测试**: 配置往返、转码参数生成、重试逻辑纯函数化后单测、media 现有测试
- **E7 文档**: 更新 AGENTS.md (架构变更)、README (用法)

## 4. 里程碑估算

| 阶段 | 内容 | 预估 |
|---|---|---|
| A | 迁移功能对等 | 2–4 天 |
| B | 设置 + 持久化 | 1–2 天 |
| C | 视觉 + 实时进度 + 取消 + toast | 1–2 天 |
| D | 预览 (图/GIF/视频) | 1–2 天 |
| E | 重构 + 打包 + 测试 | 2–3 天 |

## 5. 风险与注意事项

- **WebView2 依赖**: Win10/11 预装; 老系统不支持 (iced 无此限制)
- **进度事件**: 需确认 `-progress pipe:1` 与 ffmpeg 版本兼容; 进度粒度取决于 ffmpeg 输出频率
- **data URL 大小**: 视频预览依赖 ≤256KB 输出 (贴纸场景天然满足); 大文件预览需自定义协议 + HTTP Range (视频拖动/续播必需)
- **dx bundle 成熟度**: 已知默认值问题 (issue #4018), 打包参数需显式指定
- **并发编辑冲突**: runner 持有任务时 UI 禁编辑系数, 与现版语义一致

## 6. 落地顺序建议

A(对等迁移) → C1/C2(实时进度+视觉, 迁移完立刻有体感) → B(设置) → D(预览) → E(重构打包)

> 注: B/C/D 均可并行开发, 但建议按 A → C → B → D → E 顺序提交, 每步可独立验证。