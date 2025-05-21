use remote_desktop::{protocol::Packet, secure_channel::SecureChannel, UserType};

use crate::{structs::*, SharedSession};

pub fn handle_participant(channel: &mut SecureChannel, session: SharedSession, username: String) {
    loop {
        let packet = channel.receive().unwrap();

        match packet {
            Packet::Control { .. } => {
                let session = session.lock().unwrap();

                if session.connections.get(&username).unwrap().connection_type
                    == ConnectionType::Controller
                {
                    session.host().send(packet).unwrap();
                }
            }

            Packet::RequestControl { .. } => {
                let session = session.lock().unwrap();

                // can send to host only if participant, not unready
                if session.connections.get(&username).unwrap().connection_type
                    == ConnectionType::Participant
                {
                    session.host().send(packet).unwrap();
                } else {
                    // send DenyRequest because not participant
                    let deny_packet = Packet::DenyControl {
                        username: username.clone(),
                    };
                    channel.send(deny_packet).unwrap();
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

                channel.send(Packet::SessionExit).unwrap();

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
