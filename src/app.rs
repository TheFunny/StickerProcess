//! 根组件与全局信号状态。
//!
//! 任务仍以 `Arc<Mutex<Transcoder>>` 共享给后台 worker（共享模式不变），
//! 另存少量"显示镜像"字段，让 UI 渲染时**不加锁**（worker 转码期间会长期持有锁）。
//! Phase B：`Settings` 成为全部可配置项的唯一事实源，变更即时同步写盘。

use crate::components::{
    drop_zone::DropZone, progress_bar::ProgressBar, settings_panel::SettingsPanel,
    task_list::TaskList, toast::Toast, toast::ToastKind, toolbar::Toolbar,
};
use crate::config::{self, Settings};
use crate::media::MediaFile;
use crate::transcoder::{Status, Transcoder};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static TOAST_ID: AtomicU64 = AtomicU64::new(1);

/// Toast 淡出时长（ms）。**与 `app.css` 里 `.toast` 的 transition 时长必须是同一个数**
/// ——两处各持一份，跨文件共享不了：比 CSS 长则通知已全透明却继续占着 DOM（实测差
/// 50ms 时约 67ms 空窗），比 CSS 短则淡出被硬切断。
const TOAST_FADE_MS: u64 = 350;

/// 显示镜像：任务行/预览渲染所需的全部字段（不含共享的 Transcoder）。
///
/// 刻意派生 `PartialEq`——手写 eq 漏一个字段会让 dioxus memo **静默**跳过重渲染
/// （历史 bug：系数不显示、状态/错误徽标不刷新）。派生之后不可能再漏。
#[derive(Clone, Debug, PartialEq)]
pub struct TaskMirror {
    /// 输入文件路径（创建时快照，避免渲染期加锁）。
    pub input_path: String,
    /// 预览展示输入大小的对比基准。
    pub input_size: u64,
    pub is_video: bool,
    // ---- 以下由修改 Transcoder 的一方负责同步 ----
    pub status: Status,
    pub output_size: Option<u64>,
    /// 输出文件名（不含路径，随 output_path 同步），任务行直接展示。
    pub output_file_name: Option<String>,
    pub factor: Option<f64>,
    /// 输出文件路径镜像（set_output_dir 时同步），供预览免锁读取。
    pub output_path: Option<PathBuf>,
    /// 最近一次成功转码的耗时。
    pub elapsed_ms: Option<u64>,
    /// 最近一次失败的错误详情（悬停工具提示展示）。
    pub error: Option<String>,
}

/// 队列中的一个任务：共享 Transcoder（逻辑层）+ 显示镜像（渲染层）。
#[derive(Clone, Debug)]
pub struct TaskEntry {
    pub transcoder: Arc<Mutex<Transcoder>>,
    pub mirror: TaskMirror,
}

/// 组件 Props 派生需要 PartialEq；只比显示镜像（两边都锁 Transcoder 比较会死锁）。
impl PartialEq for TaskEntry {
    fn eq(&self, other: &Self) -> bool {
        self.mirror == other.mirror
    }
}

