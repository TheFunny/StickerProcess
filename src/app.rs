//! 根组件与全局信号状态。
//!
//! 任务仍以 `Arc<Mutex<Transcoder>>` 共享给后台 worker（共享模式不变），
//! 另存少量"显示镜像"字段，让 UI 渲染时**不加锁**（worker 转码期间会长期持有锁）。
//! Phase C 新增：实时进度/耗时/错误详情镜像、Toast、主题切换、添加时异步探测。

use crate::components::{
    drop_zone::DropZone, progress_bar::ProgressBar, task_list::TaskList, toast::Toast,
    toast::ToastKind, toolbar::Toolbar,
};
use crate::media::MediaFile;
use crate::transcoder::{Status, Transcoder};
use dioxus::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// AGENTS 约定保留：新增媒体类型时同步维护（rfd 过滤器在 Phase B 设置面板中复用）
#[allow(dead_code)]
pub const VIDEO: [&str; 3] = ["mp4", "gif", "apng"];
#[allow(dead_code)]
pub const IMAGE: [&str; 3] = ["jpg", "jpeg", "png"];
pub const SUPPORTED: [&str; 6] = ["mp4", "gif", "apng", "jpg", "jpeg", "png"];

static TOAST_ID: AtomicU64 = AtomicU64::new(1);

/// 队列中的一个任务：共享 Transcoder（逻辑层）+ 显示镜像（渲染层）。
#[derive(Clone)]
pub struct TaskEntry {
    pub transcoder: Arc<Mutex<Transcoder>>,
    /// 输入文件路径（创建时快照，避免渲染期加锁）。
    pub input_path: String,
    /// Phase D 预览将展示输入大小对比。
    #[allow(dead_code)]
    pub input_size: u64,
    pub input_duration: Option<f64>,
    pub is_video: bool,
    // ---- 显示镜像：由修改 Transcoder 的一方负责同步 ----
    pub status: Status,
    pub output_size: Option<u64>,
    pub factor: Option<f64>,
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
            && self.status == other.status
            && self.output_size == other.output_size
            && self.factor == other.factor
            && self.progress == other.progress
            && self.elapsed_ms == other.elapsed_ms
            && self.error == other.error
    }
}

impl TaskEntry {
    pub fn new(path: &PathBuf) -> Self {
        // Phase C：不再同步探测（拖入大目录不卡 UI），check_input 延后到后台线程
        let transcoder = Transcoder::new(MediaFile::new(path));
        let input_path = transcoder.media_file.path_str();
        let is_video = matches!(
            transcoder.media_file.r#type(),
            Some(crate::media::MediaType::Video(_))
        );
        log::info!("Added task: {input_path}");
        Self {
            transcoder: Arc::new(Mutex::new(transcoder)),
            input_path,
            input_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            input_duration: None,
            is_video,
            status: Status::Probing,
            output_size: None,
            factor: None,
            progress: None,
            elapsed_ms: None,
            error: None,
        }
    }

    /// 输出大小是否超限（>1.0 即超限），无输出时返回 None。
    pub fn size_excess_factor(&self) -> Option<f64> {
        const IMAGE_MAX_SIZE: f64 = (512 * 1024) as f64;
        const VIDEO_MAX_SIZE: f64 = (256 * 1024) as f64;
        let size = self.output_size? as f64;
        Some(
            size / if self.is_video {
                VIDEO_MAX_SIZE
            } else {
                IMAGE_MAX_SIZE
            },
        )
    }
}

/// 浅色/深色主题（持久化随 Phase B 设置落地，当前为会话内切换）。
#[derive(Clone, Copy, PartialEq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn attr(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::Light,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Theme::Light => "Dark",
            Theme::Dark => "Light",
        }
    }
}

