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

pub struct SecureChannel {
    socket: TcpStream,
    nonce_counter: Arc<AtomicU64>,
    rsa_private_key: Option<RsaPrivateKey>,
    cipher: Option<Aes256Gcm>,
    is_server: bool,
}

impl Clone for SecureChannel {
    fn clone(&self) -> Self {
        Self {
            socket: self.socket.try_clone().unwrap(),
            nonce_counter: self.nonce_counter.clone(),
            rsa_private_key: self.rsa_private_key.clone(),
            cipher: self.cipher.clone(),
            is_server: self.is_server.clone(),
        }
    }
}

impl SecureChannel {
    pub fn new_server(socket: TcpStream) -> std::io::Result<Self> {
        let mut rng = OsRng;
        let rsa_private_key = Some(RsaPrivateKey::new(&mut rng, 2048).unwrap());

        let mut server = Self {
            socket,
            nonce_counter: Arc::new(AtomicU64::new(1)),
            rsa_private_key,
            cipher: None,
            is_server: true,
        };

        server.send_rsa_key()?;
        server.receive_aes_key()?;

        Ok(server)
    }

    pub fn new_client(socket: TcpStream) -> std::io::Result<Self> {
        let mut client = Self {
            socket,
            nonce_counter: Arc::new(AtomicU64::new(1)),
            rsa_private_key: None,
            cipher: None,
            is_server: false,
        };

        let rsa_public_key = client.receive_rsa_key()?;
        client.send_aes_key(rsa_public_key)?;

        Ok(client)
    }

    fn send_rsa_key(&mut self) -> std::io::Result<()> {
        let public_key = RsaPublicKey::from(self.rsa_private_key.as_ref().unwrap());
        let key_to_send = public_key
            .to_pkcs1_der()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let len = key_to_send.as_bytes().len() as u32;
        let bytes = key_to_send.as_bytes();

        self.socket.write_all(&len.to_be_bytes())?;
        self.socket.write_all(bytes)?;

        Ok(())
    }

    fn receive_rsa_key(&mut self) -> std::io::Result<RsaPublicKey> {
        let mut len_buf = [0u8; 4];
        self.socket.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf);

        let mut public_key = vec![0u8; len as usize];
        self.socket.read_exact(&mut public_key)?;

        RsaPublicKey::from_pkcs1_der(&public_key)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    fn send_aes_key(&mut self, rsa_public_key: RsaPublicKey) -> std::io::Result<()> {
        let aes_key = Aes256Gcm::generate_key(OsRng);
        self.cipher = Some(Aes256Gcm::new(&aes_key));

        let encrypted_aes_key = rsa_public_key
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, &aes_key)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let len = encrypted_aes_key.len() as u32;

        self.socket.write_all(&len.to_be_bytes())?;
        self.socket.write_all(&encrypted_aes_key)?;

        Ok(())
    }

    fn receive_aes_key(&mut self) -> std::io::Result<()> {
        let mut len_buf = [0u8; 4];
        self.socket.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf);

        let mut encrypted_key = vec![0u8; len as usize];
        self.socket.read_exact(&mut encrypted_key)?;

        let decrypted_key = self
            .rsa_private_key
            .as_ref()
            .unwrap()
            .decrypt(Pkcs1v15Encrypt, &encrypted_key)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let key: &Key<Aes256Gcm> = (*decrypted_key).into();

        self.cipher = Some(Aes256Gcm::new(key));

        Ok(())
    }

    fn next_nonce(&mut self) -> [u8; 12] {
        let nonce = self.nonce_counter.fetch_add(1, Ordering::Relaxed);

        // makes server and client have different nonces
        let mut nonce_bytes = if self.is_server { [0u8; 12] } else { [1u8; 12] };
        nonce_bytes[4..].copy_from_slice(&nonce.to_be_bytes());

        nonce_bytes
    }

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

        self.socket.write_all(&to_send)?;

        Ok(())
    }

    pub fn receive<T>(&mut self) -> std::io::Result<T>
    where
        T: ProtocolMessage,
    {
        let mut len_buf = [0u8; 4];
        let mut nonce = [0u8; 12];

        self.socket.read_exact(&mut len_buf)?;
        let len = u32::from_be_bytes(len_buf);

        self.socket.read_exact(&mut nonce)?;

        let mut encrypted = vec![0u8; len as usize];
        self.socket.read_exact(&mut encrypted)?;

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

    pub fn close(&mut self) {
        self.socket
            .shutdown(std::net::Shutdown::Both)
            .expect("Could not shutdown socket");
    }
}
