use crate::get_video_path;
use h264_reader::{
    annexb::AnnexBReader,
    nal::{Nal, RefNal},
    push::NalInterest,
};

use remote_desktop::{protocol::Packet, secure_channel::SecureChannel};

use std::{
    io::Read,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

/// Starts an `ffmpeg` process that encodes the video file with H.264.
///
/// # Arguments
///
/// * `filename` - The filename of the video (without the extension).
/// * `time_seconds` - The wanted starting time of the video.
///
/// # Returns
/// The subprocess `Child` object.
fn ffmpeg_send_recording(filename: &str, time_seconds: i32) -> Child {
    let input_path = get_video_path(filename);

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

/// Starts a thread to send the H.264 NAL units.
///
/// # Arguments
///
/// * `channel` - The `SecureChannel` to send to.
/// * `stdout` - The `ffmpeg` stdout to read the H.264 stream from.
/// * `stop_flag` - The flag that tells the thread to stop.
///
/// # Returns
///
/// The thread's `JoinHandle`.
fn thread_send_screen(
    mut channel: SecureChannel,
    mut stdout: ChildStdout,
    stop_flag: Arc<AtomicBool>,
) -> JoinHandle<std::io::Result<()>> {
    thread::spawn(move || -> std::io::Result<()> {
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

            let _ = channel.send(Packet::Screen { bytes: nal_bytes });

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

        channel.send(Packet::None)?;

        Ok(())
    })
}

/// Handles packets from the client.
///
/// # Arguments
/// * `channel` - The `SecureChannel` connected to the client.
/// * `filename` - The filename of the played video.
///
/// # Returns
///
/// An `std::io::Result<()>` that signifies if something went wrong.
pub fn handle_watching(channel: &mut SecureChannel, filename: &str) -> std::io::Result<()> {
    let mut ffmpeg = ffmpeg_send_recording(filename, 0);
    let mut stdout = ffmpeg.stdout.take().unwrap();

    let mut stop_flag = Arc::new(AtomicBool::new(false));

    let mut thread_send = Some(thread_send_screen(
        channel.clone(),
        stdout,
        stop_flag.clone(),
    ));

    loop {
        let packet = channel.receive().unwrap_or_default();

        match packet {
            Packet::SeekInit => {
                // Kill current ffmpeg and thread
                stop_flag.store(true, Ordering::Relaxed);
                let _ = ffmpeg.kill();
                let _ = thread_send.take().unwrap().join();

                // send session exit
                channel.send(Packet::SeekInit)?;
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

            Packet::SessionExit | Packet::None => {
                stop_flag.store(true, Ordering::Relaxed);

                let _ = ffmpeg.kill();
                let _ = thread_send.take().unwrap().join();

                channel.send(Packet::SessionExit)?;

                break;
            }

            _ => (),
        }
    }

    Ok(())
}
