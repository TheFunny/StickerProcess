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
use log::error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn main() -> iced::Result {
    pretty_env_logger::init();
    iced::application(App::new, App::update, App::view)
        .subscription(App::subscription)
        .title(App::title)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    AddFile,
    NewFiles(Option<Vec<Box<Path>>>),
    SelectOutput,
    SetOutput(Option<PathBuf>),
    ChangeOutput(String),
    ClearDone,
    SelectRun,
    CurrentProcess(usize),
    NextProcess(ProcessResult),
    Done,
    Error(isize, String),
    TaskMessage(usize, TaskMessage),
}

#[derive(Debug, Clone)]
struct ProcessResult {
    index: usize,
    result: Result<Option<f64>, String>,
}

const VIDEO: [&str; 2] = ["mp4", "gif"];
const IMAGE: [&str; 3] = ["jpg", "jpeg", "png"];
const SUPPORTED: [&str; 5] = ["mp4", "gif", "jpg", "jpeg", "png"];

#[derive(Debug, Default)]
struct App {
    output_dir: String,
    tasks: Vec<Arc<Mutex<Transcoder>>>,
    idle: bool,
    progress: f32,
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
                Task::done(Message::CurrentProcess(0))
            }
            Message::CurrentProcess(i) => {
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
                            }
                        },
                        Message::NextProcess,
                    )
                } else {
                    Task::done(Message::Done)
                }
            }
            Message::NextProcess(result) => match result.result {
                Ok(factor) => {
                    if let Ok(mut task) = self.tasks[result.index].lock() {
                        if let Some(factor) = factor {
                            task.size_factor = Some(Factor::new(factor));
                        }
                        task.status = match task.check_size() {
                            Ok(_) => {
                                if task.is_size_excess() {
                                    Status::SizeExcess
                                } else {
                                    Status::Done
                                }
                            }
                            Err(e) => {
                                error!("Error check size: {:?}", e);
                                Status::Alert
                            }
                        };
                    }
                    self.progress = (result.index + 1) as f32 / self.tasks.len() as f32;
                    Task::done(Message::CurrentProcess(result.index + 1))
                }
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
                        _ => Message::CurrentProcess(i as usize),
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
                button("Run").on_press_maybe(self.idle.then_some(Message::SelectRun)),
            ]
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
        println!("size factor {:?}", self.size_factor);
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
            self.output_size
                .as_ref()
                .map(|size| size.view(self.is_size_excess())),
        )
        .push(factor)
        .height(32)
        .align_y(Vertical::Center)
        .spacing(10)
        .into()
    }

    const IMAGE_MAX_SIZE: u64 = 512_000;
    const VIDEO_MAX_SIZE: u64 = 256_000;

    fn is_size_excess(&self) -> bool {
        if let Some(size) = &self.output_size {
            match self.media_file.r#type() {
                Some(MediaType::Image(_)) => size.size > Self::IMAGE_MAX_SIZE,
                Some(MediaType::Video(_)) => size.size > Self::VIDEO_MAX_SIZE,
                None => false,
            }
        } else {
            false
        }
    }
}

#[derive(Debug, Clone)]
enum FactorMessage {
    EditFactor,
    SaveFactor,
    OnInputChange(String),
}

impl Factor {
    fn update(&mut self, message: FactorMessage) -> Task<TaskMessage> {
        match message {
            FactorMessage::EditFactor => {
                self.temp = Some(self.get().to_string());
                Task::none()
            }
            FactorMessage::SaveFactor => {
                if let Ok(factor) = self.temp.as_ref().unwrap().parse::<f64>() {
                    self.set(factor);
                    self.temp = None;
                    Task::none()
                } else {
                    error!("Failed to parse factor: {:?}", self.temp);
                    Task::none()
                }
            }
            FactorMessage::OnInputChange(input) => {
                self.temp = Some(input);
                Task::none()
            }
        }
    }

    fn view(&self) -> iced::Element<'static, FactorMessage> {
        if let Some(temp) = self.temp.as_ref() {
            row![
                text_input("Factor", temp)
                    .on_input(FactorMessage::OnInputChange)
                    .width(iced::Length::Fixed(60.)),
                space::horizontal().width(5),
                button("Save").on_press(FactorMessage::SaveFactor),
            ]
            .align_y(Vertical::Center)
            .into()
        } else {
            row![
                text(format!("Factor: {}", self.get())),
                space::horizontal().width(5),
                button("Edit").on_press(FactorMessage::EditFactor),
            ]
            .align_y(Vertical::Center)
            .into()
        }
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
