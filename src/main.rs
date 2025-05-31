use std::net::TcpStream;
use std::sync::mpsc::{self, Sender};
use std::thread;

use eframe::egui::Visuals;
use eframe::{egui, NativeOptions};
use login_scene::LoginScene;
use remote_desktop::secure_channel::SecureChannel;
use remote_desktop::{Scene, SceneChange};

mod host_scene;
mod login_scene;
mod menu_scene;
mod modifiers_state;
mod participant_scene;
mod watch_scene;

/// Starts a thread to connect to the server. When connected, sends the new `SecureChannel` to the sender.
/// If it could not connect to the server, sends `None` to the sender.
fn connect_to_server(sender: Sender<Option<SecureChannel>>) {
    thread::spawn(move || match TcpStream::connect("127.0.0.1:7643") {
        Ok(socket) => {
            let channel = SecureChannel::new_client(socket).unwrap();
            sender.send(Some(channel))
        }

        Err(_) => sender.send(None),
    });
}

/// The app struct. Has the `SecureChannel` and the current `Scene`.
struct MyApp {
    channel: Option<SecureChannel>,
    scene: Box<dyn Scene>,
}

impl MyApp {
    /// Creates a new app and starts connecting to the server.
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        connect_to_server(sender);

        let login = LoginScene::new(Some(receiver), false);

        Self {
            channel: None,
            scene: Box::new(login),
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let scene_change = self.scene.as_mut().update(ctx, &mut self.channel);
        match scene_change {
            SceneChange::To(scene) => self.scene = scene,
            _ => (),
        }

        ctx.request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.scene.as_mut().on_exit(&mut self.channel);
    }
}

/// The main function.
fn main() {
    let (width, height): (f32, f32) = (600.0 * 1920.0 / 1080.0, 600.0);
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([width, height]),
        hardware_acceleration: eframe::HardwareAcceleration::Preferred,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Remote Desktop",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(Visuals::dark());
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(MyApp::new()))
        }),
    );
}
