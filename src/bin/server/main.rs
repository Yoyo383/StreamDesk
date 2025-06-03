use host::handle_host;
use log::info;
use login_register::login_or_register;
use participant::handle_participant;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rand::Rng;
use std::{
    collections::HashMap,
    net::TcpListener,
    path::PathBuf,
    process::Command,
    sync::{
        mpsc::{self},
        Arc, Mutex,
    },
    thread::{self},
};
use stream_desk::{
    initialize_logger,
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
    UserType, LOG_DIR, LOG_TARGET, SERVER_LOG_FILE,
};
use structs::*;
use watch::handle_watching;

mod host;
mod login_register;
mod participant;
mod structs;
mod watch;

const RECORDINGS_FOLDER: &'static str = "recordings";
const DATABASE_FILE: &'static str = "database.sqlite";

type SharedSession = Arc<Mutex<Session>>;
type SessionHashMap = Arc<Mutex<HashMap<u32, SharedSession>>>;

/// Constructs the full file path for a video recording.
///
/// This function creates a `PathBuf` pointing to a video file within the
/// recordings directory, automatically appending the `.mp4` extension.
///
/// # Arguments
///
/// * `filename` - A `&str` representing the base filename without extension.
///
/// # Returns
///
/// A `PathBuf` containing the complete path to the video file in the format
/// `recordings/{filename}.mp4`.
fn get_video_path(filename: &str) -> PathBuf {
    PathBuf::from(RECORDINGS_FOLDER).join(format!("{filename}.mp4"))
}

/// Generates a unique 6-digit session code for new remote desktop sessions.
///
/// This function creates a random 6-digit number that doesn't conflict with
/// any existing session codes. It continuously generates numbers until finding
/// one that isn't already in use.
///
/// # Arguments
///
/// * `sessions` - A `&HashMap<u32, SharedSession>` containing all active sessions
///                to check for code uniqueness.
///
/// # Returns
///
/// A unique `u32` session code in the range 100,000 to 999,999 that can be
/// used to identify a new remote desktop session.
fn generate_session_code(sessions: &HashMap<u32, SharedSession>) -> u32 {
    let mut rng = rand::rng();
    loop {
        let code: u32 = rng.random_range(100_000..1_000_000); // Generates a 6-digit number

        if !sessions.contains_key(&code) {
            return code;
        }
    }
}

/// Determines the duration of a video file in frames using FFprobe.
///
/// This function executes FFprobe to extract the duration of a video file
/// and converts it to frame count assuming a 30 FPS frame rate.
///
/// # Arguments
///
/// * `filename` - A `&str` representing the video filename without extension.
///
/// # Returns
///
/// An `i32` representing the total number of frames in the video, calculated
/// by multiplying the duration in seconds by 30 and rounding up.
///
/// # Panics
///
/// Panics if FFprobe cannot be executed or if the output cannot be parsed as
/// a valid floating-point duration value.
fn get_duration_frames(filename: &str) -> i32 {
    let input_path = get_video_path(filename);

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            input_path.to_str().unwrap(),
        ])
        .output()
        .expect("Error launching ffprobe.");

    let output_str = String::from_utf8_lossy(&output.stdout);
    let seconds = output_str
        .trim()
        .parse::<f64>()
        .expect("should be valid f64");

    (seconds * 30.0).ceil() as i32
}

/// Retrieves all recordings associated with a specific user from the database.
///
/// This function queries the SQLite database to fetch all recording metadata
/// for a given user, including recording IDs, filenames, and timestamps.
///
/// # Arguments
///
/// * `db_pool` - A `&Pool<SqliteConnectionManager>` for database connection management.
/// * `user_id` - An `i32` representing the unique identifier of the user.
///
/// # Returns
///
/// A `HashMap<i32, Recording>` where keys are recording IDs and values are
/// `Recording` structs containing filename and timestamp information.
///
/// # Panics
///
/// Panics if the database connection fails, the SQL query is malformed, or
/// if there are issues reading the query results.
fn query_recordings(
    db_pool: &Pool<SqliteConnectionManager>,
    user_id: i32,
) -> HashMap<i32, Recording> {
    let db_pool = db_pool.get().unwrap();

    let mut query = db_pool
        .prepare("SELECT recording_id, filename, time FROM recordings WHERE user_id = ?1")
        .unwrap();

    // get a vector of (filename, time)
    let recordings = query
        .query_map([user_id], |row| {
            let id: i32 = row.get(0)?;
            let recording = Recording {
                filename: row.get(1)?,
                time: row.get(2)?,
            };
            Ok((id, recording))
        })
        .unwrap()
        .collect::<Result<HashMap<_, _>, _>>()
        .unwrap();

    recordings
}

