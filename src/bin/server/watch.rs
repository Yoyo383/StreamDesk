use crate::RECORDINGS_FOLDER;
use h264_reader::{
    annexb::AnnexBReader,
    nal::{Nal, RefNal},
    push::NalInterest,
};

use remote_desktop::{protocol::Packet, secure_channel::SecureChannel};

use std::{
    io::Read,
    path::PathBuf,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

fn ffmpeg_send_recording(filename: &str, time_seconds: i32) -> Child {
    let input_path = PathBuf::from(RECORDINGS_FOLDER).join(format!("{filename}.mp4"));

    let ffmpeg = Command::new("ffmpeg")
        .args(&[
            "-ss",
            &time_seconds.to_string(),
            "-i",
            input_path.to_str().unwrap(),
            "-vcodec",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-force_key_frames",
            "expr:gte(t,0)", // Force keyframe at the beginning
            "-f",
            "h264",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start FFmpeg");

    ffmpeg
}

fn thread_send_screen(
    mut channel: SecureChannel,
    mut stdout: ChildStdout,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = AnnexBReader::accumulate(|nal: RefNal<'_>| {
            if !nal.is_complete() {
                return NalInterest::Buffer;
            }

            // getting nal unit type
            match nal.header() {
                Ok(_) => (),
                Err(_) => return NalInterest::Ignore,
            };

            // sending the NAL (with the start)
            let mut nal_bytes: Vec<u8> = vec![0x00, 0x00, 0x01];
            nal.reader()
                .read_to_end(&mut nal_bytes)
                .expect("should be able to read NAL");

            channel.send(Packet::Screen { bytes: nal_bytes }).unwrap();

            NalInterest::Ignore
        });

        let mut buffer = [0u8; 4096];

        while !stop_flag.load(Ordering::Relaxed) {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    reader.push(&buffer[..n]);
                }
                Err(e) => {
                    eprintln!("ffmpeg read error: {}", e);
                    break;
                }
            }
        }

        channel.send(Packet::None).unwrap();
    })
}

pub fn handle_watching(channel: &mut SecureChannel, filename: &str) {
    let mut ffmpeg = ffmpeg_send_recording(filename, 0);
    let mut stdout = ffmpeg.stdout.take().unwrap();

    let mut stop_flag = Arc::new(AtomicBool::new(false));

    let mut thread_send = Some(thread_send_screen(
        channel.clone(),
        stdout,
        stop_flag.clone(),
    ));

    loop {
        let packet = channel.receive().unwrap();

        match packet {
            Packet::SeekInit => {
                // Kill current ffmpeg and thread
                stop_flag.store(true, Ordering::Relaxed);
                let _ = ffmpeg.kill();
                let _ = thread_send.take().unwrap().join();

                // send session exit
                channel.send(Packet::SessionExit).unwrap();
            }

            Packet::SeekTo { time_seconds } => {
                // Restart with seek
                ffmpeg = ffmpeg_send_recording(filename, time_seconds);
                stdout = ffmpeg.stdout.take().unwrap();
                stop_flag = Arc::new(AtomicBool::new(false));
                thread_send = Some(thread_send_screen(
                    channel.clone(),
                    stdout,
                    stop_flag.clone(),
                ));
            }

            Packet::SessionExit => {
                stop_flag.store(true, Ordering::Relaxed);

                let _ = ffmpeg.kill();
                let _ = thread_send.take().unwrap().join();

                channel.send(Packet::SessionExit).unwrap();

                break;
            }

            _ => (),
        }
    }
}
