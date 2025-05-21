use host::handle_host;
use login_register::login_or_register;
use participant::handle_participant;
use rand::Rng;
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    secure_channel::SecureChannel,
    UserType,
};
use std::{
    collections::{HashMap, HashSet},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    sync::{
        mpsc::{self},
        Arc, Mutex,
    },
    thread::{self},
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

fn generate_session_code(sessions: &HashMap<u32, SharedSession>) -> u32 {
    let mut rng = rand::rng();
    loop {
        let code: u32 = rng.random_range(100_000..1_000_000); // Generates a 6-digit number
        println!("{}", code);
        if !sessions.contains_key(&code) {
            return code;
        }
    }
}

fn get_duration_frames(filename: &str) -> i32 {
    let input_path = PathBuf::from(RECORDINGS_FOLDER).join(format!("{filename}.mp4"));

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

fn query_recordings(conn: &rusqlite::Connection, user_id: i32) -> HashMap<i32, Recording> {
    let mut query = conn
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

fn handle_client(
    mut channel: SecureChannel,
    sessions: SessionHashMap,
    logged_in_users: Arc<Mutex<HashSet<String>>>,
) {
    let conn = rusqlite::Connection::open(DATABASE_FILE).unwrap();
    let mut username: String;
    let mut user_id: i32;

    loop {
        loop {
            let packet = channel.receive().unwrap();

            if packet == Packet::Shutdown {
                channel.close();
                return;
            }

            let result = login_or_register(packet, &mut channel, &conn, logged_in_users.clone());
            if let Some((user, id)) = result {
                username = user;
                user_id = id;

                break;
            }
        }

        'menu_scene: loop {
            // send all recordings
            let recordings = query_recordings(&conn, user_id);
            for (id, recording) in &recordings {
                let packet = Packet::RecordingName {
                    id: *id,
                    name: recording.time.clone(),
                };
                channel.send(packet).unwrap();
            }
            channel.send(Packet::None).unwrap();

            loop {
                // receive first packet
                let packet = channel.receive().unwrap();

                match packet {
                    Packet::SignOut => {
                        logged_in_users.lock().unwrap().remove(&username);

                        break 'menu_scene;
                    }

                    Packet::Host => {
                        let mut sessions_guard = sessions.lock().unwrap();
                        let code = generate_session_code(&sessions_guard);

                        let host_connection = Connection {
                            channel: channel.clone(),
                            connection_type: ConnectionType::Host,
                            user_type: UserType::Host,
                            join_request_sender: None,
                        };

                        let session =
                            Arc::new(Mutex::new(Session::new(username.clone(), host_connection)));
                        sessions_guard.insert(code, session.clone());

                        // release the lock
                        drop(sessions_guard);

                        // send back the session code
                        channel
                            .send(ResultPacket::Success(format!("{}", code)))
                            .unwrap();

                        handle_host(
                            &mut channel,
                            session,
                            sessions.clone(),
                            code,
                            username.clone(),
                            user_id,
                            &conn,
                        );

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

                            let success = ResultPacket::Success("Joining".to_owned());
                            channel.send(success).unwrap();

                            // send host the join request
                            let packet = Packet::Join {
                                code,
                                username: username.clone(),
                            };
                            session_guard.host().send(packet).unwrap();

                            let (sender, receiver) = mpsc::channel();

                            let connection = Connection {
                                channel: channel.clone(),
                                connection_type: ConnectionType::Unready,
                                user_type: UserType::Participant,
                                join_request_sender: Some(sender),
                            };
                            session_guard
                                .pending_join
                                .insert(username.clone(), connection);

                            drop(session_guard);

                            // if host allowed user then continue
                            if receiver.recv().unwrap() {
                                handle_participant(&mut channel, session.clone(), username.clone());
                                break;
                            }
                        } else {
                            // no such session
                            let failure = ResultPacket::Failure(format!(
                                "No session found with code {}",
                                code
                            ));
                            channel.send(failure).unwrap();
                        }
                    }

                    Packet::WatchRecording { id } => {
                        let recording = recordings.get(&id);
                        match recording {
                            Some(recording) => {
                                let num_of_frames = get_duration_frames(&recording.filename);
                                let success = ResultPacket::Success(num_of_frames.to_string());
                                channel.send(success).unwrap();

                                handle_watching(&mut channel, &recording.filename);
                                break;
                            }
                            None => {
                                let failure =
                                    ResultPacket::Failure("No recording found.".to_owned());
                                channel.send(failure).unwrap();
                            }
                        }
                    }

                    _ => (),
                }
            }
        }
    }
}

fn main() {
    let _ = std::fs::create_dir(RECORDINGS_FOLDER);

    let conn = rusqlite::Connection::open(DATABASE_FILE).unwrap();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS users(
            user_id INTEGER PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            password TEXT NOT NULL
        )
        ",
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE IF NOT EXISTS recordings(
            recording_id INTEGER PRIMARY KEY,
            filename TEXT NOT NULL,
            time TEXT NOT NULL,
            user_id INTEGER,
            FOREIGN KEY (user_id) REFERENCES users(user_id)
        )
        ",
        [],
    )
    .unwrap();

    let listener = TcpListener::bind("0.0.0.0:7643").expect("Could not bind listener");

    let sessions: SessionHashMap = Arc::new(Mutex::new(HashMap::new()));
    let logged_in_users = Arc::new(Mutex::new(HashSet::<String>::new()));

    for connection in listener.incoming() {
        match connection {
            Ok(socket) => {
                let sessions_clone = sessions.clone();
                let logged_in_users_clone = logged_in_users.clone();

                let channel = SecureChannel::new_server(socket).unwrap();
                thread::spawn(move || {
                    handle_client(channel, sessions_clone, logged_in_users_clone)
                });
            }
            Err(e) => eprintln!("Couldn't accept client: {e:?}"),
        }
    }
}
