use std::{collections::HashMap, net::TcpStream, sync::mpsc::Sender};

use remote_desktop::{protocol::Packet, UserType};

#[derive(PartialEq, Eq, Debug)]
pub enum ConnectionType {
    Host,
    Controller,
    Participant,
    Unready,
}

pub struct Recording {
    pub filename: String,
    pub time: String,
}

#[derive(Debug)]
pub struct Connection {
    pub socket: TcpStream,
    pub connection_type: ConnectionType,
    pub user_type: UserType,
    pub join_request_sender: Option<Sender<bool>>,
}

pub struct Session {
    pub connections: HashMap<String, Connection>,
    pub pending_join: HashMap<String, Connection>,
}

impl Session {
    pub fn new(host_username: String, host_conn: Connection) -> Self {
        let mut connections = HashMap::new();
        connections.insert(host_username, host_conn);

        Self {
            connections,
            pending_join: HashMap::new(),
        }
    }

    pub fn broadcast_all(&mut self, packet: Packet) {
        for (_, connection) in &mut self.connections {
            packet.send(&mut connection.socket).unwrap();
        }
    }

    pub fn broadcast_participants(&mut self, packet: Packet) {
        for (_, connection) in &mut self.connections {
            if connection.connection_type == ConnectionType::Participant
                || connection.connection_type == ConnectionType::Controller
            {
                packet.send(&mut connection.socket).unwrap();
            }
        }
    }

    pub fn host(&self) -> TcpStream {
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