impl TaskEntry {
    pub fn new(media: MediaFile) -> Self {
        let input_path = media.display_name();
        let is_video = matches!(media.r#type(), Some(crate::media::MediaType::Video(_)));
        let input_size = media.input_len().unwrap_or(0);
        log::info!("Added task: {input_path}");
        Self {
            transcoder: Arc::new(Mutex::new(Transcoder::new(media))),
            mirror: TaskMirror {
                input_path,
                input_size,
                is_video,
                status: Status::Probing,
                output_size: None,
                output_file_name: None,
                output_path: None,
                factor: None,
                elapsed_ms: None,
                error: None,
            },
        }
    }

    /// 输出大小相对上限的倍率（>1.0 即超限），无输出时返回 None。
    pub fn size_excess_ratio(&self, video_limit: u64, image_limit: u64) -> Option<f64> {
        crate::transcoder::excess_for(
            self.mirror.is_video,
            self.mirror.output_size,
            video_limit,
            image_limit,
        )
    }
}

/// 最近一次删除的撤销槽（回插所需的一切都在条目里，含已转码产物）。
#[derive(Clone)]
pub struct UndoSlot {
    /// 与之绑定的通知 id：通知被更晚的删除顶掉后，旧按钮不能再生效。
    pub toast_id: u64,
    /// (原下标, 任务)，按原下标升序。
    pub removed: Vec<(usize, TaskEntry)>,
}

/// clear_done 的收集件：按**原下标**拆出 Done 任务、其余保序。抽成函数是因为
/// 撤销回插依赖"原下标"记账——测试必须驱动同一份实现，复刻循环在 clear_done
/// 改坏下标记账时照样绿。
fn split_done(list: Vec<TaskEntry>) -> (Vec<TaskEntry>, Vec<(usize, TaskEntry)>) {
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for (pos, entry) in list.into_iter().enumerate() {
        if entry.mirror.status == Status::Done {
            removed.push((pos, entry));
        } else {
            kept.push(entry);
        }
    }
    (kept, removed)
}

/// 把撤销槽里的条目按原下标升序回插。删除期间队列可能又变短（撤销前删了别的
/// 任务），下标越界时贴到末尾——原位回插才能还原原序。
fn reinsert_removed(list: &mut Vec<TaskEntry>, removed: Vec<(usize, TaskEntry)>) {
    for (index, entry) in removed {
        let at = index.min(list.len());
        list.insert(at, entry);
    }
}

/// 全部界面状态（signals）。`Signal<T>` 为 Copy，可按值传递。
#[derive(Clone, Copy)]
pub struct UiState {
    pub tasks: Signal<Vec<TaskEntry>>,
    /// 全部可配置项（唯一事实源，变更即时同步落盘）。
    pub settings: Signal<Settings>,
    /// 设置面板开关。
    pub show_settings: Signal<bool>,
    /// 当前打开预览的任务下标。
    pub show_preview: Signal<Option<usize>>,
    /// 兼容性报告弹窗开关。
    pub show_compat: Signal<bool>,
    pub running: Signal<bool>,
    pub overall_progress: Signal<f32>,
    /// [桌面] 有未落盘的设置修改（关窗兜底 flush 据此决定是否补写）。
    /// wasm 即时写 localStorage，无去抖窗口，不需要此状态。
    #[cfg(not(target_arch = "wasm32"))]
    pub save_pending: Signal<bool>,
    /// [桌面] 设置修改代数：每次修改递增，旧去抖任务见代数不匹配即退场——
    /// 只有最新一次修改的任务真正写盘，等价于原同步写的单写者保序。
    #[cfg(not(target_arch = "wasm32"))]
    pub save_epoch: Signal<u64>,
    /// 行内转码进度 (任务下标, 0..=1)，仅 Processing 期间有值。
    /// 刻意不放进 TaskMirror：镜像随 tasks 信号整表订阅，10Hz 进度写入会把
    /// TaskList/Summary/Toolbar/Preview 全部标脏重跑（它们的数字在 10Hz 尺度
    /// 上从不变化）。独立信号 = 只有读它的任务行被唤醒。
    /// 顺序执行（同时至多一个任务在跑），单槽足够，增删队列无需同步。
    pub progress: Signal<Option<(usize, f32)>>,
    /// 取消标记：runner 在每次尝试前检查；运行中的任务经 cancel_flag 中断 ffmpeg。
    pub cancel: Signal<bool>,
    pub toasts: Signal<Vec<Toast>>,
    /// 最近一次删除的撤销槽（`undo_remove` 消费）。
    pub undo: Signal<Option<UndoSlot>>,
}

impl UiState {
    /// 锁定第 index 个任务执行 `f`，随后刷新显示镜像并通知订阅者。
    pub fn with_task<R>(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut Transcoder) -> R,
    ) -> Option<R> {
        self.tasks.with_mut(|list| {
            let entry = list.get_mut(index)?;
            let mut task = entry.transcoder.lock().ok()?;
            let out = f(&mut task);
            let mirror = &mut entry.mirror;
            mirror.status = task.status.clone();
            mirror.is_video = matches!(
                task.media_file.r#type(),
                Some(crate::media::MediaType::Video(_))
            );
            mirror.factor = task.size_factor;
            mirror.output_size = task.output_size;
            mirror.output_path = task.get_output().cloned();
            mirror.output_file_name = task
                .get_output()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            Some(out)
        })
    }

