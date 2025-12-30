//! Async I/O support (requires "async" feature)
//!
//! Provides async boundaries for reading from and writing to async sources/sinks.
//! Uses `spawn_blocking` to prevent CPU-bound GIF processing from blocking the tokio runtime.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Error, Gif, Result};

impl Gif {
    /// Decode from an async reader.
    ///
    /// Buffers the entire input to memory, then spawns a blocking task to decode.
    /// This prevents the CPU-bound decode operation from blocking the async runtime.
    ///
    /// # Example
    /// ```ignore
    /// let gif = Gif::from_async_read(s3_stream).await?;
    /// ```
    pub async fn from_async_read<R: AsyncRead + Unpin>(mut reader: R) -> Result<Self> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.map_err(Error::IoError)?;

        tokio::task::spawn_blocking(move || Self::from_bytes(&buf))
            .await
            .map_err(|e| Error::IoError(std::io::Error::other(e)))?
    }

    /// Encode to an async writer.
    ///
    /// Spawns a blocking task to encode to bytes, then writes asynchronously.
    /// This prevents the CPU-bound encode operation from blocking the async runtime.
    ///
    /// # Example
    /// ```ignore
    /// gif.encode_to_async_write(&mut tcp_stream).await?;
    /// ```
    pub async fn encode_to_async_write<W: AsyncWrite + Unpin>(&self, mut writer: W) -> Result<()> {
        let bytes = self.to_bytes()?;
        writer.write_all(&bytes).await.map_err(Error::IoError)?;
        Ok(())
    }
}
