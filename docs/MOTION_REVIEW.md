# 弹窗动画与 Toast 评审（2026-09-20）

**结论**：**性能上没有问题**，不需要为流畅度做任何事。所有入场/退场动画都只动
`opacity`/`transform`（合成层友好），实测 0 丢帧、0 长任务、空闲时 0 个运行中的动画、
重渲染不会重启动画。值得动的只有 4 件小事，其中只有 1 件算"缺陷"（无障碍），
其余是时长对齐与语义/观感。

> **已实施（2026-09-20）**：P0 + 两个 P1。
> - P0 `prefers-reduced-motion`（`app.css` 末尾一段 media query）
> - P1 淡出时长对齐：`app.rs` 的 `TOAST_FADE_MS = 350` 与 CSS 的 0.35s 一致，
>   淡出结束到移除的实测空窗 67ms → 16ms（≈ 采样粒度，已无死时间）
> - P1 弹窗自己的入场 `@keyframes modal-in`（`translateY(4px) + scale(0.98)`），
>   不再复用 toast 的水平滑入曲线
> - P2（堆叠折叠）按报告结论跳过；退场动画也未做（要 Rust 侧延迟卸载，不划算）
>
> 实测复核：`animationstart` 只 1 次（`modal-in`），reduce 模式下 `modal`/`toast`/两处
> 进度条/转码 spinner 的 animation/transition 全为 `none`（非 reduce 时原值不变）。

## 1. 现状（代码事实）

| 动画 | 属性 | 时长 | 触发 | 位置 |
|---|---|---|---|---|
| Toast 入场 | `opacity` + `translateX(16px)` → none | 0.18s ease-out（animation） | 新通知入队 | `app.css` `.toast` / `@keyframes toast-in` |
| Toast 退场 | 同上（`.leaving` → opacity 0 + translateX(16px)） | 0.35s **ease-in**（transition） | `Toast.leaving` | 同上 `.toast.leaving` |
| Toast 生命周期 | — | hold 3600ms（带 Undo 6000ms）→ `leaving` → **再等 400ms** → 从 Vec 移除 | `push_toast_inner` | `src/app.rs` |
| 弹窗入场 | **复用 `toast-in`**（水平滑入 16px + 淡入） | 0.15s ease-out | 弹窗挂载 | `app.css` `.modal` |
| 弹窗退场 | 无（`show_* = false` 即刻卸载） | — | Esc/背景/× | — |
| 背景遮罩 | `rgba(0,0,0,0.4)` 无过渡，瞬现 | — | 同上 | `.modal-backdrop` |
| 行内进度条 | `width` % | 0.2s ease-out | 转码中 ~10Hz 更新 | `.row-progress-fill` |
| 总体进度条 | `width` % | 0.15s ease-out | 每任务一次 | `.progress-fill` |
| Processing 徽标 | `rotate` 无限 | 0.8s linear | 转码中（同时最多 1 个） | `.badge.processing::before` |

## 2. 实测数据

| 指标 | 结果 | 复现方式 |
|---|---|---|
| 丢帧 | **0**（870 帧 / 14.5s，无一帧间隔 > 30ms） | `requestAnimationFrame` 采样 + 连续 5 个 toast 进出 |
| 长任务 | toast 期间 **0**；转码期间 2 个（59ms、73ms，属 ffmpeg.wasm 主线程编码） | `PerformanceObserver('longtask')` |
| 空闲动画残留 | **0**（`document.getAnimations()` 为空） | 无 toast/无转码时取样 |
| 动画重启 | 新 toast 入队、弹窗内容切换（Re-check）都**不**重启既有动画（各元素 1 次 `animationstart`） | 动画事件埋点 + keyed list |
| Toast 淡出 → DOM 移除 | **67ms** 空窗（CSS 350ms vs Rust 400ms + 采样粒度） | 10ms 轮询计数 + `transitionend` |
| Toast 堆叠折叠 | 相邻 toast **瞬间位移 −42.6px**（一帧内），无过渡 | 16ms 采样 rect |
| `prefers-reduced-motion` | **完全未处理**：模拟 `reduce` 后 `animation-name` 仍为 `toast-in` | CDP `setEmulatedMedia` |
| 进度条宽度过渡 | 单次转码 23 次 `width` transition，144 个不同采样宽度，**无长任务** | `transitionstart` 计数 |

