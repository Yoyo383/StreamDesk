use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
};
use rusqlite::{ffi::SQLITE_CONSTRAINT_UNIQUE, params, Error::SqliteFailure};

/// Handles logging in and registering to the server.
///
/// # Arguments
///
/// * `packet` - The packet received from the client.
/// * `channel` - The `SecureChannel` connected to the client.
/// * `db_pool` - The database connection pool.
///
/// # Returns
///
/// An `std::io::Result<Option<(String, i32)>>` that represents the username and user_id,
/// and the error if there was.
pub fn login_or_register(
    packet: Packet,
    channel: &mut SecureChannel,
    db_pool: &Pool<SqliteConnectionManager>,
) -> std::io::Result<Option<(String, i32)>> {
    match packet {
        Packet::Shutdown => Ok(None),

        Packet::Login { username, password } => {
            let user_id_result: Result<i32, rusqlite::Error> = db_pool.get().unwrap().query_row(
                "SELECT user_id FROM users WHERE username = ?1 AND password = ?2",
                params![username, password],
                |row| row.get(0),
            );

            match user_id_result {
                Ok(user_id) => {
                    let result = ResultPacket::Success("Signing in".to_owned());
                    channel.send(result)?;

                    Ok(Some((username, user_id)))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    let result =
                        ResultPacket::Failure("Username or password are incorrect.".to_owned());
                    channel.send(result)?;
                    Ok(None)
                }
                _ => {
                    let result = ResultPacket::Failure("Error signing in.".to_owned());
                    channel.send(result)?;
                    Ok(None)
                }
            }
        }

        Packet::Register { username, password } => {
            // validate credentials
            if username.is_empty() {
                let result = ResultPacket::Failure("Username cannot be empty.".to_string());
                channel.send(result)?;

                return Ok(None);
            }

            if username.chars().any(|c| !c.is_ascii_alphanumeric()) {
                let result = ResultPacket::Failure(
                    "Username can only contain English letters and numbers.".to_string(),
                );
                channel.send(result)?;

                return Ok(None);
            }

            if username.len() > 20 {
                let result = ResultPacket::Failure(
                    "Username cannot be longer than 20 characters.".to_string(),
                );
                channel.send(result)?;

                return Ok(None);
            }

            let inserted = db_pool.get().unwrap().execute(
                "INSERT INTO users (username, password) VALUES (?1, ?2)",
                params![username, password],
            );

            match inserted {
                Ok(_) => {
                    let result = ResultPacket::Success("Signing in".to_owned());
                    channel.send(result)?;

                    let user_id = db_pool.get().unwrap().last_insert_rowid() as i32;

                    Ok(Some((username, user_id)))
                }
                Err(SqliteFailure(e, _)) if e.extended_code == SQLITE_CONSTRAINT_UNIQUE => {
                    let result = ResultPacket::Failure("Username already taken.".to_owned());
                    channel.send(result)?;
                    Ok(None)
                }
                _ => {
                    let result = ResultPacket::Failure("Error signing up.".to_owned());
                    channel.send(result)?;
                    Ok(None)
                }
            }
        }

        _ => Ok(None),
    }
}
