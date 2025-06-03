use stream_desk::{protocol::Packet, secure_channel::SecureChannel, UserType};

use crate::SharedSession;

/// Handles packets from the client.
///
/// # Arguments
///
/// * `channel` - A `SecureChannel` connected to the client.
/// * `session` - The `Session` object that the user is connected to.
/// * `username` - The username of the client.
///
/// # Returns
///
/// An `std::io::Result<()>` that signifies if something went wrong.
pub fn handle_participant(
    channel: &mut SecureChannel,
    session: SharedSession,
    username: String,
) -> std::io::Result<()> {
    loop {
        let packet = channel.receive().unwrap_or_default();

        match packet {
            Packet::Control { .. } => {
                let session = session.lock().unwrap();

                if session.connections.get(&username).unwrap().user_type == UserType::Controller {
                    session.host().send(packet)?;
                }
            }

            Packet::RequestControl { .. } => {
                let session = session.lock().unwrap();

                // can send to host only if participant, not unready
                if session.connections.get(&username).unwrap().user_type == UserType::Participant {
                    session.host().send(packet)?;
                } else {
                    // send DenyRequest because not participant
                    let deny_packet = Packet::DenyControl {
                        username: username.clone(),
                    };
                    channel.send(deny_packet)?;
                }
            }

            Packet::SessionExit | Packet::None => {
                let mut session = session.lock().unwrap();
                session.connections.remove(&username);

                let user_update_packet = Packet::UserUpdate {
                    user_type: UserType::Leaving,
                    joined_before: false,
                    username: username.clone(),
                };
                session.broadcast_all(user_update_packet)?;

                channel.send(Packet::SessionExit)?;

                break;
            }

            Packet::Chat { message } => {
                let message = username.to_string() + ": " + &message;
                let packet = Packet::Chat { message };

                let mut session = session.lock().unwrap();
                session.broadcast_all(packet)?;
            }

            Packet::SessionEnd => break,

            _ => (),
        }
    }

    Ok(())
}
