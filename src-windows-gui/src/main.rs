#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use iced::time;
use iced::widget::{Space, button, column, container, row, scrollable, text, text_input, toggler};
use iced::widget::{button as button_style, container as container_style};
use iced::{
    Alignment, Background, Border, Color, Element, Font, Length, Size, Subscription, Task, Theme,
    window,
};
use muda::{Menu, MenuEvent, MenuItem};
use shu_net_keeper::config::{APPConfig, APPConfigValidated, SmtpConfig, validate_config};
use shu_net_keeper::core;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use tray_icon::{TrayIcon, TrayIconBuilder};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

const APP_DIR: &str = "com.shu-net-keeper";
const SETTINGS_FILE: &str = "settings.json";
const AUTOSTART_VALUE_NAME: &str = "SHU Net Keeper";
const MAX_LOGS: usize = 300;
const LEFT_PANEL_WIDTH: f32 = 360.0;
const UI_FONT: Font = Font::with_name("Microsoft YaHei UI");

#[derive(Debug, Clone, Default)]
struct DaemonStatus {
    running: bool,
    connected: bool,
    ip: Option<String>,
    last_check: Option<String>,
    last_error: Option<String>,
    login_count: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSettings {
    config: APPConfig,
}

#[derive(Debug, Clone)]
enum UiMessage {
    Log(String),
    Status(DaemonStatus),
}

#[derive(Debug, Clone, Copy)]
enum BannerKind {
    Success,
    Error,
    Info,
}

#[derive(Debug, Clone)]
struct Banner {
    kind: BannerKind,
    text: String,
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    UsernameChanged(String),
    PasswordChanged(String),
    IntervalChanged(String),
    SmtpEnabledChanged(bool),
    SmtpServerChanged(String),
    SmtpPortChanged(String),
    SmtpSenderChanged(String),
    SmtpPasswordChanged(String),
    SmtpReceiverChanged(String),
    TogglePasswordVisibility,
    ToggleSmtpPasswordVisibility,
    SaveConfig,
    StartDaemon,
    StopDaemon,
    AutostartChanged(bool),
    AutoScrollChanged(bool),
    ClearLogs,
    DismissBanner,
    WindowCloseRequested(window::Id),
    ShowWindow,
    ExitApp,
    OpenGitHub,
}

struct WindowsGuiApp {
    username: String,
    password: String,
    interval: String,
    smtp_enabled: bool,
    smtp_server: String,
    smtp_port: String,
    smtp_sender: String,
    smtp_password: String,
    smtp_receiver: String,
    password_visible: bool,
    smtp_password_visible: bool,
    autostart_enabled: bool,
    auto_scroll: bool,
    banner: Option<Banner>,
    notice_rx: Receiver<UiMessage>,
    notice_tx: Sender<UiMessage>,
    daemon_running: Arc<AtomicBool>,
    status: DaemonStatus,
    logs: Vec<String>,
    // Tray
    _tray_icon: TrayIcon,
    tray_show_id: muda::MenuId,
    tray_exit_id: muda::MenuId,
}

impl WindowsGuiApp {
    fn new(start_hidden: bool) -> (Self, Task<Message>) {
        let (notice_tx, notice_rx) = mpsc::channel();

        // Build tray menu
        let menu = Menu::new();
        let item_show = MenuItem::new("显示窗口", true, None);
        let item_exit = MenuItem::new("退出程序", true, None);
        let show_id = item_show.id().clone();
        let exit_id = item_exit.id().clone();
        menu.append(&item_show).unwrap();
        menu.append(&item_exit).unwrap();

        // Load icon from embedded exe resource (ico file)
        let icon_bytes = include_bytes!("../../src-tauri/icons/icon.ico");
        let tray_icon_image = load_icon_from_ico(icon_bytes);

        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("SHU Net Keeper")
            .with_icon(tray_icon_image)
            .with_menu(Box::new(menu))
            .build()
            .expect("failed to create tray icon");

        let mut app = Self {
            username: String::new(),
            password: String::new(),
            interval: "600".to_string(),
            smtp_enabled: false,
            smtp_server: String::new(),
            smtp_port: String::new(),
            smtp_sender: String::new(),
            smtp_password: String::new(),
            smtp_receiver: String::new(),
            password_visible: false,
            smtp_password_visible: false,
            autostart_enabled: get_autostart_enabled().unwrap_or(false),
            auto_scroll: true,
            banner: None,
            notice_rx,
            notice_tx,
            daemon_running: Arc::new(AtomicBool::new(false)),
            status: DaemonStatus::default(),
            logs: Vec::new(),
            _tray_icon: tray_icon,
            tray_show_id: show_id,
            tray_exit_id: exit_id,
        };

        if let Ok(Some(config)) = load_gui_config() {
            app.apply_config(config);
        }

        let startup_task = if start_hidden {
            window::get_latest().and_then(|id| window::minimize(id, true))
        } else {
            Task::none()
        };

        (app, startup_task)
    }