    /// UI 专属镜像字段（progress/elapsed/error）的唯一修改入口。
    /// 派生自 Transcoder 的镜像（status/factor/output_size）请走 `with_task`。
    /// 这两个方法是任务镜像的全部写入口，勿直接 `tasks.with_mut` 改镜像字段。
    pub fn touch_entry(&mut self, index: usize, f: impl FnOnce(&mut TaskMirror)) {
        self.tasks.with_mut(|list| {
            if let Some(entry) = list.get_mut(index) {
                f(&mut entry.mirror);
            }
        });
    }
    /// 修改设置；桌面 300ms 单飞去抖落盘，wasm 即时写 localStorage。
    ///
    /// 桌面原先逐键同步写：每键 = Settings 克隆 + toml 序列化 + create_dir_all +
    /// fs::write + rename，Windows+Defender 下 rename 可达数十 ms，直接落在
    /// 渲染帧上（路径输入框逐键触发）。去抖不牺牲原同步写的保序性质：写盘只
    /// 发生在去抖任务里，代数保证单写者，且单执行器上快照与写盘之间无 await
    /// ——不会交错出撕裂/旧值覆盖新值。去抖窗口内的尾写由 App 的 CloseRequested
    /// 兜底（wry handler 先于 dioxus 关窗处理执行）。
    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) {
        self.settings.with_mut(f);

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.save_pending.set(true);
            let epoch = self.save_epoch.cloned() + 1;
            self.save_epoch.set(epoch);
            let mut ctx = *self;
            // spawn_forever：去抖任务从设置面板/行内控件的作用域触发，组件在
            // 300ms 窗口内卸载（关面板）会取消 spawn 的任务 → 保存静默丢失
            spawn_forever(async move {
                crate::timers::sleep(std::time::Duration::from_millis(300)).await;
                // 期间又有修改 → 新任务（更高代数）负责写盘，本任务退场
                if ctx.save_epoch.cloned() == epoch {
                    ctx.flush_settings();
                }
            });
        }

        // wasm：localStorage 同步写 ~1KB，无窗口生命周期钩子语义，保持即时写
        #[cfg(target_arch = "wasm32")]
        self.flush_settings();
    }

    /// 落盘当前设置快照。成功清 dirty（保留失败态：关窗兜底会再试一次）。
    pub fn flush_settings(&mut self) {
        let snapshot = self.settings.cloned();
        let saved = config::save(&snapshot);
        #[cfg(not(target_arch = "wasm32"))]
        if saved.is_ok() {
            self.save_pending.set(false);
        }
        if let Err(e) = saved {
            log::error!("Failed to save settings: {e}");
            // wasm 无日志后端、release 桌面无控制台——只写 log 时设置静默丢失。
            // 逐键触发下按文案去重：持续失败（磁盘满/存储禁用）不刷屏，
            // 通知退场后下次保存尝试仍会重新提示。
            let text = format!("Failed to save settings: {e}");
            if !self.toasts.cloned().iter().any(|t| t.text == text) {
                self.push_toast(ToastKind::Error, text);
            }
        }
    }

    /// 推送一条通知：停留 3.6 秒 → 0.4 秒渐出 → 移除。返回通知 id。
    pub fn push_toast(&mut self, kind: ToastKind, text: impl Into<String>) -> u64 {
        self.push_toast_inner(kind, text, false)
    }

    /// `undo = true` 的通知带"Undo"按钮，停留更久（3.6s 对"我点错了"太短）。
    fn push_toast_inner(&mut self, kind: ToastKind, text: impl Into<String>, undo: bool) -> u64 {
        let toast = Toast {
            id: TOAST_ID.fetch_add(1, Ordering::Relaxed),
            kind,
            text: text.into(),
            leaving: false,
            undo,
        };
        let id = toast.id;
        self.toasts.push(toast);
        if undo {
            // 撤销槽只有一个：新删除顶掉旧的，旧通知上的 Undo 就成了假按钮
            self.toasts
                .with_mut(|list| list.iter_mut().for_each(|t| t.undo = t.id == id));
        }
        let mut toasts = self.toasts;
        let hold = if undo { 6000 } else { 3600 };
        // spawn_forever 而非 spawn：后者绑定"当前 scope"（= 点 ✕ 的 TaskRow），
        // 行随删除卸载会连带取消计时任务 → toast 永不移除（实测 toast 3 armed 后
        // 102s 无 leaving）。计时器是全局一次性工作，挂 ROOT 存活到应用退出。
        spawn_forever(async move {
            crate::timers::sleep(std::time::Duration::from_millis(hold)).await;
            toasts.with_mut(|list| {
                if let Some(t) = list.iter_mut().find(|t| t.id == id) {
                    t.leaving = true;
                }
            });
            crate::timers::sleep(std::time::Duration::from_millis(TOAST_FADE_MS)).await;
            toasts.with_mut(|list| list.retain(|t| t.id != id));
        });
        id
    }

    /// 添加文件，仅保留受支持的扩展名。每个新任务在后台线程探测时长/编码。
    /// 桌面专属（web 走 `add_file_bytes`：浏览器只给字节，没有路径）。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn add_files(&mut self, files: Vec<PathBuf>) {
        let mut added = Vec::new();
        let mut skipped = Vec::new();
        self.tasks.with_mut(|list| {
            for path in files {
                let ext = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|s| s.to_ascii_lowercase());
                match ext.as_deref() {
                    Some(ext) if crate::media::is_supported(ext) => {
                        list.push(TaskEntry::new(MediaFile::new(&path)));
                        added.push(list.len() - 1);
                    }
                    _ => {
                        log::warn!("Skipped unsupported file: {}", path.display());
                        skipped.push(path.display().to_string());
                    }
                }
            }
        });
        self.report_skipped(skipped);
        for index in added {
            self.spawn_probe(index);
        }
    }

    /// 网页端：前端文件字节直接入队（扩展名过滤与 add_files 一致）。
    #[cfg(target_arch = "wasm32")]
    pub fn add_file_bytes(&mut self, name: String, data: Vec<u8>) {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        if !ext.as_deref().is_some_and(crate::media::is_supported) {
            log::warn!("Skipped unsupported file: {name}");
            self.report_skipped(vec![name]);
            return;
        };
        let index = self.tasks.with_mut(|list| {
            list.push(TaskEntry::new(MediaFile::from_bytes(data, name)));
            list.len() - 1
        });
        self.spawn_probe(index);
    }

    /// 被静默丢弃的文件必须让用户看见（拖入 10 个只进 8 个时，日志没人看）。
    /// 多个时只报数量 + 首例，避免刷屏。
    fn report_skipped(&mut self, skipped: Vec<String>) {
        match skipped.as_slice() {
            [] => {}
            [one] => {
                self.push_toast(ToastKind::Warn, format!("Skipped unsupported file: {one}"));
            }
            many => {
                self.push_toast(
                    ToastKind::Warn,
                    format!(
                        "Skipped {} unsupported files (e.g. {})",
                        many.len(),
                        many[0]
                    ),
                );
            }
        }
    }

    /// 后台探测第 index 个任务（check_input），完成后回写状态与时长。
    fn spawn_probe(&mut self, index: usize) {
        let Some(entry) = self.tasks.cloned().get(index).cloned() else {
            return;
        };
        let task_arc = Arc::clone(&entry.transcoder);
        let mut ctx = *self;
        spawn(async move {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let awaited = crate::runner::run_blocking(move || {
                    let mut t = task_arc.lock().map_err(|e| e.to_string())?;
                    t.probe()
                })
                .await;
                // 扁平化 JoinError 包装（与 runner 同一模式）
                let result = match awaited {
                    Ok(inner) => inner,
                    Err(e) => Err(format!("join error: {e}")),
                };
                Self::write_back_probe(&mut ctx, index, &entry, result);
            }
            #[cfg(target_arch = "wasm32")]
            {
                // web：probe() 恒 Ok；时长/APNG 由 JS 原生元数据回填
                let (name, data) = {
                    let Ok(t) = task_arc.lock() else {
                        // 锁中毒直接 return 会让任务永驻 Probing：
                        // run_all 的等待循环空转、start_run 永久拒绝开跑。
                        // 回写 Alert 落地（与第二处锁分支/桌面 join 失败一致）。
                        Self::write_back_probe(
                            &mut ctx,
                            index,
                            &entry,
                            Err("transcoder lock poisoned".to_string()),
                        );
                        return;
                    };
                    match (t.media_file.display_name(), t.media_file.bytes()) {
                        (name, Some(bytes)) => (name, bytes.to_vec()),
                        (name, None) => (name, Vec::new()),
                    }
                };
                let probe = crate::transcoder::web::native_probe(&name, &data).await;
                let result = (|| {
                    let Ok(mut t) = task_arc.lock() else {
                        return Err("transcoder lock poisoned".to_string());
                    };
                    t.media_file.set_duration(probe.duration);
                    // 扩展名 png 实为动画 PNG → 纠正为 APNG（走 ffmpeg-wasm 引擎）
                    if probe.apng {
                        t.media_file.set_type(crate::media::MediaType::Video(
                            crate::media::VideoType::Apng,
                        ));
                    }
                    t.probe()
                })();
                Self::write_back_probe(&mut ctx, index, &entry, result);
            }
        });
    }

    /// 探测结果回写（状态翻转 + 错误镜像），含 Arc::ptr_eq 任务校验。
    fn write_back_probe(
        ctx: &mut UiState,
        index: usize,
        entry: &TaskEntry,
        result: Result<(), String>,
    ) {
        // 先校验仍是同一任务（队列可能已被清空/重排），再写入：
        // probe 不写 Transcoder.status，需在闭包内赋值由 with_task 同步镜像
        // （含 probe 纠正类型后的 is_video，避免用错大小上限/预览渲染元素）。
        if !ctx
            .tasks
            .cloned()
            .get(index)
            .is_some_and(|e| Arc::ptr_eq(&e.transcoder, &entry.transcoder))
        {
            return;
        }
        // Run 已接管该任务（镜像 = Processing）则止步：with_task 会在主线程
        // 撞 worker 持有的锁（UI 冻结），且无条件写 Pending 会抹掉运行中状态。
        if matches!(
            ctx.tasks.cloned().get(index),
            Some(e) if e.mirror.status == Status::Processing
        ) {
            return;
        }
        let status = match &result {
            Ok(()) => Status::Pending,
            Err(_) => Status::Alert,
        };
        ctx.with_task(index, |t| t.status = status);
        if let Err(err) = &result {
            ctx.touch_entry(index, |e| e.error = Some(err.clone()));
        }
    }

    /// 一键下载全部 Done 任务的产物（web：字节驻内存，顺序触发浏览器下载，
    /// 400ms 间隔防多文件拦截）。只预收集 Arc+文件名（O(1)/项），字节在循环内
    /// 逐个锁取——先全量克隆再下载是 N×512KB 的双份峰值（产物本就驻内存）。
    #[cfg(target_arch = "wasm32")]
    pub fn download_all(&mut self) {
        let items: Vec<(std::sync::Arc<std::sync::Mutex<crate::transcoder::Transcoder>>, String)> =
            self.tasks
                .cloned()
                .iter()
                .filter(|e| e.mirror.status == Status::Done)
                .map(|e| {
                    let name = e
                        .mirror
                        .output_file_name
                        .clone()
                        .unwrap_or_else(|| "sticker.webm".into());
                    (std::sync::Arc::clone(&e.transcoder), name)
                })
                .collect();
        let mut ctx = *self;
        spawn(async move {
            let mut failed = 0usize;
            for (transcoder, name) in items {
                // 锁内克隆单项字节、下载完立即释放，不跨 await 持锁（wasm 单线程）
                let ok = {
                    let bytes = transcoder.lock().ok().and_then(|t| t.output_bytes.clone());
                    bytes
                        .as_deref()
                        .is_some_and(|b| crate::transcoder::web::sticker_download(b, &name).is_ok())
                };
                if !ok {
                    log::error!("download failed: glue missing or output gone");
                    failed += 1;
                }
                crate::timers::sleep(std::time::Duration::from_millis(400)).await;
            }
            // wasm 无日志后端：只写 log 等于点了按钮没反应（行内单文件下载
            // 同因早已改 toast）。汇总一条，避免 N 个失败叠 N 条通知。
            if failed > 0 {
                ctx.push_toast(
                    ToastKind::Error,
                    format!("Download failed for {failed} file(s)"),
                );
            }
        });
    }
    /// 重试单个任务：Alert/SizeExcess → Pending（保留用户系数，重跑由 Run 触发）。
    pub fn retry_task(&mut self, index: usize) {
        self.with_task(index, |t| t.status = Status::Pending);
    }

    /// 移除单个任务（任意状态）。删除一律留撤销槽——误点 ✕ 与按 Delete 同一个口。
    pub fn remove_task(&mut self, index: usize) {
        let removed = self
            .tasks
            .with_mut(|list| (index < list.len()).then(|| (index, list.remove(index))));
        let Some((index, entry)) = removed else {
            return;
        };
        let name = entry.mirror.input_path.clone();
        let id = self.push_toast_inner(ToastKind::Info, format!("Removed {name}"), true);
        self.undo.set(Some(UndoSlot {
            toast_id: id,
            removed: vec![(index, entry)],
        }));
    }

    /// 移除全部已完成（Done）任务。一次删一批，同样可撤销。
    pub fn clear_done(&mut self) {
        let mut removed = Vec::new();
        self.tasks.with_mut(|list| {
            // retain 会压缩下标，撤销要按原位回插——收集走 split_done（原下标记账）
            let (kept, rem) = split_done(std::mem::take(list));
            *list = kept;
            removed = rem;
        });
        if removed.is_empty() {
            return;
        }
        let id = self.push_toast_inner(
            ToastKind::Info,
            format!("Removed {} finished task(s)", removed.len()),
            true,
        );
        self.undo.set(Some(UndoSlot {
            toast_id: id,
            removed,
        }));
    }

    /// 撤销最近一次删除。按钮只挂在对应通知上，故必须核对通知 id：
    /// 3.6 秒内删了两次时，旧通知上的撤销不能把新删的任务放回去。
    pub fn undo_remove(&mut self, toast_id: u64) {
        // 运行中回插会移动队列下标，而 runner 持有的是开跑时的 index——
        // 状态/进度/大小会写到别的行，被移位的 Processing 行还会永驻僵尸态
        // （start_run 的 Processing 门从此拒绝开跑）。保留槽位，跑完仍可撤销。
        if self.running.cloned() {
            return;
        }
        let Some(slot) = self.undo.cloned() else {
            return;
        };
        if slot.toast_id != toast_id {
            return;
        }
        self.undo.set(None);
        // removed 按原下标升序，逐个回插即还原原序
        self.tasks
            .with_mut(|list| reinsert_removed(list, slot.removed));
        // 撤销后这条通知立刻退场（它的定时器随后找不到 id，自然空转）
        self.toasts
            .with_mut(|list| list.retain(|t| t.id != toast_id));
    }

    /// Ctrl+Enter：运行中 → 取消；否则开跑（与工具栏 Run/Cancel 同一状态机）。
    pub fn start_run_or_cancel(&mut self) {
        if self.running.cloned() {
            self.cancel.set(true);
        } else {
            self.start_run();
        }
    }

    /// Ctrl+O：原生多选框，入队口仍是 `add_files`（与 Add File 按钮同一条路）。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn pick_input_files(&mut self) {
        let mut ctx = *self;
        spawn(async move {
            let exts = crate::media::dotted_exts();
            let picked = rfd::AsyncFileDialog::new()
                .add_filter("Supported media", &exts)
                .pick_files()
                .await;
            if let Some(files) = picked {
                let paths: Vec<PathBuf> =
                    files.into_iter().map(|f| f.path().to_path_buf()).collect();
                ctx.add_files(paths);
            }
        });
    }

    /// 点击 Run：建输出目录后启动异步转码循环。
    /// 已完成（Done）的任务不重跑——要重转先点行内 Re-run 退回 Pending。
    pub fn start_run(&mut self) {
        if self.running.cloned() || self.tasks.cloned().is_empty() {
            return;
        }
        if !self
            .tasks
            .cloned()
            .iter()
            .any(|task| task.mirror.status != Status::Done)
        {
            self.push_toast(
                ToastKind::Info,
                "Nothing to run — every task is done (use Re-run to redo one)",
            );
            return;
        }
        if self
            .tasks
            .cloned()
            .iter()
            .any(|task| matches!(task.mirror.status, Status::Probing | Status::Processing))
        {
            self.push_toast(ToastKind::Info, "Waiting for probing to finish");
            return;
        }
        // wasm：无输出目录概念，跳过创建与校验
        #[cfg(not(target_arch = "wasm32"))]
        {
            let settings = self.settings.peek().clone();
            let output_dir = PathBuf::from(&settings.output_dir);
            if !output_dir.exists()
                && let Err(e) = std::fs::create_dir(&output_dir)
            {
                log::error!("Failed to create output directory: {:?}", e);
                self.push_toast(
                    ToastKind::Error,
                    format!("Failed to create output directory: {e}"),
                );
                return;
            }
            // 只读目录 / ACL 拒绝都能通过"存在"校验，但每个任务都会在写盘时失败。
            // 开跑前写一次就能立刻说清楚，不必让用户等一轮全红。
            if !settings.output_dir_writable() {
                self.push_toast(
                    ToastKind::Error,
                    format!("Output folder is not writable: {}", settings.output_dir),
                );
                return;
            }
        }
        self.overall_progress.set(0.0);
        self.cancel.set(false);
        self.running.set(true);

        // 实时进度通道：worker 发送 → 接收端按 ~10Hz 节流合并后写显示镜像
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::runner::ProgressUpdate>();
        let tasks = self.tasks;
        let mut progress = self.progress;
        spawn(async move {
            use std::collections::HashMap;

            const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

            let mut pending: HashMap<usize, f32> = HashMap::new();
            let mut flush = |pending: &mut HashMap<usize, f32>| {
                if pending.is_empty() {
                    return;
                }
                let latest: Vec<_> = pending.drain().collect();
                for (index, pct) in latest {
                    // 仅 Processing 期间写进度：worker 退出后残留的 100% 更新
                    // 不得复活 runner 已清掉的进度条（否则 Done 行显示进度
                    // 而非文件大小）。写独立信号、不碰 tasks——见 UiState::progress。
                    let processing = tasks
                        .peek()
                        .get(index)
                        .is_some_and(|e| matches!(e.mirror.status, Status::Processing));
                    if processing {
                        progress.set(Some((index, pct)));
                    }
                }
            };

            #[cfg(not(target_arch = "wasm32"))]
            {
                use tokio::time::MissedTickBehavior;
                let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    let closed = tokio::select! {
                        _ = ticker.tick() => false,
                        msg = progress_rx.recv() => match msg {
                            Some(update) => {
                                pending.insert(update.index, update.pct.clamp(0.0, 1.0));
                                continue;
                            }
                            None => true,
                        }
                    };
                    flush(&mut pending);
                    if closed {
                        break;
                    }
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                // wasm：tokio time/select 不可用——轮询收件箱 + setTimeout 步进
                loop {
                    crate::timers::sleep(FLUSH_INTERVAL).await;
                    while let Ok(update) = progress_rx.try_recv() {
                        pending.insert(update.index, update.pct.clamp(0.0, 1.0));
                    }
                    let closed = progress_rx.is_closed();
                    flush(&mut pending);
                    if closed {
                        break;
                    }
                }
            }
        });

        spawn(crate::runner::run_all(*self, progress_tx));
    }

    /// 打开输出目录（桌面）。失败弹 toast 而不是只写日志——web 端没有日志后端，
    /// 而桌面端用户也不看终端。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_output_dir(&mut self) {
        let dir = self.settings.peek().output_dir.clone();
        if let Err(e) = std::process::Command::new("explorer").arg(&dir).spawn() {
            log::error!("failed to open output dir '{dir}': {e}");
            self.push_toast(
                ToastKind::Error,
                format!("Failed to open output folder: {e}"),
            );
        }
    }

    /// 选择输出目录对话框（HTML 无目录选择器，沿用 rfd）。
    /// 桌面专属：web 无文件系统，工具栏/设置面板在 wasm 下根本不渲染该行。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn pick_output_dir(&mut self) {
        let mut ctx = *self;
        spawn(async move {
            if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                let path = folder.path().to_string_lossy().into_owned();
                ctx.update_settings(move |s| s.output_dir = path);
            }
        });
    }
}

