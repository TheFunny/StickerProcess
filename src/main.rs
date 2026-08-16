mod media;
mod transcoder;
//
// fn main() {
//     pretty_env_logger::init();
//     let media_file = media::MediaFile::new("input/2024-04-10-315.png".to_string());
//     let mut transcoder = transcoder::Transcoder::new(media_file);
//     transcoder
//         .set_output(&"test1.png".to_string())
//         .expect("TODO: panic message");
//     println!("{:?}", transcoder);
//     transcoder.run().unwrap();
// }

use crate::media::{MediaFile, MediaType};
use crate::transcoder::{Factor, FileSize, Status, Transcoder};
use iced::{
    Task,
    alignment::Vertical,
    event,
    widget::{
        button, column, keyed_column, progress_bar, row, scrollable, space, text, text_input,
    },
    window,
};
use iced_aw::{ICED_AW_FONT_BYTES, NumberInput};
use log::error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn main() -> iced::Result {
    pretty_env_logger::init();
    iced::application(App::new, App::update, App::view)
        .subscription(App::subscription)
        .title(App::title)
        .font(ICED_AW_FONT_BYTES)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    AddFile,                          // on click add file button
    NewFiles(Option<Vec<Box<Path>>>), // on select new files
    SelectOutput,                     // on click select output button
    SetOutput(Option<PathBuf>),       // on select output folder
    ChangeOutput(String),             // on change output text input
    ChangeRetry(u8),                  // on change retry times
    ClearDone,                        // on click clear done button
    SelectRun,                        // on click run button
    CurrentProcess(ProcessInfo),      // to start task with index
    NextProcess(ProcessResult),       // on finish task with result
    Done,                             // on finish all tasks
    Error(isize, String),             // on task error
    TaskMessage(usize, TaskMessage),
}

#[derive(Debug, Clone)]
struct ProcessInfo {
    index: usize,
    retry: u8,
}

#[derive(Debug, Clone)]
struct ProcessResult {
    index: usize,
    result: Result<(), String>,
    retry: u8,
}

const VIDEO: [&str; 3] = ["mp4", "gif", "apng"];
const IMAGE: [&str; 3] = ["jpg", "jpeg", "png"];
const SUPPORTED: [&str; 6] = ["mp4", "gif", "apng", "jpg", "jpeg", "png"];

#[derive(Debug, Default)]
struct App {
    output_dir: String,
    tasks: Vec<Arc<Mutex<Transcoder>>>,
    idle: bool,
    progress: f32,
    max_retry: u8,
}