    fn title(&self) -> String {
        "SHU Net Keeper for Windows".to_string()
    }

    fn theme(&self) -> Theme {
        Theme::Light
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            time::every(Duration::from_millis(350)).map(|_| Message::Tick),
            window::close_requests().map(Message::WindowCloseRequested),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.consume_messages();
                self.poll_tray_events()
            }
            Message::UsernameChanged(value) => {
                self.username = value;
                Task::none()
            }
            Message::PasswordChanged(value) => {
                self.password = value;
                Task::none()
            }
            Message::IntervalChanged(value) => {
                self.interval = value;
                Task::none()
            }
            Message::SmtpEnabledChanged(enabled) => {
                self.smtp_enabled = enabled;
                Task::none()
            }
            Message::SmtpServerChanged(value) => {
                self.smtp_server = value;
                Task::none()
            }
            Message::SmtpPortChanged(value) => {
                self.smtp_port = value;
                Task::none()
            }
            Message::SmtpSenderChanged(value) => {
                self.smtp_sender = value;
                Task::none()
            }
            Message::SmtpPasswordChanged(value) => {
                self.smtp_password = value;
                Task::none()
            }
            Message::SmtpReceiverChanged(value) => {
                self.smtp_receiver = value;
                Task::none()
            }
            Message::TogglePasswordVisibility => {
                self.password_visible = !self.password_visible;
                Task::none()
            }
            Message::ToggleSmtpPasswordVisibility => {
                self.smtp_password_visible = !self.smtp_password_visible;
                Task::none()
            }
            Message::SaveConfig => {
                self.save_config_with_feedback();
                Task::none()
            }
            Message::StartDaemon => {
                self.start_daemon();
                Task::none()
            }
            Message::StopDaemon => {
                self.stop_daemon();
                Task::none()
            }
            Message::AutostartChanged(enabled) => {
                self.autostart_enabled = enabled;
                if let Err(err) = set_autostart_enabled(enabled) {
                    self.autostart_enabled = !enabled;
                    self.show_banner(BannerKind::Error, format!("设置开机自启失败：{err}"));
                } else {
                    self.show_banner(
                        BannerKind::Success,
                        if enabled {
                            "已启用开机自启（最小化启动）".to_string()
                        } else {
                            "已关闭开机自启".to_string()
                        },
                    );
                }
                Task::none()
            }
            Message::AutoScrollChanged(enabled) => {
                self.auto_scroll = enabled;
                Task::none()
            }
            Message::ClearLogs => {
                self.logs.clear();
                Task::none()
            }
            Message::DismissBanner => {
                self.banner = None;
                Task::none()
            }
            Message::WindowCloseRequested(id) => {
                // Hide window to tray instead of exiting
                window::minimize(id, true).chain(window::change_mode(id, window::Mode::Hidden))
            }
            Message::ShowWindow => {
                window::get_latest().and_then(|id| {
                    window::change_mode(id, window::Mode::Windowed)
                        .chain(window::minimize(id, false))
                        .chain(window::gain_focus(id))
                })
            }
            Message::ExitApp => {
                self.daemon_running.store(false, Ordering::SeqCst);
                iced::exit()
            }
            Message::OpenGitHub => {
                let _ = open::that("https://github.com/BeiningWu/shu-net-keeper");
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let header = container(
            row![
                text("SHU Net Keeper").size(20),
                Space::with_width(Length::Fill),
                self.status_badge()
            ]
            .align_y(Alignment::Center),
        )
        .padding([14, 18])
        .style(header_style);

        let body = row![self.left_panel(), self.right_panel(),]
            .spacing(14)
            .height(Length::Fill);

        container(
            column![
                header,
                container(body).padding([14, 16]).height(Length::Fill)
            ]
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(app_shell_style)
        .into()
    }

    fn left_panel(&self) -> Element<'_, Message> {
        let mut content = column![].spacing(14);

        if let Some(banner) = &self.banner {
            content = content.push(self.banner_card(banner));
        }

        content = content
            .push(self.config_card())
            .push(self.status_card())
            .push(self.footer_card());

        container(scrollable(content).width(Length::Fill))
            .width(Length::Fixed(LEFT_PANEL_WIDTH))
            .style(side_panel_style)
            .into()
    }

