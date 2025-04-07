use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::Instant,
};

fn start_ffmpeg() -> Child {
    let ffmpeg = Command::new("ffmpeg")
        .args(&[
            "-f",
            "gdigrab",
            "-framerate",
            "30",
            "-draw_mouse",
            "0",
            "-i",
            "desktop",
            "-vcodec",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-x264opts",
            "no-scenecut",
            "-sc_threshold",
            "0",
            "-f",
            "h264", // Send proper H.264 stream
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start FFmpeg");

    ffmpeg
}

fn handle_connection(mut socket: TcpStream) {
    let mut command = start_ffmpeg();
    let mut stdout = command.stdout.take().unwrap();

    let mut now = Instant::now();

    let mut buffer = [0u8; 4096];

    loop {
        let new_now = Instant::now();
        let dt = new_now.duration_since(now).as_secs_f32();
        now = new_now;

        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                socket.write_all(&(n as u64).to_be_bytes()).unwrap();
                socket.write_all(&buffer[..n]).unwrap();
            }
            Err(e) => {
                eprintln!("ffmpeg read error: {}", e);
                break;
            }
        }
    }
}

fn main() {
    let listener = TcpListener::bind("0.0.0.0:7643").expect("Could not bind listener");
    match listener.accept() {
        Ok((socket, _addr)) => handle_connection(socket),
        Err(e) => println!("Couldn't accept client: {e:?}"),
    }
}
