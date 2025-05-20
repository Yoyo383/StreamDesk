use crate::{structs::*, SessionHashMap, SharedSession, RECORDINGS_FOLDER};
use chrono::Local;

use remote_desktop::{
    protocol::{Packet, ResultPacket},
    UserType,
};
use rusqlite::params;
use std::{
    io::Write,
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

fn ffmpeg_save_recording(filename: &str) -> Child {
    let output_path = PathBuf::from(RECORDINGS_FOLDER).join(format!("{filename}.mp4"));

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
    conn: &rusqlite::Connection,
    filename: &str,
    time: &str,
    user_id: i32,
) {
    let _ = conn.execute(
        "INSERT INTO recordings (filename, time, user_id) VALUES (?1, ?2, ?3)",
        params![filename, time, user_id],
    );
}

pub fn handle_host(
    socket: &mut TcpStream,
    session: SharedSession,
    sessions: SessionHashMap,
    code: u32,
    username: String,
    user_id: i32,
    conn: &rusqlite::Connection,
) {
    let time = Local::now().to_rfc3339();
    let filename = uuid::Uuid::new_v4().to_string();

    let mut ffmpeg = ffmpeg_save_recording(&filename);
    let mut stdin = ffmpeg.stdin.take().unwrap();

    loop {
        let packet = Packet::receive(socket).unwrap();

        match packet {
            Packet::Join { username, .. } => {
                let mut session = session.lock().unwrap();

                if let Some(mut connection) = session.pending_join.remove(&username) {
                    // notify user they were allowed
                    let success = ResultPacket::Success("Joining".to_string());
                    success.send(&mut connection.socket).unwrap();

                    // notify user thread
                    let _ = connection.join_request_sender.take().unwrap().send(true);

                    // send all usernames
                    for (username, user_connection) in &session.connections {
                        let username_packet = Packet::UserUpdate {
                            user_type: user_connection.user_type,
                            joined_before: true,
                            username: username.clone(),
                        };
                        username_packet.send(&mut connection.socket).unwrap();
                    }

                    session.connections.insert(username.clone(), connection);

                    // send new username to all participants
                    let packet = Packet::UserUpdate {
                        user_type: UserType::Participant,
                        joined_before: false,
                        username: username.clone(),
                    };
                    session.broadcast_all(packet);
                }
            }

            Packet::DenyJoin { username } => {
                let mut session = session.lock().unwrap();

                if let Some(connection) = session.pending_join.get_mut(&username) {
                    // notify user they were denied
                    let failure = ResultPacket::Failure("You were denied by the host.".to_string());
                    failure.send(&mut connection.socket).unwrap();

                    // notify user thread
                    let _ = connection.join_request_sender.take().unwrap().send(false);
                }

                // remove from pending
                session.pending_join.remove(&username);
            }

            Packet::Screen { ref bytes } => {
                stdin.write_all(&bytes).unwrap();
                let mut session = session.lock().unwrap();
                session.broadcast_participants(packet);
            }

            Packet::MergeUnready => {
                let mut session = session.lock().unwrap();

                for (_, connection) in &mut session.connections {
                    if connection.connection_type == ConnectionType::Unready {
                        connection.connection_type = ConnectionType::Participant;
                    }
                }
            }

            Packet::SessionExit => {
                let mut session = session.lock().unwrap();

                let packet = Packet::SessionEnd;
                session.broadcast_all(packet);

                let mut sessions = sessions.lock().unwrap();
                sessions.remove(&code);
                break;
            }

            Packet::RequestControl { ref username } => {
                let mut session = session.lock().unwrap();
                if let Some(user_connection) = session.connections.get_mut(username) {
                    user_connection.connection_type = ConnectionType::Controller;
                    user_connection.user_type = UserType::Controller;
                    packet.send(&mut user_connection.socket).unwrap();

                    // notify all users
                    let user_update = Packet::UserUpdate {
                        user_type: UserType::Controller,
                        joined_before: false,
                        username: username.to_string(),
                    };
                    session.broadcast_all(user_update);
                }
            }

            Packet::DenyControl { ref username } => {
                let mut session = session.lock().unwrap();
                if let Some(user_connection) = session.connections.get_mut(username) {
                    user_connection.connection_type = ConnectionType::Participant;
                    user_connection.user_type = UserType::Participant;
                    packet.send(&mut user_connection.socket).unwrap();

                    // notify all users
                    let user_update = Packet::UserUpdate {
                        user_type: UserType::Participant,
                        joined_before: false,
                        username: username.to_string(),
                    };
                    session.broadcast_all(user_update);
                }
            }

            Packet::Chat { message } => {
                let message = username.to_string() + ": " + &message;
                let packet = Packet::Chat { message };

                let mut session = session.lock().unwrap();
                session.broadcast_all(packet);
            }

            _ => (),
        }
    }

    drop(stdin);
    let _ = ffmpeg.wait();

    insert_recording_to_database(conn, &filename, &time, user_id);
}