    fn right_panel(&self) -> Element<'_, Message> {
        let logs_header = row![
            text("运行日志").size(16),
            Space::with_width(Length::Fill),
            toggler(self.auto_scroll)
                .label("自动滚动")
                .on_toggle(Message::AutoScrollChanged),
            button("清空")
                .on_press(Message::ClearLogs)
                .style(button_style::secondary)
        ]
        .align_y(Alignment::Center)
        .spacing(12);

        let logs_body: Element<'_, Message> = if self.logs.is_empty() {
            container(
                text("暂无日志，启动守护后将在此显示运行记录")
                    .size(14)
                    .font(UI_FONT)
                    .color(Color::from_rgb8(0x6B, 0x72, 0x80)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        } else {
            let mut lines = column![].spacing(4).width(Length::Fill);
            for line in &self.logs {
                lines = lines.push(
                    text(line)
                        .size(13)
                        .font(UI_FONT)
                        .color(log_text_color(line)),
                );
            }
            scrollable(lines)
                .height(Length::Fill)
                .width(Length::Fill)
                .into()
        };

        container(
            column![
                logs_header,
                container(logs_body)
                    .padding(14)
                    .height(Length::Fill)
                    .style(log_surface_style)
            ]
            .spacing(14)
            .height(Length::Fill),
        )
        .padding(18)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(card_style)
        .into()
    }

    fn config_card(&self) -> Element<'_, Message> {
        let mut form = column![
            section_header("配置"),
            input_field(
                "学号",
                "8 位数字",
                text_input("12345678", &self.username)
                    .on_input(Message::UsernameChanged)
                    .padding(10)
                    .size(14)
                    .width(Length::Fill)
                    .into(),
            ),
            password_field(
                "密码",
                "校园网密码",
                &self.password,
                self.password_visible,
                Message::PasswordChanged,
                Message::TogglePasswordVisibility,
            ),
            input_field(
                "检查间隔（秒）",
                "默认 600",
                text_input("600", &self.interval)
                    .on_input(Message::IntervalChanged)
                    .padding(10)
                    .size(14)
                    .width(Length::Fill)
                    .into(),
            ),
            toggler(self.smtp_enabled)
                .label("启用 SMTP 邮件通知")
                .on_toggle(Message::SmtpEnabledChanged),
        ]
        .spacing(14);

        if self.smtp_enabled {
            form = form
                .push(input_field(
                    "SMTP 服务器",
                    "smtp.qq.com",
                    text_input("smtp.qq.com", &self.smtp_server)
                        .on_input(Message::SmtpServerChanged)
                        .padding(10)
                        .size(14)
                        .width(Length::Fill)
                        .into(),
                ))
                .push(input_field(
                    "SMTP 端口",
                    "465 / 587",
                    text_input("465", &self.smtp_port)
                        .on_input(Message::SmtpPortChanged)
                        .padding(10)
                        .size(14)
                        .width(Length::Fill)
                        .into(),
                ))
                .push(input_field(
                    "发件邮箱",
                    "you@example.com",
                    text_input("you@example.com", &self.smtp_sender)
                        .on_input(Message::SmtpSenderChanged)
                        .padding(10)
                        .size(14)
                        .width(Length::Fill)
                        .into(),
                ))
                .push(password_field(
                    "SMTP 密码 / 授权码",
                    "授权码",
                    &self.smtp_password,
                    self.smtp_password_visible,
                    Message::SmtpPasswordChanged,
                    Message::ToggleSmtpPasswordVisibility,
                ))
                .push(input_field(
                    "接收邮箱",
                    "notify@example.com",
                    text_input("notify@example.com", &self.smtp_receiver)
                        .on_input(Message::SmtpReceiverChanged)
                        .padding(10)
                        .size(14)
                        .width(Length::Fill)
                        .into(),
                ));
        }

        let action_row = if self.status.running {
            row![button("保存配置")
                .on_press(Message::SaveConfig)
                .style(button_style::primary)]
            .spacing(10)
        } else {
            row![
                button("保存配置")
                    .on_press(Message::SaveConfig)
                    .style(button_style::secondary),
                button("保存并启动")
                    .on_press(Message::StartDaemon)
                    .style(button_style::success),
            ]
            .spacing(10)
        };

        container(form.push(action_row))
            .padding(18)
            .style(card_style)
            .into()
    }

    fn status_card(&self) -> Element<'_, Message> {
        let conn_color = if !self.status.running {
            Color::from_rgb8(0x9C, 0xA3, 0xAF)
        } else if self.status.connected {
            color_success()
        } else {
            color_danger()
        };

        let conn_label = if !self.status.running {
            "未运行"
        } else if self.status.connected {
            "已连接"
        } else {
            "未连接 / 重试中"
        };

        let ip_label = self.status.ip.as_deref().unwrap_or("—");
        let last_check = self.status.last_check.as_deref().unwrap_or("—");

