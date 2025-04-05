use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    time::Instant,
};

fn handle_connection(mut socket: TcpStream) {
    let mut monitor_option = None;
    for mon in xcap::Monitor::all().unwrap() {
        if mon.is_primary().unwrap() {
            monitor_option = Some(mon);
            break;
        }
    }

    let monitor = monitor_option.unwrap();

    let mut now = Instant::now();

    loop {
        let new_now = Instant::now();
        let dt = new_now.duration_since(now).as_secs_f32();
        now = new_now;

        let original_image = monitor.capture_image().unwrap();
        let raw_pixels = original_image.as_raw();

        socket
            .write(&(raw_pixels.len() as u64).to_be_bytes())
            .unwrap();

        socket
            .write_all(&raw_pixels)
            .expect("Could not send to client");

        // println!("{}", 1. / dt);
    }
}

fn main() {
    let listener = TcpListener::bind("0.0.0.0:7643").expect("Could not bind listener");
    match listener.accept() {
        Ok((socket, _addr)) => handle_connection(socket),
        Err(e) => println!("Couldn't accept client: {e:?}"),
    }
}
