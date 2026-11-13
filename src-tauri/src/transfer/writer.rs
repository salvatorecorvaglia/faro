//! A single blocking disk writer, with bounded async back-pressure.
use crate::error::{FaroError, Result};
use std::{io::Write, path::PathBuf};
use tokio_util::sync::CancellationToken;
enum Message {
    Data(Vec<u8>),
    Commit,
}
pub struct DiskWriter {
    sender: tokio::sync::mpsc::Sender<Message>,
    task: Option<tokio::task::JoinHandle<Result<u64>>>,
    cancel: CancellationToken,
}
impl DiskWriter {
    pub fn new(path: PathBuf, cancel: CancellationToken) -> Self {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        let token = cancel.clone();
        let task = tokio::task::spawn_blocking(move || {
            let output = super::PartialFile::new(&path)?;
            let mut writer = std::io::BufWriter::new(super::create_output(output.path())?);
            let mut bytes = 0u64;
            while let Some(message) = receiver.blocking_recv() {
                if token.is_cancelled() {
                    return Err(FaroError::Cancelled);
                }
                match message {
                    Message::Data(data) => {
                        writer.write_all(&data)?;
                        bytes += data.len() as u64;
                    }
                    Message::Commit => {
                        writer.flush()?;
                        drop(writer);
                        if token.is_cancelled() {
                            return Err(FaroError::Cancelled);
                        }
                        output.commit()?;
                        return Ok(bytes);
                    }
                }
            }
            Err(FaroError::Other(
                "The output producer stopped before completing the file".into(),
            ))
        });
        Self {
            sender,
            task: Some(task),
            cancel,
        }
    }
    async fn send(&mut self, message: Message) -> Result<()> {
        let sent = tokio::select! { biased;
            _ = self.cancel.cancelled() => return Err(FaroError::Cancelled),
            sent = self.sender.send(message) => sent,
        };
        if sent.is_err() {
            let task = self
                .task
                .take()
                .ok_or_else(|| FaroError::Io("The output writer has stopped".into()))?;
            return match task
                .await
                .map_err(|error| FaroError::Io(error.to_string()))?
            {
                Err(error) => Err(error),
                Ok(_) => Err(FaroError::Io("The output writer has stopped".into())),
            };
        }
        Ok(())
    }
    pub async fn write(&mut self, bytes: Vec<u8>) -> Result<()> {
        for chunk in bytes.chunks(64 * 1024) {
            self.send(Message::Data(chunk.to_vec())).await?;
        }
        Ok(())
    }
    pub async fn finish(mut self) -> Result<u64> {
        self.send(Message::Commit).await?;
        self.task
            .take()
            .ok_or_else(|| FaroError::Io("The output writer has stopped".into()))?
            .await
            .map_err(|error| FaroError::Io(error.to_string()))?
    }
}

#[derive(Clone, Default)]
pub struct Buffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl Buffer {
    pub fn len(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|error| error.into_inner()))
    }
}
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut buffer = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if buffer.len().saturating_add(bytes.len()) > crate::budget::MATERIALIZED_BYTES {
            return Err(std::io::Error::other(
                "An encoded output page exceeds Faro's memory budget",
            ));
        }
        buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
