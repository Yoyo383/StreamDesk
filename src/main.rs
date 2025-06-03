use std::net::TcpStream;
use std::sync::mpsc::{self, Sender};
use std::thread;

use eframe::egui::Visuals;
use eframe::{egui, NativeOptions};
use login_scene::LoginScene;
use remote_desktop::secure_channel::SecureChannel;
use remote_desktop::{initialize_logger, Scene, SceneChange, CLIENT_LOG_FILE, LOG_DIR};

mod host_scene;
mod login_scene;
mod menu_scene;
mod modifiers_state;
mod participant_scene;
mod watch_scene;

const SERVER_IP: &'static str = "127.0.0.1";
const SERVER_PORT: u16 = 7643;

/// Starts a thread to connect to the server.
///
/// When connected, it sends the new `SecureChannel` to the provided `sender`.
/// If the connection fails, `None` is sent instead.
///
/// # Arguments
///
/// * `sender` - A `mpsc::Sender<Option<SecureChannel>>` used to send the
///              result of the connection attempt back to the main application thread.
fn connect_to_server(sender: Sender<Option<SecureChannel>>) {
    thread::spawn(
        move || match TcpStream::connect(format!("{}:{}", SERVER_IP, SERVER_PORT)) {
            Ok(socket) => {
                let channel = SecureChannel::new_client(Some(socket)).unwrap();
                sender.send(Some(channel))
            }
            Err(_) => sender.send(None),
        },
    );
}

/// The main application struct for the Remote Desktop client.
///
/// It holds the **secure communication channel** to the server and manages
/// the **currently active scene**, allowing for transitions between different
/// application states.
struct MyApp {
    /// The secure communication channel used for all network interactions.
    channel: SecureChannel,
    /// The current active scene, dictating what is rendered and how input is handled.
    scene: Box<dyn Scene>,
}

impl MyApp {
    /// Creates a new instance of the `MyApp` application.
    ///
    /// This initializes the secure channel (unconnected initially) and
    /// starts a background thread to attempt connecting to the server.
    /// The initial scene is set to the **LoginScene**.
    ///
    /// # Returns
    ///
    /// A new `MyApp` instance.
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        connect_to_server(sender);

        let login = LoginScene::new(Some(receiver), false);

        Self {
            channel: SecureChannel::new_client(None).unwrap(),
            scene: Box::new(login),
        }
    }
}

impl eframe::App for MyApp {
    /// Called once per frame to update and render the application's UI.
    ///
    /// This method **delegates the update logic** to the current `scene`.
    /// If the scene requests a change, the `scene` field is updated.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The `egui::Context` providing access to egui's rendering and input state.
    /// * `_frame` - A mutable reference to the `eframe::Frame`, unused in this implementation.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let scene_change = self.scene.as_mut().update(ctx, &mut self.channel);
        if let SceneChange::To(scene) = scene_change {
            self.scene = scene;
        }
        ctx.request_repaint();
    }

    /// Called when the application is about to exit.
    ///
    /// This method gives the current scene an opportunity to perform any
    /// necessary **cleanup operations** before the application closes.
    ///
    /// # Arguments
    ///
    /// * `_gl` - An `Option` containing the `eframe::glow::Context`, unused in this implementation.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.scene.as_mut().on_exit(&mut self.channel);
    }
}

/// The entry point of the client application.
///
/// This function initializes the `eframe` application, sets up window properties,
/// and runs the `MyApp` instance.
fn main() {
    let _ = std::fs::create_dir(LOG_DIR);

    initialize_logger(CLIENT_LOG_FILE);

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