/// 窗口级原生监听桥（只装一次）：弹窗焦点陷阱 + 全局快捷键。
///
/// 陷阱用原生 Tab 循环手写，而不是把背景设 `inert`：背景内容散在 .app 的多个
/// 兄弟节点上，得为它加一层 `display: contents` 包装，而 inert 用在这里还要求
/// 元素 omit 属性（假值也照样生效），dioxus 的自定义属性没有这个语义。
///
/// 快捷键挂在 window 上而不是 DOM 上：点掉按钮/关掉弹窗后焦点落到 body，事件
/// 就不再经过 .app；原生监听顺带能**同步** preventDefault（dioxus 事件管线的
/// prevent_default 是异步的，拦不住浏览器默认动作）。web 端不接管 Ctrl+O：
/// 浏览器自身的"打开文件"拦不住，两条路径同时弹框更糟，而浏览器那个行为本身
/// 可接受。末尾永不 resolve —— eval 通道在 JS 代码返回后会被关闭，留着挂起的
/// Promise 才能一直 `dioxus.send` 回来。
///
/// （弹窗内滚轮不做转发：滚动容器是弹窗自身，见 app.css 的 `.modal-settings`。）
fn native_bridge_js() -> String {
    #[cfg(target_arch = "wasm32")]
    let open_branch = "";
    #[cfg(not(target_arch = "wasm32"))]
    let open_branch =
        "else if (e.key === 'o' || e.key === 'O') { e.preventDefault(); dioxus.send('open'); }";
    format!(
        r#"
if (!window.__stp_keys) {{
    window.__stp_keys = true;
    window.addEventListener('keydown', (e) => {{
        // 弹窗打开时 Tab 在弹窗内循环（初始焦点在 backdrop 上，第一次 Tab 进正文）
        if (e.key === 'Tab') {{
            const modal = document.querySelector('.modal');
            if (modal) {{
                const stops = [...modal.querySelectorAll('button, input, select, textarea, a[href], [tabindex]')]
                    .filter((el) => el.tabIndex >= 0 && !el.disabled && el.offsetParent !== null);
                if (!stops.length) {{ e.preventDefault(); return; }}
                const first = stops[0], last = stops[stops.length - 1], active = document.activeElement;
                const inside = modal.contains(active);
                if (e.shiftKey) {{
                    if (!inside || active === first) {{ e.preventDefault(); last.focus(); }}
                }} else if (!inside || active === last) {{
                    e.preventDefault(); first.focus();
                }}
            }}
        }}
        if (!e.ctrlKey || e.altKey || e.metaKey || e.repeat) return;
        if (e.key === 'Enter') {{ e.preventDefault(); dioxus.send('run'); }}
        {open_branch}
    }}, true);
    dioxus.send('ready');
}}
await new Promise(() => {{}});
"#
    )
}

