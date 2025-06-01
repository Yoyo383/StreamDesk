use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use aes_gcm::{aead::Aead, Aes256Gcm, Key, KeyInit};
use rsa::{
    pkcs1::{DecodeRsaPublicKey, EncodeRsaPublicKey},
    rand_core::OsRng,
    Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey,
};

use crate::protocol::ProtocolMessage;

/// Represents a secure communication channel over a TCP stream.
///
/// This struct handles the encryption and decryption of messages using AES-256 GCM,
/// with the AES key exchanged securely using RSA encryption during initial handshake.
/// It maintains a nonce counter to ensure unique nonces for each message.
pub struct SecureChannel {
    /// The underlying TCP stream for communication.
    socket: Option<TcpStream>,
    /// An atomic counter used to generate unique nonces for AES-GCM encryption.
    nonce_counter: Arc<AtomicU64>,
    /// The AES-256 GCM cipher used for symmetric encryption of messages.
    cipher: Option<Aes256Gcm>,
    /// A boolean indicating whether this channel instance is operating as a server.
    is_server: bool,
}

impl Clone for SecureChannel {
    /// Creates a new `SecureChannel` by cloning the existing one.
    ///
    /// This involves cloning the underlying `TcpStream` (if present),
    /// the atomic nonce counter, the AES cipher, and the `is_server` flag.
    ///
    /// # Panics
    ///
    /// Panics if the `TcpStream` cannot be cloned (e.g., due to an underlying OS error).
    fn clone(&self) -> Self {
        Self {
            socket: match &self.socket {
                Some(socket) => Some(socket.try_clone().unwrap()),
                None => None,
            },
            nonce_counter: self.nonce_counter.clone(),
            cipher: self.cipher.clone(),
            is_server: self.is_server.clone(),
        }
    }
}

impl SecureChannel {
    /// Creates a new `SecureChannel` instance configured for server-side operation.
    ///
    /// This constructor performs the RSA key generation and the initial handshake
    /// to exchange the AES key if a socket is provided.
    ///
    /// # Arguments
    ///
    /// * `socket` - An `Option<TcpStream>` representing the client connection. If `None`,
    ///              the channel will be created but no handshake will occur.
    ///
    /// # Returns
    ///
    /// A `std::io::Result<Self>` which is:
    /// - `Ok(SecureChannel)` on successful initialization and key exchange.
    /// - `Err(std::io::Error)` if any network or cryptographic operation fails during setup.
    pub fn new_server(socket: Option<TcpStream>) -> std::io::Result<Self> {
        let mut rng = OsRng;
        // Generate a new RSA private key for the server.
        let rsa_private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();

        let mut server = Self {
            socket,
            nonce_counter: Arc::new(AtomicU64::new(1)),
            cipher: None,
            is_server: true,
        };

        // If a socket is provided, perform the key exchange handshake.
        if server.is_connected() {
            server.send_rsa_key(&rsa_private_key)?;
            server.receive_aes_key(&rsa_private_key)?;
        }

        Ok(server)
    }

    /// Creates a new `SecureChannel` instance configured for client-side operation.
    ///
    /// This constructor performs the initial handshake to receive the server's RSA key
    /// and send an encrypted AES key if a socket is provided.
    ///
    /// # Arguments
    ///
    /// * `socket` - An `Option<TcpStream>` representing the connection to the server.
    ///              If `None`, the channel will be created but no handshake will occur.
    ///
    /// # Returns
    ///
    /// A `std::io::Result<Self>` which is:
    /// - `Ok(SecureChannel)` on successful initialization and key exchange.
    /// - `Err(std::io::Error)` if any network or cryptographic operation fails during setup.
    pub fn new_client(socket: Option<TcpStream>) -> std::io::Result<Self> {
        let mut client = Self {
            socket,
            nonce_counter: Arc::new(AtomicU64::new(1)),
            cipher: None,
            is_server: false,
        };

        // If a socket is provided, perform the key exchange handshake.
        if client.is_connected() {
            let rsa_public_key = client.receive_rsa_key()?;
            client.send_aes_key(rsa_public_key)?;
        }

        Ok(client)
    }

    /// Checks if the `SecureChannel` currently has an active TCP connection.
    ///
    /// # Returns
    ///
    /// `true` if `socket` is `Some` (a connection exists), `false` otherwise.
    pub fn is_connected(&self) -> bool {
        self.socket.is_some()
    }

