use chrono::{DateTime, Local};
use h264_reader::{
    annexb::AnnexBReader,
    nal::{Nal, RefNal},
    push::NalInterest,
};
use rand::Rng;
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    UserType,
};
use rusqlite::{ffi::SQLITE_CONSTRAINT_UNIQUE, params, Error::SqliteFailure};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, ChildStdout, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

const RECORDINGS_FOLDER: &'static str = "recordings";
const DATABASE_FILE: &'static str = "database.sqlite";

type SharedSession = Arc<Mutex<Session>>;
type SessionHashMap = Arc<Mutex<HashMap<u32, SharedSession>>>;

#[derive(PartialEq, Eq, Debug)]
enum ConnectionType {
    Host,
    Controller,
    Participant,
    Unready,
}

struct Recording {
    filename: String,
    time: String,
}

#[derive(Debug)]
struct Connection {
    socket: TcpStream,
    connection_type: ConnectionType,
    user_type: UserType,
    join_request_sender: Option<Sender<bool>>,
}

struct Session {
    connections: HashMap<String, Connection>,
    pending_join: HashMap<String, Connection>,
}

impl Session {
    fn new(host_username: String, host_conn: Connection) -> Self {
        let mut connections = HashMap::new();
        connections.insert(host_username, host_conn);

        Self {
            connections,
            pending_join: HashMap::new(),
        }
    }

    fn broadcast_all(&mut self, packet: Packet) {
        for (_, connection) in &mut self.connections {
            packet.send(&mut connection.socket).unwrap();
        }
    }

    fn broadcast_participants(&mut self, packet: Packet) {
        for (_, connection) in &mut self.connections {
            if connection.connection_type == ConnectionType::Participant
                || connection.connection_type == ConnectionType::Controller
            {
                packet.send(&mut connection.socket).unwrap();
            }
        }
    }

    fn host(&self) -> TcpStream {
        self.connections
            .iter()
            .find(|(_, conn)| conn.connection_type == ConnectionType::Host)
            .unwrap()
            .1
            .socket
            .try_clone()
            .unwrap()
    }
}

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

fn ffmpeg_send_recording(filename: &str) -> Child {
    let input_path = PathBuf::from(RECORDINGS_FOLDER).join(format!("{filename}.mp4"));

    let ffmpeg = Command::new("ffmpeg")
        .args(&[
            "-i",
            input_path.to_str().unwrap(),
            "-vcodec",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-x264opts",
            "no-scenecut",
            "-sc_threshold",
            "0",
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

fn login_or_register(
    packet: Packet,
    socket: &mut TcpStream,
    conn: &rusqlite::Connection,
) -> Option<(String, i32, HashMap<i32, Recording>)> {
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
                    let result = ResultPacket::Success("Signing in".to_owned());
                    result.send(socket).unwrap();

                    let recordings = query_recordings(&conn, user_id);

                    Some((username, user_id, recordings))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    let result =
                        ResultPacket::Failure("Username or password are incorrect.".to_owned());
                    result.send(socket).unwrap();
                    None
                }
                _ => {
                    let result = ResultPacket::Failure("Error signing in.".to_owned());
                    result.send(socket).unwrap();
                    None
                }
            }
        }

        Packet::Register { username, password } => {
            let inserted = conn.execute(
                "INSERT INTO users (username, password) VALUES (?1, ?2)",
                params![username, password],
            );

            match inserted {
                Ok(_) => {
                    let result = ResultPacket::Success("Signing in".to_owned());
                    result.send(socket).unwrap();

                    let user_id = conn.last_insert_rowid() as i32;
                    let recordings = query_recordings(&conn, user_id);

                    Some((username, user_id, recordings))
                }
                Err(SqliteFailure(e, _)) if e.extended_code == SQLITE_CONSTRAINT_UNIQUE => {
                    let result = ResultPacket::Failure("Username already taken.".to_owned());
                    result.send(socket).unwrap();
                    None
                }
                _ => {
                    let result = ResultPacket::Failure("Error signing up.".to_owned());
                    result.send(socket).unwrap();
                    None
                }
            }
        }

        _ => None,
    }
}

fn handle_host(
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

fn handle_participant(socket: &mut TcpStream, session: SharedSession, username: String) {
    loop {
        let packet = Packet::receive(socket).unwrap();

        match packet {
            Packet::Control { .. } => {
                let session = session.lock().unwrap();

                if session.connections.get(&username).unwrap().connection_type
                    == ConnectionType::Controller
                {
                    packet.send(&mut session.host()).unwrap();
                }
            }

            Packet::RequestControl { .. } => {
                let session = session.lock().unwrap();

                // can send to host only if participant, not unready
                if session.connections.get(&username).unwrap().connection_type
                    == ConnectionType::Participant
                {
                    packet.send(&mut session.host()).unwrap();
                } else {
                    // send DenyRequest because not participant
                    let deny_packet = Packet::DenyControl {
                        username: username.clone(),
                    };
                    deny_packet.send(socket).unwrap();
                }
            }

            Packet::SessionExit => {
                let mut session = session.lock().unwrap();
                session.connections.remove(&username);

                let user_update_packet = Packet::UserUpdate {
                    user_type: UserType::Leaving,
                    joined_before: false,
                    username: username.clone(),
                };
                session.broadcast_all(user_update_packet);

                let session_exit = Packet::SessionExit;
                session_exit.send(socket).unwrap();

                break;
            }

            Packet::Chat { message } => {
                let message = username.to_string() + ": " + &message;
                let packet = Packet::Chat { message };

                let mut session = session.lock().unwrap();
                session.broadcast_all(packet);
            }

            Packet::SessionEnd => break,

            _ => (),
        }
    }
}

fn thread_send_screen(
    mut socket: TcpStream,
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

            let screen_packet = Packet::Screen { bytes: nal_bytes };
            screen_packet.send(&mut socket).unwrap();

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
    })
}

