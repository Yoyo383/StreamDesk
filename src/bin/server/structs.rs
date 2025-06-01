use std::{collections::HashMap, sync::mpsc::Sender};

use remote_desktop::{protocol::Packet, secure_channel::SecureChannel, UserType};

/// Represents a recording, with a filename and a timestamp.
pub struct Recording {
    pub filename: String,
    pub time: String,
}

/// Represents a connection to a client with a `SecureChannel` and the user type.
pub struct Connection {
    pub channel: SecureChannel,
    pub user_type: UserType,
}

/// Represents a session with all of the connections and the pending requests.
pub struct Session {
    pub connections: HashMap<String, Connection>,
    pub pending_join: HashMap<String, (Connection, Sender<bool>)>,
}

impl Session {
    /// Creates a new session with a host.
    ///
    /// # Arguments
    ///
    /// * `host_username` - The username of the host.
    /// * `host_conn` - The connection of the host
    ///
    /// # Returns
    ///
    /// The new session created.
    pub fn new(host_username: String, host_conn: Connection) -> Self {
        let mut connections = HashMap::new();
        connections.insert(host_username, host_conn);

        Self {
            connections,
            pending_join: HashMap::new(),
        }
    }

    /// Sends a message to all connections.
    ///
    /// # Arguments
    ///
    /// * `packet` - The message to send.
    ///
    /// # Returns
    ///
    /// An `std::io::Result<()>` that signifies if the message was sent successfully.
    pub fn broadcast_all(&mut self, packet: Packet) -> std::io::Result<()> {
        for (_, connection) in &mut self.connections {
            connection.channel.send(packet.clone())?;
        }

        Ok(())
    }

    /// Sends a message to all participants who are not the host.
    ///
    /// # Arguments
    ///
    /// * `packet` - The message to send.
    ///
    /// # Returns
    ///
    /// An `std::io::Result<()>` that signifies if the message was sent successfully.
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

    /// Finds the connection of the host
    ///
    /// # Returns
    ///
    /// The connection of the host.
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
