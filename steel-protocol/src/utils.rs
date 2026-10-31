//! # Steel Protocol Utils
//! Utility functions and types for the protocol.

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use aes::cipher::{BlockModeDecrypt, BlockSizeUser};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// An AES-128 CFB-8 encryptor.
pub type Aes128Cfb8Enc = cfb8::Encryptor<aes::Aes128>;
/// An AES-128 CFB-8 decryptor.
pub type Aes128Cfb8Dec = cfb8::Decryptor<aes::Aes128>;

/// The maximum size of a packet.
pub const MAX_PACKET_SIZE: usize = 2_097_152;
/// The maximum size of a packet's data.
pub const MAX_PACKET_DATA_SIZE: usize = 8_388_608;

/// Describes the set of packets a connection understands at a given point.
///
/// A connection always starts out in state [`ConnectionProtocol::Handshake`]. In this state,
/// the client sends its desired protocol using [`crate::packets::handshake::SClientIntention`]. The
/// server then either accepts the connection and switches to the desired
/// protocol, or it disconnects the client (for example, in case of an
/// outdated client).
///
/// Each protocol has a `PacketListener` implementation tied to it for
/// server and client respectively.
///
/// Every packet must correspond to exactly one protocol.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ConnectionProtocol {
    /// The handshake protocol. This is the initial protocol, in which the client tells the server its intention (i.e. which protocol it wants to use).
    Handshake,
    /// The play protocol. This is the main protocol that is used while "in game" and most normal packets reside in here.
    Play,
    /// The status protocol. This protocol is used when a client pings a server while on the multiplayer screen.
    Status,
    /// The login protocol. This is the first protocol the client switches to to join a server. It handles authentication with the mojang servers. After it is complete, the connection is switched to the PLAY protocol.
    Login,
    /// The configuration protocol. Used for syncing registered registries.
    Config,
}

/// A raw packet.
#[derive(Debug)]
pub struct RawPacket {
    /// The ID of the packet.
    pub id: i32,
    buffer: Box<[u8]>,
    payload_start: u32,
}

impl RawPacket {
    /// Creates a raw packet from an already-separated payload.
    #[must_use]
    pub fn new(id: i32, payload: Vec<u8>) -> Self {
        Self {
            id,
            buffer: payload.into_boxed_slice(),
            payload_start: 0,
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "packet buffers are limited to MAX_PACKET_DATA_SIZE bytes"
    )]
    pub(crate) fn from_buffer(id: i32, buffer: Vec<u8>, payload_start: usize) -> Self {
        debug_assert!(payload_start <= buffer.len());
        debug_assert!(payload_start <= MAX_PACKET_DATA_SIZE);
        Self {
            id,
            buffer: buffer.into_boxed_slice(),
            payload_start: payload_start as u32,
        }
    }

    /// Returns the packet payload without its packet ID.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.buffer[self.payload_start as usize..]
    }
}

/// An error that can occur when handling packets.
#[derive(Error, Debug)]
pub enum PacketError {
    #[error("failed to decode packet ID")]
    /// Failed to decode the packet ID.
    DecodeID,
    #[error("packet length {0} exceeds maximum length")]
    /// The packet length exceeds the maximum length.
    TooLong(usize),
    #[error("packet length is out of bounds")]
    /// The packet length is out of bounds.
    OutOfBounds,
    #[error("malformed packet length VarInt: {0}")]
    /// The packet length `VarInt` is malformed.
    MalformedLength(String),
    #[error("malformed packet value: {0}")]
    /// A value in the packet is malformed.
    MalformedValue(String),
    #[error("failed to decompress packet: {0}")]
    /// Failed to decompress the packet.
    DecompressionFailed(String),
    #[error("failed to compress packet: {0}")]
    /// Failed to compress the packet.
    CompressionFailed(String),
    #[error("packet is uncompressed but greater than the threshold")]
    /// The packet is uncompressed but greater than the threshold.
    NotCompressed,
    #[error("failed to decrypt packet: {0}")]
    /// Failed to decrypt the packet.
    DecryptionFailed(String),
    #[error("failed to encrypt packet: {0}")]
    /// Failed to encrypt the packet.
    EncryptionFailed(String),
    #[error("the connection has closed")]
    /// The connection has closed.
    ConnectionClosed,
    #[error("{0}")]
    /// An error occurred when sending a packet.
    SendError(String),
    #[error("Error: {0}")]
    /// An other error occurred.
    Other(String),
    #[error("Invalid protocol: {0}")]
    /// The protocol is invalid.
    InvalidProtocol(String),
}

