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
use dioxus::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const VIDEO: [&str; 3] = ["mp4", "gif", "apng"];
pub const IMAGE: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// 全部受支持扩展名（编译期从 VIDEO/IMAGE 拼装，保证不漂移）。
pub const SUPPORTED: [&str; 7] = {
    let mut all = [""; 7];
    let mut i = 0;
    let mut j = 0;
    while j < VIDEO.len() {
        all[i] = VIDEO[j];
        i += 1;
        j += 1;
    }
    let mut k = 0;
    while k < IMAGE.len() {
        all[i] = IMAGE[k];
        i += 1;
        k += 1;
    }
    all
};

static TOAST_ID: AtomicU64 = AtomicU64::new(1);

/// 队列中的一个任务：共享 Transcoder（逻辑层）+ 显示镜像（渲染层）。
#[derive(Clone, Debug)]
pub struct TaskEntry {
    pub transcoder: Arc<Mutex<Transcoder>>,
    /// 输入文件路径（创建时快照，避免渲染期加锁）。
    pub input_path: String,
    /// Phase D 预览将展示输入大小对比。
    pub input_size: u64,
    pub is_video: bool,
    // ---- 显示镜像：由修改 Transcoder 的一方负责同步 ----
    pub status: Status,
    pub output_size: Option<u64>,
    /// 输出文件名（不含路径，随 output_path 同步），任务行直接展示。
    pub output_file_name: Option<String>,
    pub factor: Option<f64>,
    /// 输出文件路径镜像（set_output_dir 时同步），供预览免锁读取。
    pub output_path: Option<PathBuf>,
    /// 当前尝试的转码进度 0..=1（仅 Processing 期间有值）。
    pub progress: Option<f32>,
    /// 最近一次成功转码的耗时。
    pub elapsed_ms: Option<u64>,
    /// 最近一次失败的错误详情（悬停工具提示展示）。
    pub error: Option<String>,
}