        let mut content = column![
            section_header("状态"),
            row![
                container(Space::with_width(Length::Fixed(12.0)))
                    .width(Length::Fixed(12.0))
                    .height(Length::Fixed(12.0))
                    .style(move |_| dot_style(conn_color)),
                column![
                    text(conn_label).size(16),
                    text(format!("IP: {ip_label}"))
                        .size(12)
                        .color(color_text_sub())
                ]
                .spacing(2)
            ]
            .align_y(Alignment::Center)
            .spacing(10),
            row![
                metric_card("登录次数", self.status.login_count.to_string()),
                metric_card("上次检查", last_check.to_string()),
            ]
            .spacing(10),
            if self.status.running {
                button("停止守护")
                    .on_press(Message::StopDaemon)
                    .style(button_style::danger)
            } else {
                button("启动守护")
                    .on_press(Message::StartDaemon)
                    .style(button_style::success)
            },
            toggler(self.autostart_enabled)
                .label("开机自启动")
                .on_toggle(Message::AutostartChanged)
        ]
        .spacing(12);

        if let Some(last_error) = &self.status.last_error {
            content = content.push(
                container(
                    text(last_error)
                        .size(12)
                        .color(Color::from_rgb8(0xB9, 0x1C, 0x1C)),
                )
                .padding(10)
                .style(error_box_style),
            );
        }

        container(content).padding(18).style(card_style).into()
    }

    fn footer_card(&self) -> Element<'_, Message> {
        container(
            column![
                text("本软件开源免费，严禁商业销售")
                    .size(12)
                    .color(color_text_sub()),
                button("GitHub: BeiningWu/shu-net-keeper")
                    .on_press(Message::OpenGitHub)
                    .style(button_style::text),
            ]
            .spacing(4),
        )
        .padding(16)
        .style(card_style)
        .into()
    }

    fn status_badge(&self) -> Element<'_, Message> {
        let (label, kind) = if !self.status.running {
            ("未运行", BannerKind::Info)
        } else if self.status.connected {
            ("运行中", BannerKind::Success)
        } else {
            ("告警", BannerKind::Error)
        };

        container(text(label).size(12))
            .padding([6, 12])
            .style(move |_| badge_style(kind))
            .into()
    }

    fn banner_card<'a>(&self, banner: &'a Banner) -> Element<'a, Message> {
        let kind = banner.kind;
        container(
            row![
                text(&banner.text).size(13),
                Space::with_width(Length::Fill),
                button("关闭")
                    .on_press(Message::DismissBanner)
                    .style(button_style::secondary)
            ]
            .align_y(Alignment::Center)
            .spacing(10),
        )
        .padding(14)
        .style(move |_| banner_style(kind))
        .into()
    }

    fn apply_config(&mut self, config: APPConfig) {
        self.username = config.username;
        self.password = config.password;
        self.interval = config.interval.to_string();
        self.smtp_enabled = config.smtp_enabled;

        if let Some(smtp) = config.smtp {
            self.smtp_server = smtp.server.unwrap_or_default();
            self.smtp_port = smtp.port.map(|port| port.to_string()).unwrap_or_default();
            self.smtp_sender = smtp.sender.unwrap_or_default();
            self.smtp_password = smtp.password.unwrap_or_default();
            self.smtp_receiver = smtp.receiver.unwrap_or_default();
        }
    }

    fn save_config_with_feedback(&mut self) {
        match self.collect_config_from_form() {
            Ok(config) => match save_gui_config(&config) {
                Ok(()) => self.show_banner(BannerKind::Success, "配置已保存到本地".to_string()),
                Err(err) => self.show_banner(BannerKind::Error, format!("保存失败：{err}")),
            },
            Err(err) => self.show_banner(BannerKind::Error, format!("配置错误：{err}")),
        }
    }

    fn start_daemon(&mut self) {
        if self.daemon_running.load(Ordering::SeqCst) {
            self.show_banner(BannerKind::Info, "守护进程已经在运行".to_string());
            return;
        }

        let config = match self.collect_config_from_form() {
            Ok(config) => config,
            Err(err) => {
                self.show_banner(BannerKind::Error, format!("配置错误：{err}"));
                return;
            }
        };

        if let Err(err) = save_gui_config(&config) {
            self.show_banner(BannerKind::Error, format!("保存失败：{err}"));
            return;
        }

        let validated = match validate_config(&config) {
            Ok(validated) => validated,
            Err(err) => {
                self.show_banner(BannerKind::Error, format!("配置验证失败：{err}"));
                return;
            }
        };

        self.daemon_running.store(true, Ordering::SeqCst);
        self.status.running = true;
        self.status.last_error = None;
        self.push_log("守护进程启动中...".to_string());
        self.show_banner(BannerKind::Success, "守护进程已启动".to_string());

        let running = Arc::clone(&self.daemon_running);
        let tx = self.notice_tx.clone();
        thread::spawn(move || {
            daemon_loop(validated, running, tx);
        });
    }

    fn stop_daemon(&mut self) {
        self.daemon_running.store(false, Ordering::SeqCst);
        self.status.running = false;
        self.show_banner(BannerKind::Info, "已请求停止守护进程".to_string());
    }

    fn consume_messages(&mut self) {
        while let Ok(message) = self.notice_rx.try_recv() {
            match message {
                UiMessage::Log(line) => self.push_log(line),
                UiMessage::Status(status) => self.status = status,
            }
        }
    }

    fn poll_tray_events(&self) -> Task<Message> {
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.tray_show_id {
                return Task::done(Message::ShowWindow);
            } else if event.id == self.tray_exit_id {
                return Task::done(Message::ExitApp);
            }
        }
        Task::none()
    }

    fn push_log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > MAX_LOGS {
            let overflow = self.logs.len() - MAX_LOGS;
            self.logs.drain(0..overflow);
        }
    }

    fn show_banner(&mut self, kind: BannerKind, text: String) {
        self.banner = Some(Banner { kind, text });
    }

    fn collect_config_from_form(&self) -> Result<APPConfig, String> {
        let interval = self
            .interval
            .trim()
            .parse::<u64>()
            .map_err(|_| "检查间隔必须是正整数".to_string())?;

        let smtp = if self.smtp_enabled {
            Some(SmtpConfig {
                server: value_or_none(self.smtp_server.clone()),
                port: parse_optional_port(self.smtp_port.clone())?,
                sender: value_or_none(self.smtp_sender.clone()),
                password: value_or_none(self.smtp_password.clone()),
                receiver: value_or_none(self.smtp_receiver.clone()),
            })
        } else {
            None
        };

        Ok(APPConfig {
            username: self.username.trim().to_string(),
            password: self.password.clone(),
            interval,
            smtp_enabled: self.smtp_enabled,
            smtp,
        })
    }
}