impl From<io::Error> for PacketError {
    fn from(value: io::Error) -> Self {
        //Todo! Define & Handle all cases
        Self::MalformedValue(value.to_string())
    }
}

///NOTE: Buffered writes go through `pending` when the inner writer stalls; make sure there is a buffer somewhere down the line
pub struct StreamEncryptor<W: AsyncWrite + Unpin> {
    cipher: Aes128Cfb8Enc,
    write: W,
    /// Encrypted bytes the inner writer has not accepted yet.
    pending: Vec<u8>,
}

impl<W: AsyncWrite + Unpin> StreamEncryptor<W> {
    /// Creates a new `StreamEncryptor`.
    pub const fn new(cipher: Aes128Cfb8Enc, stream: W) -> Self {
        Self {
            cipher,
            write: stream,
            pending: Vec::new(),
        }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for StreamEncryptor<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let ref_self = self.get_mut();

        if !buf.is_empty() {
            // Encrypt the whole buffer in one pass, mirroring `StreamDecryptor::poll_read`.
            // CFB8 feedback is strictly sequential, so encrypting here is safe even if the
            // inner writer accepts the bytes across several polls.
            let start = ref_self.pending.len();
            ref_self.pending.extend_from_slice(buf);
            ref_self.cipher.encrypt(&mut ref_self.pending[start..]);
        }
        if ref_self.pending.is_empty() {
            return Poll::Ready(Ok(0));
        }

        // The pending buffer is handed to the inner writer as a shared slice, so take the
        // accepted length first and then drop the written prefix.
        let write = Pin::new(&mut ref_self.write);
        match write.poll_write(cx, &ref_self.pending) {
            Poll::Ready(Ok(written)) => {
                ref_self.pending.drain(..written);
                Poll::Ready(Ok(buf.len()))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            // `buf` was already encrypted into `pending`, so it is consumed even though the
            // inner writer stalled; `poll_flush` drains the rest.
            Poll::Pending => Poll::Ready(Ok(buf.len())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let ref_self = self.get_mut();
        while !ref_self.pending.is_empty() {
            let write = Pin::new(&mut ref_self.write);
            match write.poll_write(cx, &ref_self.pending) {
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to flush buffered encrypted data",
                    )));
                }
                Poll::Ready(Ok(written)) => {
                    ref_self.pending.drain(..written);
                }
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => return Poll::Pending,
            }
        }
        let write = Pin::new(&mut ref_self.write);
        write.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let ref_self = self.get_mut();
        let write = Pin::new(&mut ref_self.write);
        write.poll_shutdown(cx)
    }
}

/// A stream that decrypts data.
pub struct StreamDecryptor<R: AsyncRead + Unpin> {
    cipher: Aes128Cfb8Dec,
    read: R,
}

impl<R: AsyncRead + Unpin> StreamDecryptor<R> {
    /// Creates a new `StreamDecryptor`.
    pub const fn new(cipher: Aes128Cfb8Dec, stream: R) -> Self {
        Self {
            cipher,
            read: stream,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for StreamDecryptor<R> {
    #[expect(
        clippy::unwrap_used,
        reason = "CFB8 block size is one byte, so each chunk fits the cipher block type"
    )]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let ref_self = self.get_mut();
        let read = Pin::new(&mut ref_self.read);
        let cipher = &mut ref_self.cipher;

        // Get the starting position
        let original_fill = buf.filled().len();
        // Read the raw data
        let internal_poll = read.poll_read(cx, buf);

        if matches!(internal_poll, Poll::Ready(Ok(()))) {
            // Decrypt the raw data in-place, note that our block size is 1 byte, so this is always safe
            for block in buf.filled_mut()[original_fill..].chunks_mut(Aes128Cfb8Dec::block_size()) {
                cipher.decrypt_block(block.try_into().unwrap());
            }
        }

        internal_poll
    }
}

