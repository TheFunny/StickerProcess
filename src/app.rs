//! 根组件与全局信号状态（Phase A 状态模型）。
//!
//! iced 的 `App`/`Message`/`update` 架构 → dioxus 信号 + 异步任务：
//! 任务仍以 `Arc<Mutex<Transcoder>>` 共享给后台 worker（共享模式不变），
//! 另存少量"显示镜像"字段，让 UI 渲染时**不加锁**（worker 转码期间会长期持有锁）。

use crate::components::{
    drop_zone::DropZone, progress_bar::ProgressBar, task_list::TaskList, toolbar::Toolbar,
};
use crate::media::MediaFile;
use crate::transcoder::{Status, Transcoder};
use dioxus::prelude::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// AGENTS 约定保留：新增媒体类型时同步维护（rfd 过滤器在 Phase B 设置面板中复用）
#[allow(dead_code)]
pub const VIDEO: [&str; 3] = ["mp4", "gif", "apng"];
#[allow(dead_code)]
pub const IMAGE: [&str; 3] = ["jpg", "jpeg", "png"];
pub const SUPPORTED: [&str; 6] = ["mp4", "gif", "apng", "jpg", "jpeg", "png"];

/// 队列中的一个任务：共享 Transcoder（逻辑层）+ 显示镜像（渲染层）。
#[derive(Clone)]
pub struct TaskEntry {
    pub transcoder: Arc<Mutex<Transcoder>>,
    /// 输入文件路径（创建时快照，避免渲染期加锁）。
    pub input_path: String,
    // Phase A 仅记录；Phase C/D 的"添加时探测/预览"会展示
    #[allow(dead_code)]
    pub input_size: u64,
    #[allow(dead_code)]
    pub input_duration: Option<f64>,
    pub is_video: bool,
    // ---- 显示镜像：由修改 Transcoder 的一方负责同步 ----
    pub status: Status,
    pub output_size: Option<u64>,
    pub factor: Option<f64>,
}

// 组件 Props 派生需要 PartialEq；共享的 Transcoder 不参与比较（比较显示镜像即可）
impl PartialEq for TaskEntry {
    fn eq(&self, other: &Self) -> bool {
        self.input_path == other.input_path
            && self.is_video == other.is_video
            && self.status == other.status
            && self.output_size == other.output_size
            && self.factor == other.factor
    }
}

impl TaskEntry {
    pub fn new(path: &PathBuf) -> Self {
        // 与 iced 版一致：添加时同步探测（Transcoder::new → check_input）
        let transcoder = Transcoder::new(MediaFile::new(path));
        let input_path = transcoder.media_file.path_str();
        let input_duration = transcoder.media_file.duration();
        let is_video = matches!(
            transcoder.media_file.r#type(),
            Some(crate::media::MediaType::Video(_))
        );
        let status = transcoder.status.clone();
        log::info!("Added task: {input_path}");
        Self {
            transcoder: Arc::new(Mutex::new(transcoder)),
            input_path,
            input_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            input_duration,
            is_video,
            status,
            output_size: None,
            factor: None,
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

/// 全部界面状态（signals）。`Signal<T>` 为 Copy，可按值传递。
#[derive(Clone, Copy)]
pub struct UiState {
    pub tasks: Signal<Vec<TaskEntry>>,
    pub output_dir: Signal<String>,
    pub max_retry: Signal<u8>,
    pub running: Signal<bool>,
    pub overall_progress: Signal<f32>,
    /// 取消标记：runner 在每次尝试前检查（UI 开关在 Phase C 提供）。
    pub cancel: Signal<bool>,
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

    /// 添加文件，仅保留受支持的扩展名（与 iced NewFiles 的过滤一致）。
    pub fn add_files(&mut self, files: Vec<PathBuf>) {
        self.tasks.with_mut(|list| {
            for path in files {
                let ext = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|s| s.to_ascii_lowercase());
                match ext.as_deref() {
                    Some(ext) if SUPPORTED.contains(&ext) => list.push(TaskEntry::new(&path)),
                    _ => log::warn!("Skipped unsupported file: {}", path.display()),
                }
            }
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
        let output_dir = PathBuf::from(self.output_dir.peek().clone());
        if !output_dir.exists()
            && let Err(e) = std::fs::create_dir(&output_dir)
        {
            log::error!("Failed to create output directory: {:?}", e);
            spawn(async move { crate::runner::show_error_dialog(e.to_string()).await });
            return;
        }
        self.overall_progress.set(0.0);
        self.cancel.set(false);
        self.running.set(true);
        spawn(crate::runner::run_all(*self));
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

/// 内嵌样式（Phase C 将换成主题系统/CSS 变量）。
const APP_CSS: &str = include_str!("app.css");

#[component]
pub fn App() -> Element {
    let ctx = UiState {
        tasks: use_signal(Vec::new),
        output_dir: use_signal(default_output_dir),
        max_retry: use_signal(|| 3u8),
        running: use_signal(|| false),
        overall_progress: use_signal(|| 0.0f32),
        cancel: use_signal(|| false),
    };
    use_context_provider(move || ctx);

    rsx! {
        style { {APP_CSS} }
        DropZone {
            Toolbar {}
            TaskList {}
            ProgressBar {}
        }
    }
}
