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

## P2 — 结构性重构（随 D 或 D 后）

- [ ] **5. `transcoder.rs` 拆分** (380 行混四件事)：`check_input` 归还 `media.rs`；
      命令生成 / webm duration patch / 图片管道+oxipng 各自成模块
- [ ] **6. `runner.rs run_all` 分解** (170 行双层循环)：拆出 `run_one_task() -> Decision`；
      取消 watcher 改为每任务一个（现为每次尝试 spawn + abort）
- [ ] **7. 镜像同步双轨制统一** ⚠️：`with_task` 刷 status/factor/output_size，
      `set_mirror` 刷 progress/elapsed/error，两条路径靠记忆维护（本次开发已踩坑两次）。
      统一为单一 `sync_mirror(entry, &Transcoder)`，所有修改后必经此口
- [ ] **8. 错误处理枚举化 (E1)**：`&str` 贯穿 transcoder→runner→mirror/toast；
      取消判断靠 `e == "Cancelled"` 字符串比较。`thiserror` 定义
      `CommandBuild / Spawn / Cancelled / DurationPatch / ImagePipe / SizeCheck`
- [ ] **9. 进度事件节流**：unbounded mpsc 每事件触发整表重渲染；当前规模无碍，
      任务多/视频长时接收端按 ~10Hz 合并

## P3 — 暂缓

- [ ] **E2 去 `Arc<Mutex>`**：依赖 D 的预览读取形态，先不动
- [ ] **魔法数常量化**（码率基准 `256*1024*8` / crf 26 / bufsize ×1.5）：等"码率基准进设置"一起做
- [ ] **config 防抖退出丢写**：500ms 窗口内退出丢最后一次修改，影响极小

## 已核查无需动

- 锁纪律：无跨 `.await` 持锁
- cancel watcher 所有 break 路径均有 abort
- `run_all` 启动时快照 Settings：运行中改设置下次 Run 生效——有意语义，已文档化
