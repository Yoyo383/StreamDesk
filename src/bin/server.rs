use rand::Rng;
use remote_desktop::protocol::{Message, MessageType};
use std::{
    collections::HashMap,
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

#[derive(PartialEq, Eq, Debug)]
enum ClientType {
    None,
    Host,
    Participant,
}

struct Session {
    host: TcpStream,
    participants: Vec<TcpStream>,
    unready: Vec<TcpStream>,
}

impl Session {
    fn new(host: TcpStream) -> Self {
        Self {
            host,
            participants: Vec::new(),
            unready: Vec::new(),
        }
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
    let mut client_type = ClientType::None;
    let mut session_code: i32 = -1;

    loop {
        let message = Message::receive(&mut socket).unwrap();

        match message.message_type {
            MessageType::Hosting => {
                // start new session
                let mut sessions = sessions.lock().unwrap();
                session_code = generate_session_code(&sessions);
                let session = Session::new(socket.try_clone().unwrap());
                sessions.insert(session_code, session);

                // send back the session code
                let message = Message::new_joining(session_code);
                message.send(&mut socket).unwrap();
                client_type = ClientType::Host;
            }

            MessageType::Joining => {
                let mut sessions = sessions.lock().unwrap();
                session_code = message.general_data;

                // check if the code exists
                // if code exists, send it back, and if it doesn't exist, send back code -1
                match sessions.get_mut(&session_code) {
                    Some(session) => {
                        session.unready.push(socket.try_clone().unwrap());
                        client_type = ClientType::Participant;

                        message.send(&mut socket).unwrap();
                    }
                    None => {
                        let message = Message::new_joining(-1);
                        message.send(&mut socket).unwrap();
                    }
                }
            }

            MessageType::MergeUnready => {
                if client_type != ClientType::Host {
                    return;
                }

                let mut sessions = sessions.lock().unwrap();
                let session = sessions
                    .get_mut(&session_code)
                    .expect("should contain a session");

                session.participants.append(&mut session.unready);
            }

            _ => match client_type {
                ClientType::Host => {
                    // forward the message to all participants
                    let mut sessions = sessions.lock().unwrap();
                    let session = sessions
                        .get_mut(&session_code)
                        .expect("should contain a session");

                    for participant in &mut session.participants {
                        message.send(participant).unwrap();
                    }
                }

                ClientType::Participant => {
                    // forward the message to the host
                    let mut sessions = sessions.lock().unwrap();
                    let session = sessions
                        .get_mut(&session_code)
                        .expect("should contain a session");

                    message.send(&mut session.host).unwrap();
                }

                _ => (),
            },
        }
    }
}

fn main() {
    let listener = TcpListener::bind("0.0.0.0:7643").expect("Could not bind listener");

    let sessions: Arc<Mutex<HashMap<i32, Session>>> = Arc::new(Mutex::new(HashMap::new()));

    for connection in listener.incoming() {
        match connection {
            Ok(socket) => {
                let sessions_clone = sessions.clone();
                thread::spawn(move || handle_client(socket, sessions_clone));
            }
            Err(e) => println!("Couldn't accept client: {e:?}"),
        }
    }

    // match listener.accept() {
    //     Ok((socket, _addr)) => handle_connection(socket),
    //     Err(e) => println!("Couldn't accept client: {e:?}"),
    // }
}
