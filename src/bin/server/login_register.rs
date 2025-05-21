use remote_desktop::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
};
use rusqlite::{ffi::SQLITE_CONSTRAINT_UNIQUE, params, Error::SqliteFailure};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

pub fn login_or_register(
    packet: Packet,
    channel: &mut SecureChannel,
    conn: &rusqlite::Connection,
    logged_in_users: Arc<Mutex<HashSet<String>>>,
) -> Option<(String, i32)> {
    match packet {
        Packet::Shutdown => None,

        Packet::Login { username, password } => {
            let user_id_result: Result<i32, rusqlite::Error> = conn.query_row(
                "SELECT user_id FROM users WHERE username = ?1 AND password = ?2",
                params![username, password],
                |row| row.get(0),
            );

            match user_id_result {
                Ok(user_id) => {
                    if !logged_in_users.lock().unwrap().contains(&username) {
                        logged_in_users.lock().unwrap().insert(username.clone());

                        let result = ResultPacket::Success("Signing in".to_owned());
                        channel.send(result).unwrap();

                        Some((username, user_id))
                    } else {
                        let result =
                            ResultPacket::Failure("User already logged in elsewhere.".to_owned());
                        channel.send(result).unwrap();
                        None
                    }
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    let result =
                        ResultPacket::Failure("Username or password are incorrect.".to_owned());
                    channel.send(result).unwrap();
                    None
                }
                _ => {
                    let result = ResultPacket::Failure("Error signing in.".to_owned());
                    channel.send(result).unwrap();
                    None
                }
            }
        }

        Packet::Register { username, password } => {
            // validate credentials
            if username.is_empty() {
                let result = ResultPacket::Failure("Username cannot be empty.".to_string());
                channel.send(result).unwrap();

                return None;
            }

            if username.chars().any(|c| !c.is_ascii_alphanumeric()) {
                let result = ResultPacket::Failure(
                    "Username can only contain English letters and numbers.".to_string(),
                );
                channel.send(result).unwrap();

                return None;
            }

            let inserted = conn.execute(
                "INSERT INTO users (username, password) VALUES (?1, ?2)",
                params![username, password],
            );

            match inserted {
                Ok(_) => {
                    logged_in_users.lock().unwrap().insert(username.clone());

                    let result = ResultPacket::Success("Signing in".to_owned());
                    channel.send(result).unwrap();

                    let user_id = conn.last_insert_rowid() as i32;

                    Some((username, user_id))
                }
                Err(SqliteFailure(e, _)) if e.extended_code == SQLITE_CONSTRAINT_UNIQUE => {
                    let result = ResultPacket::Failure("Username already taken.".to_owned());
                    channel.send(result).unwrap();
                    None
                }
                _ => {
                    let result = ResultPacket::Failure("Error signing up.".to_owned());
                    channel.send(result).unwrap();
                    None
                }
            }
        }

        _ => None,
    }
}