fn handle_watching(socket: &mut TcpStream, filename: &str) {
    let mut ffmpeg = ffmpeg_send_recording(filename);
    let stdout = ffmpeg.stdout.take().unwrap();

    let stop_flag = Arc::new(AtomicBool::new(false));

    let thread_send_screen =
        thread_send_screen(socket.try_clone().unwrap(), stdout, stop_flag.clone());

    loop {
        let packet = Packet::receive(socket).unwrap();

        match packet {
            Packet::SessionExit => {
                stop_flag.store(true, Ordering::Relaxed);

                let _ = ffmpeg.kill();
                let _ = thread_send_screen.join();

                let packet = Packet::SessionExit;
                packet.send(socket).unwrap();

                break;
            }

            _ => (),
        }
    }
}

fn handle_client(mut socket: TcpStream, sessions: SessionHashMap) {
    let conn = rusqlite::Connection::open(DATABASE_FILE).unwrap();
    let mut username: String;
    let mut user_id: i32;
    let mut recordings: HashMap<i32, Recording>;

    loop {
        loop {
            let packet = Packet::receive(&mut socket).unwrap();

            if packet == Packet::Shutdown {
                socket
                    .shutdown(std::net::Shutdown::Both)
                    .expect("Could not close socket.");
                return;
            }

            let result = login_or_register(packet, &mut socket, &conn);
            if let Some((user, id, records)) = result {
                username = user;
                user_id = id;
                recordings = records;
                break;
            }
        }

        loop {
            // send all recordings
            for (id, recording) in &recordings {
                let time: DateTime<Local> = recording.time.parse().unwrap();
                let recording_display_name = time.format("%B %-d, %Y | %T").to_string();

                let packet = Packet::RecordingName {
                    id: *id,
                    name: recording_display_name,
                };
                packet.send(&mut socket).unwrap();
            }
            let end_packet = Packet::None;
            end_packet.send(&mut socket).unwrap();

            // receive first packet
            let packet = Packet::receive(&mut socket).unwrap();

            match packet {
                Packet::SignOut => {
                    username.clear();
                    break;
                }

                Packet::Host => {
                    let mut sessions_guard = sessions.lock().unwrap();
                    let code = generate_session_code(&sessions_guard);

                    let host_connection = Connection {
                        socket: socket.try_clone().unwrap(),
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
                    let success = ResultPacket::Success(format!("{}", code));
                    success.send(&mut socket).unwrap();

                    handle_host(
                        &mut socket,
                        session,
                        sessions.clone(),
                        code,
                        username.clone(),
                        user_id,
                        &conn,
                    );
                }

                Packet::Join { code, ref username } => {
                    let sessions = sessions.lock().unwrap();

                    // check if the code exists
                    if let Some(session) = sessions.get(&code) {
                        // cloning so i can drop sessions and unlock the mutex
                        let session = session.clone();
                        drop(sessions);

                        let mut session_guard = session.lock().unwrap();

                        let success = ResultPacket::Success("Joining".to_owned());
                        success.send(&mut socket).unwrap();

                        // send host the join request
                        packet.send(&mut session_guard.host()).unwrap();

                        let (sender, receiver) = mpsc::channel();

                        let connection = Connection {
                            socket: socket.try_clone().unwrap(),
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
                            handle_participant(&mut socket, session.clone(), username.clone());
                        }
                    } else {
                        // no such session
                        let failure =
                            ResultPacket::Failure(format!("No session found with code {}", code));
                        failure.send(&mut socket).unwrap();
                    }
                }

                Packet::WatchRecording { id } => {
                    let recording = recordings.get(&id);
                    match recording {
                        Some(recording) => {
                            let success = ResultPacket::Success("Watching".to_owned());
                            success.send(&mut socket).unwrap();

                            handle_watching(&mut socket, &recording.filename);
                        }
                        None => {
                            let failure = ResultPacket::Failure("No recording found.".to_owned());
                            failure.send(&mut socket).unwrap();
                        }
                    }
                }

                _ => (),
            }
        }
    }
}

fn main() {
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

    for connection in listener.incoming() {
        match connection {
            Ok(socket) => {
                let sessions_clone = sessions.clone();
                thread::spawn(move || handle_client(socket, sessions_clone));
            }
            Err(e) => eprintln!("Couldn't accept client: {e:?}"),
        }
    }
}