    /// Sends the server's RSA public key to the connected client.
    ///
    /// The public key is serialized into PKCS#1 DER format, length-prefixed,
    /// and then sent over the TCP stream.
    ///
    /// # Arguments
    ///
    /// * `rsa_private_key` - A reference to the server's RSA private key, from which
    ///                       the public key will be derived and sent.
    ///
    /// # Returns
    ///
    /// A `std::io::Result<()>` indicating success or an `std::io::Error` on failure
    /// (e.g., socket write error, key serialization error).
    ///
    /// # Panics
    ///
    /// Panics if the `socket` is `None` (i.e., the channel is not connected).
    fn send_rsa_key(&mut self, rsa_private_key: &RsaPrivateKey) -> std::io::Result<()> {
        let public_key = RsaPublicKey::from(rsa_private_key);
        let key_to_send = public_key
            .to_pkcs1_der()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let len = key_to_send.as_bytes().len() as u32;
        let bytes = key_to_send.as_bytes();

        let socket = self.socket.as_mut().unwrap();

        socket.write_all(&len.to_be_bytes())?;
        socket.write_all(bytes)?;

        Ok(())
    }

    /// Receives an RSA public key from the connected peer.
    ///
    /// The function reads a 4-byte length prefix, then reads that many bytes
    /// to reconstruct the RSA public key from its PKCS#1 DER format.
    ///
    /// # Returns
    ///
    /// A `std::io::Result<RsaPublicKey>` which is:
    /// - `Ok(RsaPublicKey)` on successful reception and parsing of the key.
    /// - `Err(std::io::Error)` if any network read error occurs or the received bytes
    ///   do not form a valid RSA public key.
    ///
    /// # Panics
    ///
    /// Panics if the `socket` is `None` (i.e., the channel is not connected).
    fn receive_rsa_key(&mut self) -> std::io::Result<RsaPublicKey> {
        let socket = self.socket.as_mut().unwrap();

        let mut len_buf = [0u8; 4];
        socket.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf);

        let mut public_key = vec![0u8; len as usize];
        socket.read_exact(&mut public_key)?;

        RsaPublicKey::from_pkcs1_der(&public_key)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    /// Generates an AES key, encrypts it using the provided RSA public key,
    /// and sends the encrypted AES key to the connected peer.
    ///
    /// After sending, the generated AES key is used to initialize the `cipher` field
    /// of the `SecureChannel`.
    ///
    /// # Arguments
    ///
    /// * `rsa_public_key` - The RSA public key of the receiving peer, used to encrypt the AES key.
    ///
    /// # Returns
    ///
    /// A `std::io::Result<()>` indicating success or an `std::io::Error` on failure
    /// (e.g., AES key generation error, RSA encryption error, socket write error).
    ///
    /// # Panics
    ///
    /// Panics if the `socket` is `None` (i.e., the channel is not connected).
    fn send_aes_key(&mut self, rsa_public_key: RsaPublicKey) -> std::io::Result<()> {
        let aes_key = Aes256Gcm::generate_key(OsRng);
        self.cipher = Some(Aes256Gcm::new(&aes_key));

        let encrypted_aes_key = rsa_public_key
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, &aes_key)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let len = encrypted_aes_key.len() as u32;

        let socket = self.socket.as_mut().unwrap();

        socket.write_all(&len.to_be_bytes())?;
        socket.write_all(&encrypted_aes_key)?;