/// 全部界面状态（signals）。`Signal<T>` 为 Copy，可按值传递。
#[derive(Clone, Copy)]
pub struct UiState {
    pub tasks: Signal<Vec<TaskEntry>>,
    pub output_dir: Signal<String>,
    pub max_retry: Signal<u8>,
    pub running: Signal<bool>,
    pub overall_progress: Signal<f32>,
    pub cancel: Signal<bool>,
    pub toasts: Signal<Vec<Toast>>,
    pub theme: Signal<Theme>,
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
            entry.factor = task.size_factor.as_ref().map(|f| f.get());
            entry.output_size = task.output_size.as_ref().map(|s| s.size);
            Some(out)
        })
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
            tokio::time::sleep(std::time::Duration::from_millis(3600)).await;
            toasts.with_mut(|list| {
                if let Some(t) = list.iter_mut().find(|t| t.id == id) {
                    t.leaving = true;
                }
            });
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            toasts.with_mut(|list| list.retain(|t| t.id != id));
        });
    }

    /// 添加文件，仅保留受支持的扩展名（与 iced NewFiles 的过滤一致）。
    /// 每个新任务在后台线程探测时长/编码，期间显示 Probing 状态。
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
                        list.push(TaskEntry::new(&path));
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

    /// 后台探测第 index 个任务（check_input），完成后回写状态与时长。
    fn spawn_probe(&mut self, index: usize) {
        let Some(entry) = self.tasks.cloned().get(index).cloned() else {
            return;
        };
        let task_arc = Arc::clone(&entry.transcoder);
        let mut ctx = *self;
        spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut t = task_arc.lock().map_err(|e| e.to_string())?;
                t.probe().map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(format!("join error: {e}")));
            ctx.tasks.with_mut(|list| {
                // 队列可能已被清空/重排：校验仍是同一个任务
                let Some(e) = list.get_mut(index) else {
                    return;
                };
                if !Arc::ptr_eq(&e.transcoder, &entry.transcoder) {
                    return;
                }
                match result {
                    Ok(()) => {
                        e.status = Status::Pending;
                        e.input_duration = e
                            .transcoder
                            .lock()
                            .ok()
                            .and_then(|t| t.media_file.duration());
                    }
                    Err(err) => {
                        e.status = Status::Alert;
                        e.error = Some(err);
                    }
                }
            });
        });
    }

    /// 移除已完成（Done）的任务。
    pub fn clear_done(&mut self) {
        self.tasks
            .with_mut(|list| list.retain(|task| task.status != Status::Done));
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
        let output_dir = PathBuf::from(self.output_dir.peek().clone());
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
        self.overall_progress.set(0.0);
        self.cancel.set(false);
        self.running.set(true);

        // 实时进度通道：worker 线程发送 → 接收循环写显示镜像
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::runner::ProgressUpdate>();
        let mut tasks = self.tasks;
        spawn(async move {
            while let Some(update) = progress_rx.recv().await {
                tasks.with_mut(|list| {
                    if let Some(entry) = list.get_mut(update.index) {
                        entry.progress = Some(update.pct.clamp(0.0, 1.0));
                    }
                });
            }
        });

        spawn(crate::runner::run_all(*self, progress_tx));
    }

    /// 选择输出目录对话框（HTML 无目录选择器，沿用 rfd）。
    pub fn pick_output_dir(&mut self) {
        let mut output_dir = self.output_dir;
        spawn(async move {
            if let Some(folder) = rfd::AsyncFileDialog::new().pick_folder().await {
                output_dir.set(folder.path().to_string_lossy().into());
            }
        });
    }
}

fn default_output_dir() -> String {
    let mut output = std::env::current_dir().unwrap_or_else(|e| {
        log::error!("Failed to get current directory: {}", e);
        PathBuf::from(".")
    });
    output.push("output");
    output.to_string_lossy().into()
}

#[component]
pub fn App() -> Element {
    let ctx = UiState {
        tasks: use_signal(Vec::new),
        output_dir: use_signal(default_output_dir),
        max_retry: use_signal(|| 3u8),
        running: use_signal(|| false),
        overall_progress: use_signal(|| 0.0f32),
        cancel: use_signal(|| false),
        toasts: use_signal(Vec::new),
        theme: use_signal(|| Theme::Light),
    };

    // 主题属性挂到 <html>，CSS 变量按 [data-theme="dark"] 覆盖
    use_effect(move || {
        let theme = ctx.theme.read().attr();
        document::eval(&format!(
            "document.documentElement.setAttribute('data-theme', '{theme}')"
        ));
    });

    use_context_provider(move || ctx);

    rsx! {
        style { {include_str!("app.css")} }
        DropZone {
            Toolbar {}
            TaskList {}
            ProgressBar {}
            crate::components::toast::ToastContainer {}
        }
    }
}
