use rand::Rng;
use remote_desktop::{
    protocol::{Packet, ResultPacket},
    UserType,
};
use rusqlite::{ffi::SQLITE_CONSTRAINT_UNIQUE, params, Error::SqliteFailure};
use std::{
    collections::HashMap,
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

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

#[derive(Debug)]
struct Connection {
    socket: TcpStream,
    connection_type: ConnectionType,
    user_type: UserType,
}

struct Session {
    connections: HashMap<String, Connection>,
}

impl Session {
    fn new(host_username: String, host_conn: Connection) -> Self {
        let mut connections = HashMap::new();
        connections.insert(host_username, host_conn);

        Self { connections }
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

fn login_or_register(
    packet: Packet,
    socket: &mut TcpStream,
    conn: &rusqlite::Connection,
) -> Option<String> {
    match packet {
        Packet::Shutdown => None,

        Packet::Login { username, password } => {
            let user_id_result: Result<i32, rusqlite::Error> = conn.query_row(
                "SELECT id FROM users WHERE username = ?1 AND password = ?2",
                params![username, password],
                |row| row.get(0),
            );

            match user_id_result {
                Ok(_) => {
                    let result = ResultPacket::Success("Signing in".to_owned());
                    result.send(socket).unwrap();
                    Some(username)
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
                    Some(username)
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
) {
    loop {
        let packet = Packet::receive(socket).unwrap();

        match packet {
            Packet::Screen { .. } => {
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

            // TODO: implement this
            Packet::RequestControl => {}

            // TODO: implement this
            Packet::DenyControl => {}

            _ => (),
        }
    }
}

fn handle_participant(socket: &mut TcpStream, session: SharedSession, username: String) {
    loop {
        let packet = Packet::receive(socket).unwrap();

        match packet {
            Packet::Control { .. } => {
                // TODO: only forward if user is a controller
                let session = session.lock().unwrap();
                packet.send(&mut session.host()).unwrap();
            }

            // TODO: implement this
            Packet::RequestControl => {}

            Packet::SessionExit => {
                let mut session = session.lock().unwrap();
                session.connections.remove(&username);

                let user_update_packet = Packet::UserUpdate {
                    user_type: UserType::Leaving,
                    username: username.clone(),
                };
                session.broadcast_all(user_update_packet);

                let session_exit = Packet::SessionExit;
                session_exit.send(socket).unwrap();

                break;
            }

            Packet::SessionEnd => break,

            _ => (),
        }
    }
}

fn handle_client(mut socket: TcpStream, sessions: SessionHashMap) {
    let conn = rusqlite::Connection::open(DATABASE_FILE).unwrap();
    let mut username: String;

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
            if let Some(user) = result {
                username = user;
                break;
            }
        }

        loop {
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
                    };

                    let session =
                        Arc::new(Mutex::new(Session::new(username.clone(), host_connection)));
                    sessions_guard.insert(code, session.clone());

                    // release the lock
                    drop(sessions_guard);

                    // send back the session code
                    let success = ResultPacket::Success(format!("{}", code));
                    success.send(&mut socket).unwrap();

                    handle_host(&mut socket, session, sessions.clone(), code);
                }

                Packet::Join { code } => {
                    let sessions = sessions.lock().unwrap();

                    // check if the code exists
                    if let Some(session) = sessions.get(&code) {
                        let mut session_guard = session.lock().unwrap();

                        let success = ResultPacket::Success("Joining".to_owned());
                        success.send(&mut socket).unwrap();

                        // send new username to all participants
                        let packet = Packet::UserUpdate {
                            user_type: UserType::Participant,
                            username: username.clone(),
                        };
                        session_guard.broadcast_all(packet);

                        // create connection
                        let connection = Connection {
                            socket: socket.try_clone().unwrap(),
                            connection_type: ConnectionType::Unready,
                            user_type: UserType::Participant,
                        };
                        session_guard
                            .connections
                            .insert(username.clone(), connection);

                        // send all usernames
                        for (username, connection) in &session_guard.connections {
                            let username_packet = Packet::UserUpdate {
                                user_type: connection.user_type,
                                username: username.clone(),
                            };
                            username_packet.send(&mut socket).unwrap();
                        }

                        drop(session_guard);

                        handle_participant(&mut socket, session.clone(), username.clone());
                    } else {
                        // no such session
                        let failure =
                            ResultPacket::Failure(format!("No session found with code {}", code));
                        failure.send(&mut socket).unwrap();
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
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password TEXT NOT NULL
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
