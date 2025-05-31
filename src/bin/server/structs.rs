use std::{collections::HashMap, sync::mpsc::Sender};

use remote_desktop::{protocol::Packet, secure_channel::SecureChannel, UserType};

pub struct Recording {
    pub filename: String,
    pub time: String,
}

pub struct Connection {
    pub channel: SecureChannel,
    pub user_type: UserType,
}

pub struct Session {
    pub connections: HashMap<String, Connection>,
    pub pending_join: HashMap<String, (Connection, Sender<bool>)>,
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

    pub fn broadcast_all(&mut self, packet: Packet) -> std::io::Result<()> {
        for (_, connection) in &mut self.connections {
            connection.channel.send(packet.clone())?;
        }

        Ok(())
    }

    pub fn broadcast_participants(&mut self, packet: Packet) -> std::io::Result<()> {
        for (_, connection) in &mut self.connections {
            if connection.user_type == UserType::Participant
                || connection.user_type == UserType::Controller
            {
                connection.channel.send(packet.clone())?;
            }
        }

        Ok(())
    }

    pub fn host(&self) -> SecureChannel {
        self.connections
            .iter()
            .find(|(_, conn)| conn.user_type == UserType::Host)
            .unwrap()
            .1
            .channel
            .clone()
    }
}