#[cfg(test)]
mod tests {
    use super::{TaskEntry, Status, reinsert_removed, split_done};
    use crate::media::MediaFile;
    use std::path::Path;

    /// 路径即身份（mirror.input_path 就是 display_name）。
    fn entries(names: &[&str]) -> Vec<TaskEntry> {
        names
            .iter()
            .map(|n| TaskEntry::new(MediaFile::new(Path::new(n))))
            .collect()
    }

    fn names(list: &[TaskEntry]) -> Vec<String> {
        list.iter().map(|e| e.mirror.input_path.clone()).collect()
    }

    #[test]
    fn undo_reinsert_restores_original_order() {
        // 收集走 clear_done 的同一份实现（split_done），回插走 reinsert_removed：
        // 下标记账两端都是真件——clear_done 改坏收集/下标，此测试必须红
        let mut list = entries(&["a.mp4", "b.mp4", "c.mp4", "d.mp4", "e.mp4"]);
        list[1].mirror.status = Status::Done;
        list[3].mirror.status = Status::Done;
        let (mut kept, removed) = split_done(list);
        assert_eq!(names(&kept), ["a.mp4", "c.mp4", "e.mp4"]);
        // 撤销槽记的是**原**下标
        assert_eq!(
            removed.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [1, 3]
        );
        reinsert_removed(&mut kept, removed);
        assert_eq!(names(&kept), ["a.mp4", "b.mp4", "c.mp4", "d.mp4", "e.mp4"]);
    }

