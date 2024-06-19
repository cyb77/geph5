#![windows_subsystem = "windows"]

mod daemon;
mod l10n;
mod logs;
mod pac;
mod prefs;
mod refresh_cell;
mod settings;
mod store_cell;
mod tabs;
mod timeseries;

use std::time::Duration;

use daemon::{stop_daemon, DAEMON, TOTAL_BYTES_TIMESERIES};
use egui::{FontData, FontDefinitions, FontFamily, IconData, Visuals};
use l10n::l10n;
use logs::LogLayer;
use native_dialog::MessageType;
use prefs::{pref_read, pref_write};
use settings::USERNAME;
use single_instance::SingleInstance;
use tabs::{dashboard::Dashboard, login::Login, logs::Logs, settings::render_settings};
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt, EnvFilter};
use tray_icon::{Icon, TrayIconBuilder};

// 0123456789

fn main() {
    const IMAGE_DATA: &[u8] = include_bytes!("../icon.png");
    let img = image::load_from_memory(IMAGE_DATA).unwrap();
    let tray_icon = TrayIconBuilder::new()
        .with_tooltip("system-tray - tray icon library!")
        .with_icon(
            Icon::from_rgba(img.as_rgba8().unwrap().to_vec(), img.width(), img.height()).unwrap(),
        )
        .build()
        .unwrap();
    tray_icon.set_visible(true).unwrap();
    let instance = SingleInstance::new("geph5-client-gui").unwrap();
    if !instance.is_single() {
        native_dialog::MessageDialog::new()
            .set_type(MessageType::Error)
            .set_text(l10n("geph_already_running"))
            .set_title("Error")
            .show_alert()
            .unwrap();
        std::process::exit(-1)
    }

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .with_writer(std::io::stderr),
        )
        .with(
            EnvFilter::builder()
                .with_default_directive("geph5=debug".parse().unwrap())
                .from_env_lossy(),
        )
        .with(LogLayer)
        .init();
    // default prefs
    for (key, value) in [("lang", "en")] {
        if pref_read(key).is_err() {
            pref_write(key, value).unwrap();
        }
    }

    let (icon_rgba, icon_width, icon_height) = {
        let icon = include_bytes!("../icon.ico");
        let image = image::load_from_memory(icon)
            .expect("Failed to open icon path")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([320.0, 320.0])
            .with_min_inner_size([320.0, 320.0])
            .with_icon(IconData {
                rgba: icon_rgba,
                width: icon_width,
                height: icon_height,
            }),
        ..Default::default()
    };
    eframe::run_native(
        l10n("geph"),
        native_options,
        Box::new(|cc| Box::new(App::new(cc))),
    )
    .unwrap();
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TabName {
    Dashboard,
    Logs,
    Settings,
}

pub struct App {
    selected_tab: TabName,
    login: Login,

    dashboard: Dashboard,
    logs: Logs,
}

impl App {
    /// Constructs the app.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        // set up fonts. currently this uses SC for CJK, but this can be autodetected instead.
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "normal".into(),
            FontData::from_static(include_bytes!("assets/normal.otf")),
        );
        fonts.font_data.insert(
            "chinese".into(),
            FontData::from_static(include_bytes!("assets/chinese.ttf")),
        );
        // fonts.font_data.insert(
        //     "persian".into(),
        //     FontData::from_static(include_bytes!("assets/persian.ttf")),
        // );
        {
            let fonts = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
            fonts.insert(0, "chinese".into());
            // fonts.insert(0, "persian".into());
            fonts.insert(0, "normal".into());
        }

        cc.egui_ctx.set_fonts(fonts);
        cc.egui_ctx.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);

            // style.spacing.button_padding = egui::vec2(5.0, 4.0);
            style.visuals = Visuals::light();
        });

        Self {
            selected_tab: TabName::Dashboard,
            login: Login::new(),

            dashboard: Dashboard::new(),
            logs: Logs::new(),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.set_zoom_factor(1.1);
        ctx.request_repaint_after(Duration::from_millis(200));

        {
            let daemon = DAEMON.lock();
            if let Some(daemon) = daemon.as_ref() {
                TOTAL_BYTES_TIMESERIES.record(daemon.total_rx_bytes());
            }
        }

        if USERNAME.get().is_empty() {
            egui::CentralPanel::default().show(ctx, |ui| {
                self.login.render(ui).unwrap();
            });

            return;
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.selected_tab,
                    TabName::Dashboard,
                    l10n("dashboard"),
                );
                ui.selectable_value(&mut self.selected_tab, TabName::Logs, l10n("logs"));
                ui.selectable_value(&mut self.selected_tab, TabName::Settings, l10n("settings"));
            });
        });

        let result = egui::CentralPanel::default().show(ctx, |ui| match self.selected_tab {
            TabName::Dashboard => self.dashboard.render(ui),
            TabName::Logs => self.logs.render(ui),
            TabName::Settings => {
                egui::ScrollArea::vertical()
                    .show(ui, |ui| render_settings(ctx, ui))
                    .inner
            }
        });

        if let Err(err) = result.inner {
            let _ = native_dialog::MessageDialog::new()
                .set_title("Fatal error")
                .set_text(&format!(
                    "Unfortunately, a fatal error occurred, so Geph must die:\n\n{:?}",
                    err
                ))
                .set_type(MessageType::Error)
                .show_alert();
            std::process::exit(-1);
        }
    }

    fn on_exit(&mut self) {
        // stop the daemon, unset the proxies, etc
        let _ = stop_daemon();
    }
}