/// Handles all communication and operations for a connected client.
///
/// This function manages the complete client lifecycle from initial connection
/// through authentication, menu navigation, and session handling. It coordinates
/// between different client operations like hosting, joining sessions, and
/// watching recordings.
///
/// # Arguments
///
/// * `channel` - A `SecureChannel` for encrypted communication with the client.
/// * `sessions` - A `SessionHashMap` containing all active remote desktop sessions.
/// * `db_pool` - An `Arc<Pool<SqliteConnectionManager>>` for database operations.
///
/// # Returns
///
/// A `std::io::Result<()>` which is:
/// - `Ok(())` when the client disconnects gracefully or requests shutdown
/// - `Err(std::io::Error)` if network communication fails during client handling
///
/// # Behavior
///
/// - Handles initial login/registration flow
/// - Manages menu scene with recording listings
/// - Processes host requests and creates new sessions
/// - Handles join requests with session validation
/// - Manages recording playback requests
/// - Maintains proper cleanup on client disconnection
fn handle_client(
    mut channel: SecureChannel,
    sessions: SessionHashMap,
    db_pool: Arc<Pool<SqliteConnectionManager>>,
) -> std::io::Result<()> {
    let mut username: String;
    let mut user_id: i32;

    loop {
        loop {
            let packet = channel.receive()?;

            if packet == Packet::Shutdown {
                channel.close();
                return Ok(());
            }

            let result = login_or_register(packet, &mut channel, &db_pool)?;
            if let Some((user, id)) = result {
                username = user;
                user_id = id;

                break;
            }
        }

        'menu_scene: loop {
            // send all recordings
            let recordings = query_recordings(&db_pool, user_id);
            for (id, recording) in &recordings {
                let packet = Packet::RecordingName {
                    id: *id,
                    name: recording.time.clone(),
                };
                channel.send(packet)?;
            }
            channel.send(Packet::None)?;

            loop {
                // receive first packet
                let packet = channel.receive()?;

                match packet {
                    Packet::SignOut => {
                        info!(target: LOG_TARGET, "User {} signed out.", username);
                        break 'menu_scene;
                    }

                    Packet::Host => {
                        let mut sessions_guard = sessions.lock().unwrap();
                        let code = generate_session_code(&sessions_guard);

                        let host_connection = Connection {
                            channel: channel.clone(),
                            user_type: UserType::Host,
                        };

                        let session =
                            Arc::new(Mutex::new(Session::new(username.clone(), host_connection)));
                        sessions_guard.insert(code, session.clone());

                        // release the lock
                        drop(sessions_guard);

                        // send back the session code
                        channel.send(ResultPacket::Success(format!("{}", code)))?;

                        info!(
                            target: LOG_TARGET,
                            "User {} started hosting a session with code {}.",
                            username, code
                        );

                        handle_host(
                            &mut channel,
                            session,
                            sessions.clone(),
                            code,
                            username.clone(),
                            user_id,
                            &db_pool,
                        )?;

                        break;
                    }

                    Packet::Join { code, username } => {
                        let sessions = sessions.lock().unwrap();

                        // check if the code exists
                        if let Some(session) = sessions.get(&code) {
                            // cloning so i can drop sessions and unlock the mutex
                            let session = session.clone();
                            drop(sessions);

                            let mut session_guard = session.lock().unwrap();

                            if session_guard.connections.contains_key(&username) {
                                let failure = ResultPacket::Failure(
                                    "You are already connected to this session from elsewhere."
                                        .to_string(),
                                );
                                channel.send(failure)?;
                                continue;
                            }

                            let success = ResultPacket::Success("Joining".to_owned());
                            channel.send(success)?;

                            // send host the join request
                            let packet = Packet::Join {
                                code,
                                username: username.clone(),
                            };
                            session_guard.host().send(packet)?;

                            let (sender, receiver) = mpsc::channel();

                            let connection = Connection {
                                channel: channel.clone(),
                                user_type: UserType::Participant,
                            };
                            session_guard
                                .pending_join
                                .insert(username.clone(), (connection, sender));

                            drop(session_guard);

                            info!(target: LOG_TARGET, "User {} requested to join session {}.", username, code);

                            // if host allowed user then continue
                            if receiver.recv().unwrap() {
                                info!(target: LOG_TARGET, "User {} was allowed to join session {}.", username, code);
                                handle_participant(
                                    &mut channel,
                                    session.clone(),
                                    username.clone(),
                                )?;
                                break;
                            }
                        } else {
                            // no such session
                            let failure = ResultPacket::Failure(format!(
                                "No session found with code {}",
                                code
                            ));
                            channel.send(failure)?;
                        }
                    }

                    Packet::WatchRecording { id } => {
                        let recording = recordings.get(&id);
                        match recording {
                            Some(recording) => {
                                let does_video_exists =
                                    get_video_path(&recording.filename).exists();

                                if does_video_exists {
                                    let num_of_frames = get_duration_frames(&recording.filename);
                                    let success = ResultPacket::Success(num_of_frames.to_string());
                                    channel.send(success)?;

                                    info!(
                                        target: LOG_TARGET,
                                        "User {} is watching recording {}.mp4.",
                                        username, recording.filename
                                    );

                                    handle_watching(&mut channel, &recording.filename)?;
                                    break;
                                } else {
                                    let failure = ResultPacket::Failure(
                                        "Video file does not exist on the server.".to_owned(),
                                    );
                                    channel.send(failure)?;
                                }
                            }
                            None => {
                                let failure =
                                    ResultPacket::Failure("No recording found.".to_owned());
                                channel.send(failure)?;
                            }
                        }
                    }

                    _ => (),
                }
            }
        }
    }
}