fn section_header<'a>(title: &'a str) -> Element<'a, Message> {
    text(title).size(16).into()
}

fn input_field<'a>(
    label: &'a str,
    hint: &'a str,
    field: Element<'a, Message>,
) -> Element<'a, Message> {
    column![
        text(label).size(13),
        field,
        text(hint).size(11).color(color_text_sub())
    ]
    .spacing(6)
    .into()
}

fn password_field<'a>(
    label: &'a str,
    placeholder: &'a str,
    value: &'a str,
    visible: bool,
    on_input: fn(String) -> Message,
    toggle_message: Message,
) -> Element<'a, Message> {
    input_field(
        label,
        "支持直接粘贴",
        row![
            text_input(placeholder, value)
                .on_input(on_input)
                .secure(!visible)
                .padding(10)
                .size(14)
                .width(Length::Fill),
            button(if visible { "隐藏" } else { "显示" })
                .on_press(toggle_message)
                .style(button_style::secondary)
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    )
}

fn metric_card<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    container(
        column![
            text(value).size(20),
            text(label).size(11).color(color_text_sub())
        ]
        .spacing(4)
        .align_x(Alignment::Start),
    )
    .padding(12)
    .width(Length::Fill)
    .style(metric_style)
    .into()
}

fn value_or_none(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn parse_optional_port(value: String) -> Result<Option<u16>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        trimmed
            .parse::<u16>()
            .map(Some)
            .map_err(|_| "SMTP 端口必须是 1-65535 的整数".to_string())
    }
}

fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn push_event(tx: &Sender<UiMessage>, message: UiMessage) {
    let _ = tx.send(message);
}

