use rand::Rng;
use remote_desktop::protocol::{Message, MessageType};
use rusqlite::params;
use std::{
    collections::HashMap,
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
};

const DATABASE_FILE: &'static str = "database.sqlite";

#[derive(PartialEq, Eq, Debug)]
enum ConnectionType {
    None,
    Host,
    Participant,
    Unready,
}

#[derive(Debug)]
struct Connection {
    socket: TcpStream,
    connection_type: ConnectionType,
    type_sender: Sender<ConnectionType>,
    username: String,
}

struct Session {
    connections: Vec<Connection>,
}

impl Session {
    fn new(host: Connection) -> Self {
        Self {
            connections: vec![host],
        }
    }

    fn broadcast_all(&mut self, message: Message) {
        for connection in &mut self.connections {
            message.send(&mut connection.socket).unwrap();
        }
    }

    fn broadcast_non_host(&mut self, message: Message) {
        for connection in &mut self.connections {
            if connection.connection_type != ConnectionType::Host {
                message.send(&mut connection.socket).unwrap();
            }
        }
    }

    fn broadcast_participants(&mut self, message: Message) {
        for connection in &mut self.connections {
            if connection.connection_type == ConnectionType::Participant {
                message.send(&mut connection.socket).unwrap();
            }
        }
    }

    fn host(&self) -> TcpStream {
        self.connections
            .iter()
            .find(|conn| conn.connection_type == ConnectionType::Host)
            .unwrap()
            .socket
            .try_clone()
            .unwrap()
    }

    fn usernames(&self) -> String {
        self.connections
            .iter()
            .map(|conn| &conn.username)
            .cloned()
            .collect::<Vec<String>>()
            .join("\n")
    }
}

fn generate_session_code(sessions: &HashMap<i32, Session>) -> i32 {
    let mut rng = rand::rng();
    loop {
        let code: i32 = rng.random_range(0..1_000_000); // Generates a 6-digit number
        println!("{}", code);
        if !sessions.contains_key(&code) {
            return code;
        }
    }
}