## 3. 建议（按性价比排序）

### P0 `prefers-reduced-motion`（唯一算缺陷的一项）— ✅ 已实施
现在无论用户是否开启"减少动态效果"，滑入/淡入、模态动画、spinner 都照跑。加一段
media query 即可，约 8 行，零风险：

```css
@media (prefers-reduced-motion: reduce) {
  .toast, .modal { animation: none; }
  .toast { transition: none; }
  .row-progress-fill, .progress-fill { transition: none; }
  .badge.processing::before { animation: none; }
}
```
（`transition: none` 会让退场的 toast 立刻到终态——反正 400ms 后就被移除，观感无差。）

### P1 淡出时长与移除等待对齐（1 行）— ✅ 已实施
`transition` 0.35s ↔ Rust 等 400ms → 每个 toast 有 ~67ms"已经不可见、还在 DOM 里"的
空窗。把两处改成同一个值（建议都 0.35s / 350ms）即可。收益：少 67ms 的无效节点存活；
风险：无。实施：`app.rs::TOAST_FADE_MS = 350`，注释里写明"必须与 `.toast` 的
transition 一致"（跨文件共享不了这个数，只能靠注释互指）。

### P1 弹窗自己的入场/退场（约 10 行）— 入场 ✅ 已实施，退场未做
- 弹窗复用 toast 的 `@keyframes toast-in`（16px **水平**滑入）——居中对齐的对话框从
  右侧滑入在视觉上没有来由；改用 `@keyframes modal-in`（`translateY(4px) + scale(0.98)`）。
- 弹窗**没有退场**，而 toast 有 → 不一致。补一个 `.modal-backdrop.closing` +
  `animation: fade-out 0.12s` 需要 Rust 侧配合延迟卸载（Toast 那套 `leaving` 机制的
  翻版），**只有真觉得需要才做**（本次未做）。
- 拆 keyframes 还顺手解掉一个坑：改 toast 的入场曲线不再连带改弹窗。

### P2 Toast 堆叠折叠（建议先不做）
移除一条通知时，其它 toast 一帧内瞬移 42.6px。做平滑折叠要么 FLIP（读 rect → 反向
transform → 下一帧放开），要么 `grid-template-rows: 0fr → 1fr` 技巧，两者都要给
"进场/退场"两态各写一套时序，还要和 `column-reverse`、`max-width` 换行共处。
收益纯观感，复杂度不小——**除非明确要这个质感，否则跳过**。

### 不做（明确记录理由）
- **`will-change`**：手动常驻会让元素长期占合成层显存；Chromium 对 `opacity`/`transform`
  动画已自动提升，实测也无掉帧。
- **进度条 `width` → `transform: scaleX()`**：这是全 UI 唯一动布局属性的动画，理论上有
  优化空间，但实测 23 次过渡、0 长任务、0 丢帧（一个 4px 高的条，栅格化成本可忽略）。
  等真有观感问题（例如加 shimmer）再换。
- **降低 toast 停留时长/数量上限**：当前 5 条实测无掉帧，且堆叠有 `max-width` 约束；
  除非出现通知风暴场景，否则不为假想问题加代码。

## 4. 复现方法（本文数据都可重跑）

```bash
dx build --platform web
cp assets/*.js assets/*.png assets/ffmpeg-core-st.* target/dx/StickerProcess/debug/web/public/
python -m http.server 8123   # 在 public/ 下
```
埋点：`animationstart`/`transitionstart`/`transitionend` 捕获 + `PerformanceObserver`
（`longtask`）+ rAF 采样帧间隔 + `getAnimations()`；`prefers-reduced-motion` 用
CDP `Emulation.setEmulatedMedia` 模拟。桌面（WebView2）用
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` 挂 CDP 复测。