fn daemon_loop(config: APPConfigValidated, running: Arc<AtomicBool>, tx: Sender<UiMessage>) {
    let mut last_ip: Option<String> = None;
    let mut status = DaemonStatus {
        running: true,
        ..Default::default()
    };

    push_event(&tx, UiMessage::Log("守护进程已启动".to_string()));
    push_event(&tx, UiMessage::Status(status.clone()));

    while running.load(Ordering::SeqCst) {
        push_event(&tx, UiMessage::Log("正在检查网络连接状态...".to_string()));

        match core::network::check_network_connection(&mut last_ip) {
            Ok(true) => {
                status.connected = true;
                status.ip = last_ip.clone();
                status.last_check = Some(now_str());
                status.last_error = None;
                push_event(
                    &tx,
                    UiMessage::Log(format!(
                        "✓ 网络连接正常，IP: {}",
                        status.ip.as_deref().unwrap_or("未知")
                    )),
                );
            }
            Ok(false) => {
                push_event(&tx, UiMessage::Log("网络未连接，尝试登录...".to_string()));
                match core::login::network_login(&config.username, &config.password) {
                    Ok(()) => {
                        let current_ip = core::network::get_host_ip()
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| "未知".to_string());
                        let ip_changed = matches!(&last_ip, Some(old) if old != &current_ip);
                        status.connected = true;
                        status.ip = Some(current_ip.clone());
                        status.last_check = Some(now_str());
                        status.last_error = None;
                        status.login_count += 1;
                        last_ip = Some(current_ip.clone());

                        push_event(&tx, UiMessage::Log(format!("✓ 登录成功，IP: {current_ip}")));

                        if let Some(smtp) = &config.smtp {
                            match core::email::send_login_notification(
                                smtp,
                                &config.username,
                                &current_ip,
                                ip_changed,
                            ) {
                                Ok(()) => {
                                    push_event(&tx, UiMessage::Log("✓ 邮件通知已发送".to_string()))
                                }
                                Err(err) => push_event(
                                    &tx,
                                    UiMessage::Log(format!("✗ 邮件发送失败: {err}")),
                                ),
                            }
                        }
                    }
                    Err(err) => {
                        status.connected = false;
                        status.last_check = Some(now_str());
                        status.last_error = Some(err.to_string());
                        push_event(&tx, UiMessage::Log(format!("✗ 登录失败: {err}")));
                    }
                }
            }
            Err(err) => {
                status.connected = false;
                status.last_check = Some(now_str());
                status.last_error = Some(err.to_string());
                push_event(&tx, UiMessage::Log(format!("✗ 网络检查失败: {err}")));
            }
        }

        push_event(&tx, UiMessage::Status(status.clone()));

        let interval = config.interval;
        push_event(
            &tx,
            UiMessage::Log(format!("等待 {interval} 秒后再次检查...")),
        );

        for _ in 0..interval {
            if !running.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    status.running = false;
    push_event(&tx, UiMessage::Log("守护进程已停止".to_string()));
    push_event(&tx, UiMessage::Status(status));
}

fn settings_dir() -> Result<PathBuf, String> {
    let base = dirs::data_dir().ok_or_else(|| "无法定位系统应用数据目录".to_string())?;
    Ok(base.join(APP_DIR))
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(settings_dir()?.join(SETTINGS_FILE))
}

fn load_gui_config() -> Result<Option<APPConfig>, String> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read(&path).map_err(|e| format!("读取配置文件失败: {e}"))?;
    let settings: StoredSettings =
        serde_json::from_slice(&content).map_err(|e| format!("解析配置文件失败: {e}"))?;
    Ok(Some(settings.config))
}

fn save_gui_config(config: &APPConfig) -> Result<(), String> {
    let dir = settings_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;

    let settings = StoredSettings {
        config: config.clone(),
    };
    let data = serde_json::to_vec_pretty(&settings).map_err(|e| format!("序列化配置失败: {e}"))?;
    fs::write(dir.join(SETTINGS_FILE), data).map_err(|e| format!("写入配置文件失败: {e}"))
}

fn run_key() -> Result<RegKey, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .map(|(key, _)| key)
        .map_err(|e| format!("访问注册表失败: {e}"))
}

fn autostart_command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("获取当前程序路径失败: {e}"))?;
    Ok(format!("\"{}\" --minimized", exe.display()))
}

fn get_autostart_enabled() -> Result<bool, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            KEY_READ,
        )
        .map_err(|e| format!("读取注册表失败: {e}"))?;
    match key.get_value::<String, _>(AUTOSTART_VALUE_NAME) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
    let key = run_key()?;
    if enabled {
        key.set_value(AUTOSTART_VALUE_NAME, &autostart_command()?)
            .map_err(|e| format!("写入注册表失败: {e}"))?;
    } else {
        let _ = key.delete_value(AUTOSTART_VALUE_NAME);
    }
    Ok(())
}

fn color_bg() -> Color {
    Color::from_rgb8(0xF4, 0xF6, 0xF9)
}

fn color_surface() -> Color {
    Color::from_rgb8(0xFF, 0xFF, 0xFF)
}

fn color_border() -> Color {
    Color::from_rgb8(0xE0, 0xE4, 0xEB)
}

fn color_text() -> Color {
    Color::from_rgb8(0x1A, 0x1D, 0x23)
}

fn color_text_sub() -> Color {
    Color::from_rgb8(0x6B, 0x72, 0x80)
}

fn color_success() -> Color {
    Color::from_rgb8(0x22, 0xC5, 0x5E)
}