fn handle_client(mut socket: TcpStream, sessions: Arc<Mutex<HashMap<i32, Session>>>) {
    let mut connection_type = ConnectionType::None;
    let mut session_code: i32 = -1;
    let mut type_receiver: Option<Receiver<ConnectionType>> = None;

    let conn = rusqlite::Connection::open(DATABASE_FILE).unwrap();

    loop {
        if let Some(ref receiver) = type_receiver {
            if let Ok(new_type) = receiver.try_recv() {
                connection_type = new_type;
            }
        }
        let message = Message::receive(&mut socket).unwrap();

        match message.message_type {
            MessageType::Login => {
                // get username and password
                let username_password =
                    String::from_utf8(message.vector_data).expect("bytes should be utf8");
                let newline_pos = username_password.find('\n').unwrap();

                let username = &username_password[..newline_pos];
                let password = &username_password[newline_pos + 1..];

                // query database
                let user_id_result: Result<i32, rusqlite::Error> = conn.query_row(
                    "SELECT id FROM users WHERE username = ?1 AND password = ?2",
                    params![username, password],
                    |row| row.get(0),
                );

                match user_id_result {
                    Ok(_) => {
                        let message = Message::new_login(username, password);
                        message.send(&mut socket).unwrap();
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {
                        let message = Message::default();
                        message.send(&mut socket).unwrap();
                    }
                    _ => (),
                }
            }

            MessageType::Register => {
                // get username and password
                let username_password =
                    String::from_utf8(message.vector_data).expect("bytes should be utf8");
                let newline_pos = username_password.find('\n').unwrap();

                let username = &username_password[..newline_pos];
                let password = &username_password[newline_pos + 1..];

                let inserted = conn.execute(
                    "INSERT INTO users (username, password) VALUES (?1, ?2)",
                    params![username, password],
                );

                match inserted {
                    Ok(_) => {
                        let message = Message::new_register(username, password);
                        message.send(&mut socket).unwrap();
                    }

                    Err(rusqlite::Error::SqliteFailure(e, _))
                        if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
                    {
                        let message = Message::default();
                        message.send(&mut socket).unwrap();
                    }

                    _ => (),
                }
            }

            MessageType::Hosting => {
                let username =
                    String::from_utf8(message.vector_data).expect("bytes should be utf8");

                // start new session
                let mut sessions = sessions.lock().unwrap();
                session_code = generate_session_code(&sessions);

                let (sender, receiver) = mpsc::channel();
                type_receiver = Some(receiver);

                connection_type = ConnectionType::Host;
                let host_connection = Connection {
                    socket: socket.try_clone().unwrap(),
                    connection_type: ConnectionType::Host,
                    type_sender: sender,
                    username: username.clone(),
                };

                let session = Session::new(host_connection);
                sessions.insert(session_code, session);

                // send back the session code
                let message = Message::new_joining(session_code, &username);
                message.send(&mut socket).unwrap();
            }

            MessageType::Joining => {
                let mut sessions = sessions.lock().unwrap();
                session_code = message.general_data;

                // check if the code exists
                // if code exists, send it back, and if it doesn't exist, send back code -1
                match sessions.get_mut(&session_code) {
                    Some(session) => {
                        let username = String::from_utf8(message.vector_data.clone())
                            .expect("bytes should be utf8");

                        let (sender, receiver) = mpsc::channel();
                        type_receiver = Some(receiver);

                        // send new username to all participants
                        session.broadcast_all(message);

                        connection_type = ConnectionType::Unready;
                        let connection = Connection {
                            socket: socket.try_clone().unwrap(),
                            connection_type: ConnectionType::Unready,
                            type_sender: sender,
                            username,
                        };
                        session.connections.push(connection);

                        // send all usernames
                        let usernames = session.usernames(); //session.usernames.join("\n");
                        let message = Message::new_joining(session_code, &usernames);
                        message.send(&mut socket).unwrap();
                    }
                    None => {
                        let message = Message::new_joining(-1, "");
                        message.send(&mut socket).unwrap();
                    }
                }
            }

            MessageType::MergeUnready => {
                if connection_type != ConnectionType::Host {
                    return;
                }

                let mut sessions = sessions.lock().unwrap();
                let session = sessions
                    .get_mut(&session_code)
                    .expect("should contain a session");

                // change all unready to participants
                for connection in &mut session.connections {
                    if connection.connection_type == ConnectionType::Unready {
                        connection.connection_type = ConnectionType::Participant;
                        let _ = connection.type_sender.send(ConnectionType::Participant);
                    }
                }
            }

            MessageType::SessionExit => {
                let mut sessions = sessions.lock().unwrap();
                let session = sessions
                    .get_mut(&session_code)
                    .expect("should contain a session");

                match connection_type {
                    ConnectionType::Host => {
                        let message = Message::new_session_end();
                        session.broadcast_non_host(message);

                        sessions.remove(&session_code);
                        connection_type = ConnectionType::None;
                    }

                    _ => {
                        // removing participant
                        session.connections.retain(|conn| {
                            conn.socket.peer_addr().unwrap() != socket.peer_addr().unwrap()
                        });

                        session.broadcast_all(message);

                        let message = Message::new_session_end();
                        message.send(&mut socket).unwrap();

                        connection_type = ConnectionType::None;
                    }
                }
            }

            MessageType::Shutdown => {
                socket
                    .shutdown(std::net::Shutdown::Both)
                    .expect("Could not close socket.");

                break;
            }

            MessageType::Screen => {
                let mut sessions = sessions.lock().unwrap();
                let session = sessions
                    .get_mut(&session_code)
                    .expect("should contain a session");

                session.broadcast_participants(message);
            }

            _ => match connection_type {
                ConnectionType::Host => {
                    // forward the message to all participants
                    let mut sessions = sessions.lock().unwrap();
                    let session = sessions
                        .get_mut(&session_code)
                        .expect("should contain a session");

                    session.broadcast_non_host(message);
                }

                _ => {
                    // forward the message to the host
                    let mut sessions = sessions.lock().unwrap();
                    let session = sessions
                        .get_mut(&session_code)
                        .expect("should contain a session");

                    message.send(&mut session.host()).unwrap();
                }
            },
        }
    }
}

fn main() {
    // TODO: create database
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

    let sessions: Arc<Mutex<HashMap<i32, Session>>> = Arc::new(Mutex::new(HashMap::new()));

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