#[cfg(test)]
mod tests {
    use aes::cipher::KeyIvInit;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    use super::*;

    fn cipher_pair() -> (Aes128Cfb8Enc, Aes128Cfb8Dec) {
        let key = [0x5Au8; 16];
        (
            Aes128Cfb8Enc::new_from_slices(&key, &key).expect("test key should initialize"),
            Aes128Cfb8Dec::new_from_slices(&key, &key).expect("test key should initialize"),
        )
    }

    #[tokio::test]
    async fn encryptor_write_round_trips_with_decryptor_read() {
        let (encrypt_cipher, decrypt_cipher) = cipher_pair();
        let (mut client_end, server_end) = duplex(64 * 1024);
        let mut encryptor = StreamEncryptor::new(encrypt_cipher, server_end);

        let plaintext: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        // Split writes exercise the carry of the CFB8 feedback across `poll_write` calls.
        encryptor
            .write_all(&plaintext[..137])
            .await
            .expect("first encrypted write should succeed");
        encryptor
            .write_all(&plaintext[137..])
            .await
            .expect("second encrypted write should succeed");
        encryptor
            .flush()
            .await
            .expect("encrypted flush should succeed");
        drop(encryptor);

        let mut encrypted = Vec::new();
        client_end
            .read_to_end(&mut encrypted)
            .await
            .expect("encrypted bytes should be readable");

        let mut decryptor = StreamDecryptor::new(decrypt_cipher, encrypted.as_slice());
        let mut decrypted = Vec::new();
        decryptor
            .read_to_end(&mut decrypted)
            .await
            .expect("decryption should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[tokio::test]
    async fn encryptor_whole_buffer_write_matches_per_byte_encryption() {
        let (encrypt_cipher, _) = cipher_pair();
        let plaintext: Vec<u8> = (0..=255u8).cycle().take(611).collect();

        let mut reference_cipher = Aes128Cfb8Enc::new_from_slices(&[0x5Au8; 16], &[0x5Au8; 16])
            .expect("reference cipher should initialize");
        let mut expected = plaintext.clone();
        reference_cipher.encrypt(&mut expected);

        let (mut client_end, server_end) = duplex(64 * 1024);
        let mut encryptor = StreamEncryptor::new(encrypt_cipher, server_end);
        encryptor
            .write_all(&plaintext)
            .await
            .expect("encrypted write should succeed");
        encryptor
            .flush()
            .await
            .expect("encrypted flush should succeed");
        drop(encryptor);

        let mut encrypted = Vec::new();
        client_end
            .read_to_end(&mut encrypted)
            .await
            .expect("encrypted bytes should be readable");
        assert_eq!(encrypted, expected);
    }

    #[tokio::test]
    async fn encryptor_flush_drains_bytes_a_stalled_writer_buffered() {
        let (encrypt_cipher, decrypt_cipher) = cipher_pair();
        // A tiny duplex forces the inner writer to accept fewer bytes than a packet holds.
        let (mut client_end, server_end) = duplex(64);
        let mut encryptor = StreamEncryptor::new(encrypt_cipher, server_end);

        let plaintext: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
        encryptor
            .write_all(&plaintext)
            .await
            .expect("write should buffer through the stalled writer");

        let reader = tokio::spawn(async move {
            let mut encrypted = Vec::new();
            client_end
                .read_to_end(&mut encrypted)
                .await
                .expect("encrypted bytes should be readable");
            encrypted
        });
        encryptor
            .flush()
            .await
            .expect("flush should drain the buffered encrypted bytes");
        drop(encryptor);
        let encrypted = reader.await.expect("reader task should finish");

        let mut decryptor = StreamDecryptor::new(decrypt_cipher, encrypted.as_slice());
        let mut decrypted = Vec::new();
        decryptor
            .read_to_end(&mut decrypted)
            .await
            .expect("decryption should succeed");
        assert_eq!(decrypted, plaintext);
    }
}