/// Entry point for the remote desktop server application.
///
/// This function initializes the server infrastructure including database setup,
/// directory creation, and TCP listener configuration. It accepts incoming client
/// connections and spawns individual threads to handle each client session.
///
/// # Behavior
///
/// - Creates the recordings directory if it doesn't exist
/// - Initializes SQLite database connection pool
/// - Creates necessary database tables (users and recordings)
/// - Binds TCP listener to port 7643 on all interfaces
/// - Spawns secure channels and client handler threads for each connection
/// - Maintains a shared session map for active remote desktop sessions
/// - Handles connection errors gracefully without terminating the server
fn main() {
    let _ = std::fs::create_dir(LOG_DIR);
    let _ = std::fs::create_dir(RECORDINGS_FOLDER);

    initialize_logger(SERVER_LOG_FILE);

    let db_manager = SqliteConnectionManager::file(DATABASE_FILE);
    let db_pool = Arc::new(r2d2::Pool::new(db_manager).unwrap());

    db_pool
        .get()
        .unwrap()
        .execute(
            "CREATE TABLE IF NOT EXISTS users(
                user_id INTEGER PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                password TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

    db_pool
        .get()
        .unwrap()
        .execute(
            "CREATE TABLE IF NOT EXISTS recordings(
                recording_id INTEGER PRIMARY KEY,
                filename TEXT NOT NULL,
                time TEXT NOT NULL,
                user_id INTEGER,
                FOREIGN KEY (user_id) REFERENCES users(user_id)
            )",
            [],
        )
        .unwrap();

    let listener = TcpListener::bind("0.0.0.0:7643").expect("Could not bind listener");

    let sessions: SessionHashMap = Arc::new(Mutex::new(HashMap::new()));

    for connection in listener.incoming() {
        match connection {
            Ok(socket) => {
                let sessions_clone = sessions.clone();
                let db_pool_clone = db_pool.clone();

                let mut channel = SecureChannel::new_server(Some(socket)).unwrap();
                thread::spawn(move || {
                    if let Err(_) = handle_client(channel.clone(), sessions_clone, db_pool_clone) {
                        channel.close();
                    }
                });
            }
            Err(e) => eprintln!("Couldn't accept client: {e:?}"),
        }
    }
}