fn color_danger() -> Color {
    Color::from_rgb8(0xEF, 0x44, 0x44)
}

fn card_style(_: &Theme) -> container_style::Style {
    container_style::Style {
        background: Some(Background::Color(color_surface())),
        text_color: Some(color_text()),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: color_border(),
        },
        ..Default::default()
    }
}

fn side_panel_style(_: &Theme) -> container_style::Style {
    container_style::Style {
        text_color: Some(color_text()),
        ..Default::default()
    }
}

fn header_style(_: &Theme) -> container_style::Style {
    container_style::Style {
        background: Some(Background::Color(color_surface())),
        text_color: Some(color_text()),
        border: Border {
            radius: 0.0.into(),
            width: 1.0,
            color: color_border(),
        },
        ..Default::default()
    }
}

fn app_shell_style(_: &Theme) -> container_style::Style {
    container_style::Style {
        background: Some(Background::Color(color_bg())),
        text_color: Some(color_text()),
        ..Default::default()
    }
}

fn metric_style(_: &Theme) -> container_style::Style {
    container_style::Style {
        background: Some(Background::Color(color_bg())),
        text_color: Some(color_text()),
        border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: color_border(),
        },
        ..Default::default()
    }
}

fn log_surface_style(_: &Theme) -> container_style::Style {
    container_style::Style {
        background: Some(Background::Color(Color::from_rgb8(0x0F, 0x11, 0x17))),
        text_color: Some(Color::from_rgb8(0xC9, 0xD1, 0xD9)),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: Color::from_rgb8(0x1F, 0x29, 0x37),
        },
        ..Default::default()
    }
}

fn error_box_style(_: &Theme) -> container_style::Style {
    container_style::Style {
        background: Some(Background::Color(Color::from_rgb8(0xFE, 0xE2, 0xE2))),
        text_color: Some(Color::from_rgb8(0xB9, 0x1C, 0x1C)),
        border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: Color::from_rgb8(0xFE, 0xCA, 0xCA),
        },
        ..Default::default()
    }
}

fn badge_style(kind: BannerKind) -> container_style::Style {
    let (background, text_color) = match kind {
        BannerKind::Success => (
            Color::from_rgb8(0xDC, 0xFC, 0xE7),
            Color::from_rgb8(0x15, 0x80, 0x3D),
        ),
        BannerKind::Error => (
            Color::from_rgb8(0xFE, 0xE2, 0xE2),
            Color::from_rgb8(0xB9, 0x1C, 0x1C),
        ),
        BannerKind::Info => (Color::from_rgb8(0xF1, 0xF5, 0xF9), color_text_sub()),
    };

    container_style::Style {
        background: Some(Background::Color(background)),
        text_color: Some(text_color),
        border: Border {
            radius: 12.0.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        ..Default::default()
    }
}

fn banner_style(kind: BannerKind) -> container_style::Style {
    let (background, border, text_color) = match kind {
        BannerKind::Success => (
            Color::from_rgb8(0xF0, 0xFD, 0xF4),
            Color::from_rgb8(0xBB, 0xF7, 0xD0),
            Color::from_rgb8(0x16, 0x6A, 0x3A),
        ),
        BannerKind::Error => (
            Color::from_rgb8(0xFE, 0xF2, 0xF2),
            Color::from_rgb8(0xFE, 0xCA, 0xCA),
            Color::from_rgb8(0xB9, 0x1C, 0x1C),
        ),
        BannerKind::Info => (
            Color::from_rgb8(0xEF, 0xF6, 0xFF),
            Color::from_rgb8(0xBF, 0xDB, 0xFE),
            Color::from_rgb8(0x1D, 0x4E, 0x89),
        ),
    };

    container_style::Style {
        background: Some(Background::Color(background)),
        text_color: Some(text_color),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: border,
        },
        ..Default::default()
    }
}

