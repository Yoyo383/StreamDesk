use std::net::TcpStream;
use std::sync::mpsc::{self, Sender};
use std::thread;

use eframe::{egui, NativeOptions};
use login_scene::LoginScene;
use remote_desktop::secure_channel::SecureChannel;
use remote_desktop::{Scene, SceneChange};

mod host_scene;
mod login_scene;
mod main_scene;
mod menu_scene;
mod modifiers_state;
mod watch_scene;

fn connect_to_server(sender: Sender<Option<SecureChannel>>) {
    thread::spawn(move || match TcpStream::connect("127.0.0.1:7643") {
        Ok(socket) => {
            let channel = SecureChannel::new_client(socket).unwrap();
            sender.send(Some(channel))
        }

        Err(_) => sender.send(None),
    });
}

struct MyApp {
    channel: Option<SecureChannel>,
    scene: Box<dyn Scene>,
}

impl MyApp {
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

fn main() {
    let (width, height): (f32, f32) = (600.0 * 1920.0 / 1080.0, 600.0);
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([width, height]),
        hardware_acceleration: eframe::HardwareAcceleration::Preferred,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Screen Capture",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(MyApp::new()))
        }),
    );
}
