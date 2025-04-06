use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, ChildStdin, Command, Stdio},
    thread,
    time::Instant,
};

fn start_ffmpeg() -> Child {
    let ffmpeg = Command::new("ffmpeg")
        .args([
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            "1920x1080",
            "-r",
            "30",
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-f",
            "h264", // Output raw H.264 stream
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start ffmpeg");

    ffmpeg
}

fn start_screenshot_thread(mut stdin: ChildStdin) {
    thread::spawn(move || {
        let mut monitor_option = None;
        for mon in xcap::Monitor::all().unwrap() {
            if mon.is_primary().unwrap() {
                monitor_option = Some(mon);
                break;
            }
        }

        let monitor = monitor_option.unwrap();
        loop {
            let original_image = monitor.capture_image().unwrap();
            let raw_pixels = original_image.as_raw();

            stdin.write_all(&raw_pixels).unwrap();
        }
    });
}

fn handle_connection(mut socket: TcpStream) {
    let mut command = start_ffmpeg();
    let stdin = command.stdin.take().unwrap();
    let mut stdout = command.stdout.take().unwrap();

    start_screenshot_thread(stdin);

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
                // println!("{} image size: {}", 1. / dt, n);
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