impl App {
    fn new() -> (App, Task<Message>) {
        let mut output = std::env::current_dir().unwrap_or_else(|e| {
            error!("Failed to get current directory: {}", e);
            PathBuf::from(".")
        });
        output.push("output");
        (
            App {
                output_dir: output.to_string_lossy().into(),
                idle: true,
                max_retry: 3,
                ..Self::default()
            },
            Task::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Sticker Process")
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AddFile => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter("Supported Types", &SUPPORTED)
                        .add_filter("Video", &VIDEO)
                        .add_filter("Image", &IMAGE)
                        .add_filter("All", &["*"])
                        .pick_files()
                        .await
                        .map(|files| files.into_iter().map(|file| file.path().into()).collect())
                },
                Message::NewFiles,
            ),
            Message::NewFiles(files) => {
                if let Some(files) = files {
                    self.tasks.extend(files.into_iter().filter_map(|file| {
                        let ext = file
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .map(|s| s.to_ascii_lowercase());
                        match ext.as_deref() {
                            Some(ext) if SUPPORTED.contains(&ext) => {
                                Some(Arc::new(Mutex::new(Transcoder::new(MediaFile::new(&file)))))
                            }
                            _ => None,
                        }
                    }));
                };
                Task::none()
            }
            Message::SelectOutput => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .pick_folder()
                        .await
                        .map(|folder| folder.path().to_path_buf())
                },
                Message::SetOutput,
            ),
            Message::SetOutput(output) => {
                if let Some(output) = output {
                    self.output_dir = output.to_string_lossy().into();
                }
                Task::none()
            }
            Message::ChangeOutput(output) => {
                self.output_dir = output;
                Task::none()
            }
            Message::ChangeRetry(retry) => {
                self.max_retry = retry;
                Task::none()
            }
            Message::ClearDone => {
                self.tasks
                    .retain(|task| task.lock().map_or(true, |t| t.status != Status::Done));
                Task::none()
            }
            Message::SelectRun => {
                if self.tasks.is_empty() {
                    return Task::none();
                }
                let output_dir = Path::new(&self.output_dir);
                if !output_dir.exists()
                    && let Err(e) = std::fs::create_dir(output_dir)
                {
                    error!("Failed to create output directory: {:?}", e);
                    return Task::done(Message::Error(-1, e.to_string()));
                }
                self.progress = 0.0;
                self.idle = false;
                Task::done(Message::CurrentProcess(ProcessInfo { index: 0, retry: 0 }))
            }
            Message::CurrentProcess(info) => {
                let i = info.index;
                if i < self.tasks.len() {
                    let task_arc = Arc::clone(&self.tasks[i]);
                    {
                        let mut task = task_arc.lock().unwrap();
                        if task.get_output().is_none()
                            && let Err(e) = task.set_output_dir(&self.output_dir)
                        {
                            error!("Failed to set output: {:?}", e);
                            return Task::done(Message::Error(-1, e.to_string()));
                        }
                    }
                    Task::perform(
                        async move {
                            let handle = tokio::task::spawn_blocking(move || {
                                let mut task = task_arc.lock().unwrap();
                                task.run().map_err(|e| e.to_string())
                            });
                            ProcessResult {
                                index: i,
                                result: handle.await.unwrap(),
                                retry: info.retry,
                            }
                        },
                        Message::NextProcess,
                    )
                } else {
                    Task::done(Message::Done)
                }
            }
            Message::NextProcess(result) => match result.result {
                Ok(..) => match self.tasks[result.index].lock() {
                    Ok(mut task) => {
                        task.status = match task.check_size() {
                            Ok(_) => match task.size_excess_factor() {
                                Some(excess) if excess > 1. => {
                                    let size_factor = task.size_factor.as_mut().unwrap();
                                    size_factor.set(size_factor.get() / excess * 0.96);
                                    Status::SizeExcess
                                }
                                Some(_) => Status::Done,
                                None => Status::Alert,
                            },
                            Err(e) => {
                                error!("Error check size: {:?}", e);
                                Status::Alert
                            }
                        };
                        Task::done(Message::CurrentProcess(
                            if task.status == Status::SizeExcess && self.max_retry > result.retry {
                                ProcessInfo {
                                    index: result.index,
                                    retry: result.retry + 1,
                                }
                            } else {
                                self.progress = (result.index + 1) as f32 / self.tasks.len() as f32;
                                ProcessInfo {
                                    index: result.index + 1,
                                    retry: 0,
                                }
                            },
                        ))
                    }
                    Err(e) => Task::done(Message::Error(-1, e.to_string())),
                },
                Err(e) => {
                    error!("Failed to process: {:?}", e);
                    error!("Current Task: {:?}", self.tasks[result.index]);
                    if let Ok(mut task) = self.tasks[result.index].lock() {
                        task.status = Status::Alert;
                    }
                    Task::done(Message::Error((result.index + 1) as isize, e))
                }
            },
            Message::Done => {
                self.idle = true;
                Task::none()
            }
            Message::Error(i, e) => Task::perform(
                async move {
                    rfd::AsyncMessageDialog::new()
                        .set_title("Error")
                        .set_description(e)
                        .show()
                        .await;
                    match i {
                        -1 => Message::Done,
                        _ => Message::CurrentProcess(ProcessInfo {
                            index: i as usize,
                            retry: 0,
                        }),
                    }
                },
                |m| m,
            ),
            Message::TaskMessage(i, message) => {
                if let Ok(mut task) = self.tasks[i].lock() {
                    let _ = task.update(message);
                }
                Task::none()
            }
        }
    }

    fn view(&'_ self) -> iced::Element<'_, Message> {
        column![
            row![
                button("Add File").on_press_maybe(self.idle.then_some(Message::AddFile)),
                button("Clear Done").on_press_maybe(self.idle.then_some(Message::ClearDone)),
                space::horizontal().width(iced::Length::Fill),
                text("Retry times:"),
                NumberInput::new(&self.max_retry, 0u8..=10u8, Message::ChangeRetry).width(50),
                button("Run").on_press_maybe(
                    (self.idle && !self.tasks.is_empty()).then_some(Message::SelectRun)
                ),
            ]
            .align_y(Vertical::Center)
            .spacing(10),
            row![
                text("Output Dir:"),
                text_input("Type output directory here", &self.output_dir)
                    .on_input_maybe(self.idle.then_some(Message::ChangeOutput)),
                button("Select").on_press_maybe(self.idle.then_some(Message::SelectOutput)),
            ]
            .align_y(Vertical::Center)
            .spacing(10),
            scrollable(keyed_column(self.tasks.iter().enumerate().map(
                |(i, task_arc)| {
                    let element = {
                        let task = task_arc.lock().unwrap();
                        task.view()
                    };
                    (i, element.map(move |m| Message::TaskMessage(i, m)))
                }
            )))
            .width(iced::Length::Fill)
            .height(iced::Length::Fill),
        ]
        .push((!self.idle).then_some(progress_bar(0.0..=1.0, self.progress)))
        .padding(10)
        .spacing(10)
        .into()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        event::listen_with(|event, _status, _windows| match event {
            event::Event::Window(window::Event::FileDropped(path)) => {
                Some(Message::NewFiles(Some(vec![Box::from(path)])))
            }
            _ => None,
        })
    }
}

