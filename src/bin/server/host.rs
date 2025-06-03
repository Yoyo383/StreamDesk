use crate::{get_video_path, SessionHashMap, SharedSession};
use chrono::Local;

use log::info;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
    UserType, LOG_TARGET,
};
use rusqlite::params;
use std::{
    io::Write,
    process::{Child, Command, Stdio},
};

/// Starts an `ffmpeg` process that saves the recording to file at the end of the session.
///
/// # Arguments
///
/// * `filename` - The filename to save the video to.
///
/// # Returns
///
/// The subprocess `Child` object.
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

/// Inserts the recording data to the database.
///
/// # Arguments
/// * `db_pool` - The pool of the database connections.
/// * `filename` - The filename of the video.
/// * `time` - The timestamp of the meeting.
/// * `user_id` - The ID of the user this recording belongs to.
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

/// Handles packets from the client.
///
/// # Arguments
///
/// * `channel` - A `SecureChannel` connected to the client.
/// * `session` - The `Session` object that the user is connected to.
/// * `sessions` - The `HashMap` of all the sessions.
/// * `code` - The session code.
/// * `username` - The username of the client.
/// * `user_id` - The user's ID in the database.
/// * `db_pool` - The pool of the database connections.
///
/// # Returns
///
/// An `std::io::Result<()>` that signifies if something went wrong.
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

                if let Some((mut connection, join_sender)) = session.pending_join.remove(&username)
                {
                    // notify user they were allowed
                    let success = ResultPacket::Success("Joining".to_string());
                    connection.channel.send(success)?;

                    // notify user thread
                    let _ = join_sender.send(true);

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

                if let Some((connection, join_sender)) = session.pending_join.get_mut(&username) {
                    // notify user they were denied
                    let failure = ResultPacket::Failure("You were denied by the host.".to_string());
                    connection.channel.send(failure)?;

                    // notify user thread
                    let _ = join_sender.send(false);
                }

                // remove from pending
                session.pending_join.remove(&username);

                info!(target: LOG_TARGET, "User {} was denied from session {}.", username, code);
            }

            Packet::Screen { ref bytes } => {
                stdin.write_all(&bytes).unwrap();
                let mut session = session.lock().unwrap();
                session.broadcast_participants(packet)?;
            }

            Packet::SessionExit | Packet::None => {
                let mut session = session.lock().unwrap();

                let packet = Packet::SessionEnd;
                session.broadcast_all(packet)?;

                let mut sessions = sessions.lock().unwrap();
                sessions.remove(&code);

                info!(target: LOG_TARGET, "Host ended session {}.", code);

                break;
            }

            Packet::RequestControl { username } => {
                let mut session = session.lock().unwrap();
                if let Some(user_connection) = session.connections.get_mut(&username) {
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

                    info!(
                        target: LOG_TARGET,
                        "User {} is now the Controller of session {}.",
                        username, code
                    );
                }
            }

            Packet::DenyControl { username } => {
                let mut session = session.lock().unwrap();
                if let Some(user_connection) = session.connections.get_mut(&username) {
                    let was_controller = user_connection.user_type == UserType::Controller;

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

                        info!(
                            target: LOG_TARGET,
                            "User {} is no longer the Controller of session {}.",
                            username, code
                        );
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
