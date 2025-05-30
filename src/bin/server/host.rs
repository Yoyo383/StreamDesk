use crate::{get_video_path, structs::*, SessionHashMap, SharedSession};
use chrono::Local;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
    UserType,
};
use rusqlite::params;
use std::{
    io::Write,
    process::{Child, Command, Stdio},
};

fn ffmpeg_save_recording(filename: &str) -> Child {
    let output_path = get_video_path(filename);

    let ffmpeg = Command::new("ffmpeg")
        .args([
            "-f",
            "h264",
            "-i",
            "-",
            "-c",
            "copy",
            output_path.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn ffmpeg");

    ffmpeg
}

fn insert_recording_to_database(
    db_pool: &Pool<SqliteConnectionManager>,
    filename: &str,
    time: &str,
    user_id: i32,
) {
    let _ = db_pool.get().unwrap().execute(
        "INSERT INTO recordings (filename, time, user_id) VALUES (?1, ?2, ?3)",
        params![filename, time, user_id],
    );
}

pub fn handle_host(
    channel: &mut SecureChannel,
    session: SharedSession,
    sessions: SessionHashMap,
    code: u32,
    username: String,
    user_id: i32,
    db_pool: &Pool<SqliteConnectionManager>,
) -> std::io::Result<()> {
    let time = Local::now().to_rfc3339();
    let filename = uuid::Uuid::new_v4().to_string();

    let mut ffmpeg = ffmpeg_save_recording(&filename);
    let mut stdin = ffmpeg.stdin.take().unwrap();

    loop {
        let packet = channel.receive().unwrap_or_default();

        match packet {
            Packet::Join { username, .. } => {
                let mut session = session.lock().unwrap();

                if let Some(mut connection) = session.pending_join.remove(&username) {
                    // notify user they were allowed
                    let success = ResultPacket::Success("Joining".to_string());
                    connection.channel.send(success)?;

                    // notify user thread
                    let _ = connection.join_request_sender.take().unwrap().send(true);

                    // send all usernames
                    for (username, user_connection) in &session.connections {
                        let username_packet = Packet::UserUpdate {
                            user_type: user_connection.user_type,
                            joined_before: true,
                            username: username.clone(),
                        };
                        connection.channel.send(username_packet)?;
                    }

                    session.connections.insert(username.clone(), connection);

                    // send new username to all participants
                    let packet = Packet::UserUpdate {
                        user_type: UserType::Participant,
                        joined_before: false,
                        username: username.clone(),
                    };
                    session.broadcast_all(packet)?;
                }
            }

            Packet::DenyJoin { username } => {
                let mut session = session.lock().unwrap();

                if let Some(connection) = session.pending_join.get_mut(&username) {
                    // notify user they were denied
                    let failure = ResultPacket::Failure("You were denied by the host.".to_string());
                    connection.channel.send(failure)?;

                    // notify user thread
                    let _ = connection.join_request_sender.take().unwrap().send(false);
                }

                // remove from pending
                session.pending_join.remove(&username);
            }

            Packet::Screen { ref bytes } => {
                stdin.write_all(&bytes).unwrap();
                let mut session = session.lock().unwrap();
                session.broadcast_participants(packet)?;
            }

            Packet::MergeUnready => {
                let mut session = session.lock().unwrap();

                for (_, connection) in &mut session.connections {
                    if connection.connection_type == ConnectionType::Unready {
                        connection.connection_type = ConnectionType::Participant;
                    }
                }
            }

            Packet::SessionExit | Packet::None => {
                let mut session = session.lock().unwrap();

                let packet = Packet::SessionEnd;
                session.broadcast_all(packet)?;

                let mut sessions = sessions.lock().unwrap();
                sessions.remove(&code);
                break;
            }

            Packet::RequestControl { username } => {
                let mut session = session.lock().unwrap();
                if let Some(user_connection) = session.connections.get_mut(&username) {
                    user_connection.connection_type = ConnectionType::Controller;
                    user_connection.user_type = UserType::Controller;

                    let packet = Packet::RequestControl {
                        username: username.clone(),
                    };
                    user_connection.channel.send(packet)?;

                    // notify all users
                    let user_update = Packet::UserUpdate {
                        user_type: UserType::Controller,
                        joined_before: true,
                        username: username.to_string(),
                    };
                    session.broadcast_all(user_update)?;
                }
            }

            Packet::DenyControl { username } => {
                let mut session = session.lock().unwrap();
                if let Some(user_connection) = session.connections.get_mut(&username) {
                    let was_controller =
                        user_connection.connection_type == ConnectionType::Controller;

                    user_connection.connection_type = ConnectionType::Participant;
                    user_connection.user_type = UserType::Participant;

                    let packet = Packet::DenyControl {
                        username: username.clone(),
                    };
                    user_connection.channel.send(packet)?;

                    // if the user is a controller notify all users
                    if was_controller {
                        let user_update = Packet::UserUpdate {
                            user_type: UserType::Participant,
                            joined_before: true,
                            username: username.to_string(),
                        };
                        session.broadcast_all(user_update)?;
                    }
                }
            }

            Packet::Chat { message } => {
                let message = username.to_string() + ": " + &message;
                let packet = Packet::Chat { message };

                let mut session = session.lock().unwrap();
                session.broadcast_all(packet)?;
            }

            _ => (),
        }
    }

    drop(stdin);
    let _ = ffmpeg.wait();

    insert_recording_to_database(db_pool, &filename, &time, user_id);

    Ok(())
}