fn dot_style(color: Color) -> container_style::Style {
    container_style::Style {
        background: Some(Background::Color(color)),
        border: Border {
            radius: 7.0.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        ..Default::default()
    }
}

fn log_text_color(line: &str) -> Color {
    if line.contains('✓') {
        Color::from_rgb8(0x4A, 0xDE, 0x80)
    } else if line.contains('✗') || line.contains("失败") {
        Color::from_rgb8(0xF8, 0x71, 0x71)
    } else if line.contains("警告") || line.contains("warn") {
        Color::from_rgb8(0xFB, 0xBF, 0x24)
    } else if line.contains("等待") || line.contains("检查") {
        Color::from_rgb8(0x93, 0xC5, 0xFD)
    } else {
        Color::from_rgb8(0xC9, 0xD1, 0xD9)
    }
}

fn load_icon_from_ico(ico_data: &[u8]) -> tray_icon::Icon {
    // Parse ICO: header (6 bytes) + entries (16 bytes each)
    // Pick the largest image entry
    let count = u16::from_le_bytes([ico_data[4], ico_data[5]]) as usize;
    let mut best_idx = 0;
    let mut best_size: u32 = 0;
    for i in 0..count {
        let offset = 6 + i * 16;
        let w = if ico_data[offset] == 0 { 256u32 } else { ico_data[offset] as u32 };
        let h = if ico_data[offset + 1] == 0 { 256u32 } else { ico_data[offset + 1] as u32 };
        let size = w * h;
        if size > best_size {
            best_size = size;
            best_idx = i;
        }
    }
    let entry_offset = 6 + best_idx * 16;
    let img_size = u32::from_le_bytes([
        ico_data[entry_offset + 8],
        ico_data[entry_offset + 9],
        ico_data[entry_offset + 10],
        ico_data[entry_offset + 11],
    ]) as usize;
    let img_offset = u32::from_le_bytes([
        ico_data[entry_offset + 12],
        ico_data[entry_offset + 13],
        ico_data[entry_offset + 14],
        ico_data[entry_offset + 15],
    ]) as usize;

    let img_data = &ico_data[img_offset..img_offset + img_size];

    // Check if it's a PNG (starts with PNG magic)
    if img_data.len() > 8 && img_data[0..4] == [0x89, 0x50, 0x4E, 0x47] {
        // Decode PNG to RGBA
        let decoder = png::Decoder::new(img_data);
        let mut reader = decoder.read_info().expect("failed to read PNG info");
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("failed to decode PNG frame");
        let rgba = &buf[..info.buffer_size()];
        tray_icon::Icon::from_rgba(rgba.to_vec(), info.width, info.height)
            .expect("failed to create icon from RGBA")
    } else {
        // Fallback: try to use a simple 16x16 icon
        tray_icon::Icon::from_rgba(vec![0x40, 0x80, 0xC0, 0xFF].repeat(16 * 16), 16, 16)
            .expect("failed to create fallback icon")
    }
}

fn load_window_icon() -> window::Icon {
    let ico_data = include_bytes!("../../src-tauri/icons/icon.ico");
    let count = u16::from_le_bytes([ico_data[4], ico_data[5]]) as usize;
    let mut best_idx = 0;
    let mut best_size: u32 = 0;
    for i in 0..count {
        let offset = 6 + i * 16;
        let w = if ico_data[offset] == 0 { 256u32 } else { ico_data[offset] as u32 };
        let h = if ico_data[offset + 1] == 0 { 256u32 } else { ico_data[offset + 1] as u32 };
        let size = w * h;
        if size > best_size {
            best_size = size;
            best_idx = i;
        }
    }
    let entry_offset = 6 + best_idx * 16;
    let img_size = u32::from_le_bytes([
        ico_data[entry_offset + 8],
        ico_data[entry_offset + 9],
        ico_data[entry_offset + 10],
        ico_data[entry_offset + 11],
    ]) as usize;
    let img_offset = u32::from_le_bytes([
        ico_data[entry_offset + 12],
        ico_data[entry_offset + 13],
        ico_data[entry_offset + 14],
        ico_data[entry_offset + 15],
    ]) as usize;
    let img_data = &ico_data[img_offset..img_offset + img_size];

    if img_data.len() > 8 && img_data[0..4] == [0x89, 0x50, 0x4E, 0x47] {
        let decoder = png::Decoder::new(img_data);
        let mut reader = decoder.read_info().expect("failed to read PNG info");
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("failed to decode PNG frame");
        let rgba = &buf[..info.buffer_size()];
        window::icon::from_rgba(rgba.to_vec(), info.width, info.height)
            .expect("failed to create window icon")
    } else {
        window::icon::from_rgba(vec![0x40, 0x80, 0xC0, 0xFF].repeat(16 * 16), 16, 16)
            .expect("failed to create fallback window icon")
    }
}

fn main() -> iced::Result {
    let start_hidden = std::env::args().any(|arg| arg == "--minimized");

    iced::application(
        WindowsGuiApp::title,
        WindowsGuiApp::update,
        WindowsGuiApp::view,
    )
    .theme(WindowsGuiApp::theme)
    .subscription(WindowsGuiApp::subscription)
    .default_font(UI_FONT)
    .window(window::Settings {
        size: Size::new(1140.0, 760.0),
        min_size: Some(Size::new(920.0, 640.0)),
        visible: true,
        exit_on_close_request: false,
        icon: Some(load_window_icon()),
        ..Default::default()
    })
    .run_with(move || WindowsGuiApp::new(start_hidden))
}