        Ok(())
    }

    /// Receives an encrypted AES key from the connected peer, decrypts it using
    /// the provided RSA private key, and initializes the `cipher` field.
    ///
    /// The function reads a 4-byte length prefix, then reads that many bytes
    /// representing the RSA-encrypted AES key.
    ///
    /// # Arguments
    ///
    /// * `rsa_private_key` - A reference to this channel's RSA private key, used to decrypt the AES key.
    ///
    /// # Returns
    ///
    /// A `std::io::Result<()>` indicating success or an `std::io::Error` on failure
    /// (e.g., socket read error, RSA decryption error, invalid key format).
    ///
    /// # Panics
    ///
    /// Panics if the `socket` is `None` (i.e., the channel is not connected).
    fn receive_aes_key(&mut self, rsa_private_key: &RsaPrivateKey) -> std::io::Result<()> {
        let socket = self.socket.as_mut().unwrap();

        let mut len_buf = [0u8; 4];
        socket.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf);

        let mut encrypted_key = vec![0u8; len as usize];
        socket.read_exact(&mut encrypted_key)?;

        let decrypted_key = rsa_private_key
            .decrypt(Pkcs1v15Encrypt, &encrypted_key)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let key: &Key<Aes256Gcm> = (*decrypted_key).into();

        self.cipher = Some(Aes256Gcm::new(key));

        Ok(())
    }

    /// Generates the next unique 12-byte nonce for AES-GCM encryption.
    ///
    /// The nonce is constructed using an atomic counter. To ensure distinct nonces
    /// between server and client in a session, the first 4 bytes are differentiated
    /// based on the `is_server` flag.
    ///
    /// # Returns
    ///
    /// A `[u8; 12]` array representing the unique nonce.
    fn next_nonce(&mut self) -> [u8; 12] {
        let nonce = self.nonce_counter.fetch_add(1, Ordering::Relaxed);

        // makes server and client have different nonces
        let mut nonce_bytes = if self.is_server { [0u8; 12] } else { [1u8; 12] };
        nonce_bytes[4..].copy_from_slice(&nonce.to_be_bytes());

        nonce_bytes
    }

    /// Encrypts and sends a `ProtocolMessage` over the secure channel.
    ///
    /// The message is first converted to bytes using `ProtocolMessage::to_bytes()`,
    /// then encrypted using AES-256 GCM with a unique nonce. The resulting ciphertext,
    /// prefixed by its length and the nonce, is sent over the TCP stream.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The type of the packet to send, must implement `ProtocolMessage`.
    ///
    /// # Arguments
    ///
    /// * `packet` - The `ProtocolMessage` to be sent.
    ///
    /// # Returns
    ///
    /// A `std::io::Result<()>` indicating success or an `std::io::Error` on failure
    /// (e.g., encryption error, socket write error).
    ///
    /// # Panics
    ///
    /// Panics if the `cipher` or `socket` is `None` (i.e., the channel is not initialized or connected).
    pub fn send<T>(&mut self, packet: T) -> std::io::Result<()>
    where
        T: ProtocolMessage,
    {
        let nonce = self.next_nonce();
        let encrypted = self
            .cipher
            .as_mut()
            .unwrap()
            .encrypt((&nonce).into(), &*packet.to_bytes())
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::Other, "Could not encrypt message")
            })?;

        let len = encrypted.len() as u32;

        let mut to_send = Vec::new();
        to_send.extend_from_slice(&len.to_be_bytes());
        to_send.extend_from_slice(&nonce);
        to_send.extend_from_slice(&encrypted);

        let socket = self.socket.as_mut().unwrap();
        socket.write_all(&to_send)?;

        Ok(())
    }

    /// Receives an encrypted message from the secure channel and decrypts it into a `ProtocolMessage`.
    ///
    /// The function first reads a 4-byte length prefix, then the 12-byte nonce,
    /// and then the encrypted message bytes. It then attempts to decrypt the message
    /// using AES-256 GCM and parse it into the target `ProtocolMessage` type.
    ///
    /// # Type Parameters
    ///
    /// * `T` - The expected type of the received packet, must implement `ProtocolMessage`.
    ///
    /// # Returns
    ///
    /// A `std::io::Result<T>` which is:
    /// - `Ok(packet)` on successful reception, decryption, and parsing.
    /// - `Err(std::io::Error)` if any network read error occurs, decryption fails,
    ///   or the decrypted bytes cannot be parsed into the target `ProtocolMessage` type.
    ///
    /// # Panics
    ///
    /// Panics if the `cipher` or `socket` is `None` (i.e., the channel is not initialized or connected).
    pub fn receive<T>(&mut self) -> std::io::Result<T>
    where
        T: ProtocolMessage,
    {
        let mut len_buf = [0u8; 4];
        let mut nonce = [0u8; 12];
        let socket = self.socket.as_mut().unwrap();

        socket.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf);

        socket.read_exact(&mut nonce)?;

        let mut encrypted = vec![0u8; len as usize];
        socket.read_exact(&mut encrypted)?;

        let decrypted = self
            .cipher
            .as_mut()
            .unwrap()
            .decrypt((&nonce).into(), encrypted.as_ref())
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::Other, "Could not decrypt message")
            })?;

        T::from_bytes(decrypted).ok_or(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Could not parse packet",
        ))
    }

    /// Shuts down the underlying TCP socket, closing both the read and write halves.
    ///
    /// This effectively terminates the connection.
    ///
    /// # Panics
    ///
    /// Panics if the shutdown operation on the socket fails.
    pub fn close(&mut self) {
        if let Some(socket) = &self.socket {
            socket
                .shutdown(std::net::Shutdown::Both)
                .expect("Could not shutdown socket");
        }
    }
}
