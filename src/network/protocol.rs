//! Wire protocol for GrabNet P2P communication

use std::io;
use async_trait::async_trait;
use futures::prelude::*;
use libp2p::request_response;
use libp2p::StreamProtocol;

use crate::types::{GrabRequest, GrabResponse};

/// Protocol identifier
pub const PROTOCOL_NAME: StreamProtocol = StreamProtocol::new("/grabnet/1.0.0");

/// GrabNet protocol definition
#[derive(Debug, Clone)]
pub struct GrabProtocol;

impl AsRef<str> for GrabProtocol {
    fn as_ref(&self) -> &str {
        "/grabnet/1.0.0"
    }
}

/// Codec for encoding/decoding messages
#[derive(Debug, Clone, Default)]
pub struct GrabCodec;

#[async_trait]
impl request_response::Codec for GrabCodec {
    type Protocol = StreamProtocol;
    type Request = GrabRequest;
    type Response = GrabResponse;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        // Read length prefix (4 bytes, big endian)
        let mut len_buf = [0u8; 4];
        io.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        // Sanity check
        if len > 100 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Message too large",
            ));
        }

        // Read message
        let mut buf = vec![0u8; len];
        io.read_exact(&mut buf).await?;

        // Decode
        bincode::deserialize(&buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        // Read length prefix
        let mut len_buf = [0u8; 4];
        io.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;

        if len > 100 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Message too large",
            ));
        }

        let mut buf = vec![0u8; len];
        io.read_exact(&mut buf).await?;

        bincode::deserialize(&buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        request: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let buf = bincode::serialize(&request)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // Write length prefix
        let len = (buf.len() as u32).to_be_bytes();
        io.write_all(&len).await?;
        io.write_all(&buf).await?;
        io.flush().await?;

        Ok(())
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        response: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let buf = bincode::serialize(&response)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let len = (buf.len() as u32).to_be_bytes();
        io.write_all(&len).await?;
        io.write_all(&buf).await?;
        io.flush().await?;

        Ok(())
    }
}
