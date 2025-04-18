use eframe::egui::Context;
use eframe::{egui, NativeOptions};
use menu_scene::MenuScene;
use remote_desktop::{AppData, Scene, SceneChange};

mod host_scene;
mod main_scene;
mod menu_scene;
mod modifiers_state;

struct MyApp {
    data: AppData,
    scene: Box<dyn Scene>,
}

impl MyApp {
    fn new(width: f32, height: f32) -> Self {
        let data: AppData = AppData {
            socket: None,
            width,
            height,
        };

        //let main_scene = MainScene::new(&mut data);
        let menu = MenuScene::new();

        Self {
            data,
            scene: Box::new(menu),
        }
    }

    fn update_size(&mut self, ctx: &Context) {
        let size = ctx.screen_rect().size();
        if self.data.width != size.x {
            self.data.width = size.x;
        }
        if self.data.height != size.y {
            self.data.height = size.y;
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_size(ctx);

        if let Some(scene_change) = self.scene.as_mut().update(ctx, &mut self.data) {
            match scene_change {
                SceneChange::To(scene) => self.scene = scene,
                SceneChange::Quit => (),
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.scene.as_mut().on_exit(&mut self.data);
    }
}

fn main() {
    let (width, height): (f32, f32) = (600.0 * 1920.0 / 1080.0, 600.0);
    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0 * 1920.0 / 1080.0, 600.0]),
        hardware_acceleration: eframe::HardwareAcceleration::Preferred,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Screen Capture",
        options,
        Box::new(move |_| Ok(Box::new(MyApp::new(width, height)))),
    );
}