// 组件 Props 派生需要 PartialEq；共享的 Transcoder 不参与比较（比较显示镜像即可）
impl PartialEq for TaskEntry {
    fn eq(&self, other: &Self) -> bool {
        self.input_path == other.input_path
            && self.is_video == other.is_video
            && self.factor == other.factor
            && self.output_file_name == other.output_file_name
            && self.output_size == other.output_size
            && self.progress == other.progress
            && self.elapsed_ms == other.elapsed_ms
            && self.output_path == other.output_path
            && self.status == other.status
            && self.error == other.error
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
            input_path,
            input_size,
            is_video,
            status: Status::Probing,
            output_size: None,
            output_file_name: None,
            output_path: None,
            factor: None,
            progress: None,
            elapsed_ms: None,
            error: None,
        }
    }

    /// 输出大小相对上限的倍率（>1.0 即超限），无输出时返回 None。
    pub fn size_excess_ratio(&self, video_limit: u64, image_limit: u64) -> Option<f64> {
        let size = self.output_size? as f64;
        Some(
            size / if self.is_video {
                video_limit
            } else {
                image_limit
            } as f64,
        )
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
    pub running: Signal<bool>,
    pub overall_progress: Signal<f32>,
    /// 取消标记：runner 在每次尝试前检查；运行中的任务经 cancel_flag 中断 ffmpeg。
    pub cancel: Signal<bool>,
    pub toasts: Signal<Vec<Toast>>,
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
            entry.status = task.status.clone();
            entry.is_video = matches!(
                task.media_file.r#type(),
                Some(crate::media::MediaType::Video(_))
            );
            entry.factor = task.size_factor.as_ref().map(|f| f.get());
            entry.output_size = task.output_size.as_ref().map(|s| s.size);
            entry.output_path = task.get_output().cloned();
            entry.output_file_name = task
                .get_output()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            Some(out)
        })
    }

    /// UI 专属镜像字段（progress/elapsed/error）的唯一修改入口。
    /// 派生自 Transcoder 的镜像（status/factor/output_size）请走 `with_task`。
    /// 这两个方法是任务镜像的全部写入口，勿直接 `tasks.with_mut` 改镜像字段。
    pub fn touch_entry(&mut self, index: usize, f: impl FnOnce(&mut TaskEntry)) {
        self.tasks.with_mut(|list| {
            if let Some(entry) = list.get_mut(index) {
                f(entry);
            }
        });
    }
    /// 修改设置并即时同步落盘（settings.toml 仅 ~200B，写盘亚毫秒级；
    /// 同步执行杜绝并发写撕裂/乱序——异步写完成后旧值可能覆盖新值）。
    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) {
        self.settings.with_mut(f);
        let snapshot = self.settings.cloned();
        if let Err(e) = config::save(&snapshot) {
            log::error!("Failed to save settings: {e}");
        }
    }

    /// 推送一条通知：停留 3.6 秒 → 0.4 秒渐出 → 移除。
    pub fn push_toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        let toast = Toast {
            id: TOAST_ID.fetch_add(1, Ordering::Relaxed),
            kind,
            text: text.into(),
            leaving: false,
        };
        let id = toast.id;
        self.toasts.push(toast);
        let mut toasts = self.toasts;
        spawn(async move {
            crate::timers::sleep(std::time::Duration::from_millis(3600)).await;
            toasts.with_mut(|list| {
                if let Some(t) = list.iter_mut().find(|t| t.id == id) {
                    t.leaving = true;
                }
            });
            crate::timers::sleep(std::time::Duration::from_millis(400)).await;
            toasts.with_mut(|list| list.retain(|t| t.id != id));
        });
    }

    /// 添加文件，仅保留受支持的扩展名。每个新任务在后台线程探测时长/编码。
    pub fn add_files(&mut self, files: Vec<PathBuf>) {
        let mut added = Vec::new();
        self.tasks.with_mut(|list| {
            for path in files {
                let ext = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|s| s.to_ascii_lowercase());
                match ext.as_deref() {
                    Some(ext) if SUPPORTED.contains(&ext) => {
                        list.push(TaskEntry::new(MediaFile::new(&path)));
                        added.push(list.len() - 1);
                    }
                    _ => log::warn!("Skipped unsupported file: {}", path.display()),
                }
            }
        });
        for index in added {
            self.spawn_probe(index);
        }
    }

    /// 网页端：前端文件字节直接入队（扩展名过滤与 add_files 一致）。
    pub fn add_file_bytes(&mut self, name: String, data: Vec<u8>) {
        let ext = name.rsplit('.').next().map(|s| s.to_ascii_lowercase());
        if !ext.as_deref().is_some_and(|e| SUPPORTED.contains(&e)) {
            log::warn!("Skipped unsupported file: {name}");
            return;
        };
        let mut added = Vec::new();
        self.tasks.with_mut(|list| {
            list.push(TaskEntry::new(MediaFile::from_bytes(data, name)));
            added.push(list.len() - 1);
        });
        for index in added {
            self.spawn_probe(index);
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
                    t.probe().map_err(|e| e.to_string())
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
                // web：probe() 恒 Ok；时长由 JS 元数据（ImageDecoder）回填
                let (name, data) = {
                    let Ok(t) = task_arc.lock() else {
                        return;
                    };
                    match (t.media_file.display_name(), t.media_file.bytes()) {
                        (name, Some(bytes)) => (name, bytes.to_vec()),
                        (name, None) => (name, Vec::new()),
                    }
                };
                let duration = crate::transcoder::web::probe_duration(&name, &data).await;
                let result = (|| {
                    let Ok(mut t) = task_arc.lock() else {
                        return Err("transcoder lock poisoned".to_string());
                    };
                    t.media_file.set_duration(duration);
                    t.probe().map_err(|_: ()| "probe failed".to_string())
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
        let status = match &result {
            Ok(()) => Status::Pending,
            Err(_) => Status::Alert,
        };
        ctx.with_task(index, |t| t.status = status);
        if let Err(err) = &result {
            ctx.touch_entry(index, |e| e.error = Some(err.clone()));
        }
    }

    /// 移除已完成（Done）的任务。
    pub fn clear_done(&mut self) {
        self.tasks
            .with_mut(|list| list.retain(|task| task.status != Status::Done));
    }

    /// 重试单个任务：Alert/SizeExcess → Pending（保留用户系数，重跑由 Run 触发）。
    pub fn retry_task(&mut self, index: usize) {
        self.with_task(index, |t| t.status = Status::Pending);
    }

    /// 移除单个任务（任意状态）。
    pub fn remove_task(&mut self, index: usize) {
        self.tasks.with_mut(|list| {
            if index < list.len() {
                list.remove(index);
            }
        });
    }

    /// 点击 Run：建输出目录后启动异步转码循环。
    pub fn start_run(&mut self) {
        if self.running.cloned() || self.tasks.cloned().is_empty() {
            return;
        }
        if self
            .tasks
            .cloned()
            .iter()
            .any(|task| matches!(task.status, Status::Probing | Status::Processing))
        {
            self.push_toast(ToastKind::Info, "Waiting for probing to finish");
            return;
        }
        // wasm：无输出目录概念，跳过创建与校验
        #[cfg(not(target_arch = "wasm32"))]
        {
            let output_dir = PathBuf::from(self.settings.peek().output_dir.clone());
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
        }
        self.overall_progress.set(0.0);
        self.cancel.set(false);
        self.running.set(true);

        // 实时进度通道：worker 发送 → 接收端按 ~10Hz 节流合并后写显示镜像
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::runner::ProgressUpdate>();
        let mut tasks = self.tasks;
        spawn(async move {
            use std::collections::HashMap;

            const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

            let mut pending: HashMap<usize, f32> = HashMap::new();
            let mut flush = |pending: &mut HashMap<usize, f32>| {
                if pending.is_empty() {
                    return;
                }
                let latest: Vec<_> = pending.drain().collect();
                tasks.with_mut(|list| {
                    for (index, pct) in latest {
                        if let Some(entry) = list.get_mut(index) {
                            // 仅 Processing 期间写进度：worker 退出后残留的
                            // 100% 更新不得覆盖 runner 清掉的 progress（否则
                            // Done 任务显示进度条而非文件大小）
                            if matches!(entry.status, Status::Processing) {
                                entry.progress = Some(pct);
                            }
                        }
                    }
                });
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

    /// 选择输出目录对话框（HTML 无目录选择器，沿用 rfd）。
    /// wasm：无目录选择，函数体为空（下一阶段 showSaveFilePicker / 下载）。
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

    #[cfg(target_arch = "wasm32")]
    pub fn pick_output_dir(&mut self) {}
}

#[component]
pub fn App() -> Element {
    let ctx = UiState {
        tasks: use_signal(Vec::new),
        settings: use_signal(config::load),
        show_settings: use_signal(|| false),
        show_preview: use_signal(|| None),
        running: use_signal(|| false),
        overall_progress: use_signal(|| 0.0f32),
        cancel: use_signal(|| false),
        toasts: use_signal(Vec::new),
    };

    // 主题属性挂到 <html>，CSS 变量按 [data-theme="dark"] 覆盖；随设置持久化。
    // "system" 档：读取 prefers-color-scheme 并监听系统切换实时回写。
    use_effect(move || {
        let theme = ctx.settings.read().theme.clone();
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
            document::eval(&format!(
                "window.__stp_theme_handler && (() => {{
                    window.matchMedia('(prefers-color-scheme: dark)')
                        .removeEventListener('change', window.__stp_theme_handler);
                    window.__stp_theme_handler = null;
                }})();
                document.documentElement.setAttribute('data-theme', '{theme}')"
            ));
        }
    });

    use_context_provider(move || ctx);

    rsx! {
        style { {include_str!("app.css")} }
        DropZone {
            Toolbar {}
            TaskList {}
            ProgressBar {}
            SettingsPanel {}
            crate::components::preview::PreviewModal {}
            crate::components::toast::ToastContainer {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AGENTS 契约：eq 必须覆盖组件依赖的每个镜像字段——漏字段会 memo 跳过重渲染。
    #[test]
    fn task_entry_eq_covers_status_and_error() {
        let mut a = TaskEntry::new(MediaFile::new(&PathBuf::from("x.mp4")));
        let b = a.clone();
        assert_eq!(a, b);
        a.status = Status::Alert;
        a.error = Some("boom".into());
        assert_ne!(a, b);
    }
}