    #[test]
    fn undo_reinsert_clamps_stale_index() {
        let mut list = entries(&["a.mp4"]);
        // 槽里的下标来自更长的队列（撤销前又删过任务）——贴到末尾，不 panic
        reinsert_removed(
            &mut list,
            vec![(7, TaskEntry::new(MediaFile::new(Path::new("x.mp4"))))],
        );
        assert_eq!(names(&list), ["a.mp4", "x.mp4"]);
    }
}

#[component]
pub fn App() -> Element {
    let ctx = UiState {
        tasks: use_signal(Vec::new),
        settings: use_signal(config::load),
        show_settings: use_signal(|| false),
        show_preview: use_signal(|| None),
        show_compat: use_signal(|| false),
        running: use_signal(|| false),
        overall_progress: use_signal(|| 0.0f32),
        progress: use_signal(|| None),
        #[cfg(not(target_arch = "wasm32"))]
        save_pending: use_signal(|| false),
        #[cfg(not(target_arch = "wasm32"))]
        save_epoch: use_signal(|| 0u64),
        cancel: use_signal(|| false),
        toasts: use_signal(Vec::new),
        undo: use_signal(|| None),
    };

    // 主题属性挂到 <html>，CSS 变量按 [data-theme="dark"] 覆盖；随设置持久化。
    // "system" 档：读取 prefers-color-scheme 并监听系统切换实时回写。
    // 派生信号：settings 其它字段改动（如目录逐键写入）不重跑本 effect，
    // 只有 theme 变化才重新注入。
    let theme = use_memo(move || ctx.settings.read().theme.clone());
    use_effect(move || {
        let theme = theme.cloned();
        if theme == "system" {
            document::eval(
                "(() => {
                    const mq = window.matchMedia('(prefers-color-scheme: dark)');
                    const apply = () => document.documentElement.setAttribute('data-theme', mq.matches ? 'dark' : 'light');
                    window.__stp_theme_handler && mq.removeEventListener('change', window.__stp_theme_handler);
                    window.__stp_theme_handler = apply;
                    mq.addEventListener('change', apply);
                    apply();
                })()",
            );
        } else {
            // 非 system 档只认 dark/light（未知值按 light 渲染，不进插值）
            let attr = if theme == "dark" { "dark" } else { "light" };
            document::eval(&format!(
                "window.__stp_theme_handler && (() => {{
                    window.matchMedia('(prefers-color-scheme: dark)')
                        .removeEventListener('change', window.__stp_theme_handler);
                    window.__stp_theme_handler = null;
                }})();
                document.documentElement.setAttribute('data-theme', '{attr}')"
            ));
        }
    });

    use_context_provider(move || ctx);

    // 设置去抖的关窗兜底：300ms 窗口内改完就关会丢尾写。wry handler 在
    // app.tick() 里执行，先于 dioxus 的 CloseRequested 处理——这里是最后时机。
    // 只读 save_pending，无待写时连磁盘都不碰。
    #[cfg(not(target_arch = "wasm32"))]
    let _save_on_close = dioxus::desktop::use_wry_event_handler(move |event, _| {
        use dioxus::desktop::WindowEvent;
        use dioxus::desktop::tao::event::Event;
        if matches!(
            event,
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
        ) && ctx.save_pending.cloned()
        {
            let mut writer = ctx;
            writer.flush_settings();
        }
    });

    // 全局快捷键走 window 级原生监听 + eval 通道回传，而不是挂在 DOM 上：
    // 点掉一个按钮/关掉弹窗后焦点会落到 body，事件就不再经过 .app，挂根的
    // 方案会静默失效；原生监听还顺带能**同步** preventDefault（dioxus 事件
    // 管线的 prevent_default 是异步的，拦不住浏览器默认动作）。
    use_effect(move || {
        let mut ctx = ctx;
        let mut keys = document::eval(&native_bridge_js());
        spawn(async move {
            loop {
                let msg = match keys.recv::<String>().await {
                    Ok(msg) => msg,
                    Err(e) => {
                        log::error!("key bridge closed: {e}");
                        break;
                    }
                };
                // 弹窗打开时不代理快捷键（此刻 Run 按钮也是禁用的）
                if ctx.show_settings.cloned()
                    || ctx.show_preview.cloned().is_some()
                    || ctx.show_compat.cloned()
                {
                    continue;
                }
                match msg.as_str() {
                    "run" => ctx.start_run_or_cancel(),
                    // 装没装上只能靠这一行确认：桥断了快捷键就是静默失效
                    "ready" => log::info!("key bridge installed"),
                    #[cfg(not(target_arch = "wasm32"))]
                    "open" => ctx.pick_input_files(),
                    _ => {}
                }
            }
        });
    });

    rsx! {
        // 浏览器标签页 + 分享卡片（OG）：资源路径全部相对页面 base（同 glue
        // 注入套路；favicon.png/og-image.png 随 assets 手动 cp 到 web 根）。
        document::Title { "Sticker Process" }
        document::Meta { name: "description", content: "Convert images and videos into Telegram stickers — right in your browser, nothing uploaded." }
        document::Meta { property: "og:title", content: "Sticker Process" }
        document::Meta { property: "og:description", content: "Image/video → Telegram sticker converter running fully client-side (WASM)." }
        document::Meta { property: "og:type", content: "website" }
        document::Meta { property: "og:image", content: "og-image.png" }
        document::Link { rel: "icon", r#type: "image/png", href: "favicon.png" }
        style { {include_str!("app.css")} }
        DropZone {
            Toolbar {}
            TaskList {}
            crate::components::summary::SummaryBar {}
            ProgressBar {}
            SettingsPanel {}
            crate::components::compat::CompatModal {}
            crate::components::preview::PreviewModal {}
            crate::components::toast::ToastContainer {}
        }
    }
}