#[derive(Debug, Clone)]
enum TaskMessage {
    FactorMessage(FactorMessage),
}

impl Transcoder {
    fn update(&mut self, message: TaskMessage) -> Task<TaskMessage> {
        match message {
            TaskMessage::FactorMessage(factor_message) => {
                let _ = self.size_factor.as_mut().unwrap().update(factor_message);
                Task::none()
            }
        }
    }

    fn view(&self) -> iced::Element<'static, TaskMessage> {
        // println!("size factor {:?}", self.size_factor);
        let factor = self
            .size_factor
            .as_ref()
            .map(|factor| factor.view().map(TaskMessage::FactorMessage));
        row![
            text(format!(
                "[{:?}] {}",
                self.status,
                self.media_file.path_str()
            ))
            .width(iced::Length::Fill),
        ]
        .push(
            self.output_size.as_ref().map(|size| {
                size.view(self.size_excess_factor().is_some_and(|excess| excess > 1.0))
            }),
        )
        .push(factor)
        .height(32)
        .align_y(Vertical::Center)
        .spacing(10)
        .into()
    }

    const IMAGE_MAX_SIZE: u64 = 512 * 1024;
    const VIDEO_MAX_SIZE: u64 = 256 * 1024;

    fn size_excess_factor(&self) -> Option<f64> {
        self.output_size.clone().map(|file_size| {
            file_size.size as f64
                / match self.media_file.r#type() {
                    Some(MediaType::Image(..)) => Self::IMAGE_MAX_SIZE as f64,
                    Some(MediaType::Video(..)) => Self::VIDEO_MAX_SIZE as f64,
                    None => unreachable!(),
                }
        })
    }
}

#[derive(Debug, Clone)]
enum FactorMessage {
    OnInputChange(f64),
}

impl Factor {
    fn update(&mut self, message: FactorMessage) -> Task<TaskMessage> {
        match message {
            FactorMessage::OnInputChange(input) => {
                self.set(input);
                Task::none()
            }
        }
    }

    fn view(&self) -> iced::Element<'static, FactorMessage> {
        NumberInput::new(&self.get(), 0.1..=10.0, FactorMessage::OnInputChange)
            .width(60)
            .step(0.1)
            .into()
    }
}

impl FileSize {
    fn view(&self, is_excess: bool) -> iced::Element<'static, TaskMessage> {
        row![
            text(format! {"{:.2}KB", self.size as f64 / 1024.0}).style(if is_excess {
                text::danger
            } else {
                text::success
            }),
        ]
        .align_y(Vertical::Center)
        .into()
    }
}
