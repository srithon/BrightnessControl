use crate::shared::{DaemonResponse, ExitStatus, BINCODE_OPTIONS};
use bincode::Options;
use tokio::{io::AsyncWriteExt, net::UnixStream};

pub struct SocketMessage {
    message: String,
    is_error: bool,
}

pub struct SocketMessageHolder {
    // queue messages into this and then push them all to the socket at the end
    messages: Vec<SocketMessage>,
    socket: UnixStream,
}

impl SocketMessageHolder {
    pub fn new(socket: UnixStream) -> SocketMessageHolder {
        SocketMessageHolder {
            messages: Vec::with_capacity(5),
            socket,
        }
    }

    pub fn queue_message<T>(&mut self, message: T, is_error: bool)
    where
        T: Into<String>,
    {
        self.messages.push(SocketMessage {
            message: message.into(),
            is_error: is_error,
        })
    }

    pub fn queue_success<T>(&mut self, message: T)
    where
        T: Into<String>,
    {
        self.queue_message(message, false)
    }

    pub fn queue_error<T>(&mut self, message: T)
    where
        T: Into<String>,
    {
        self.queue_message(message, true)
    }

    // NOTE remember to consume before it goes out of scope
    pub fn consume(mut self) {
        tokio::spawn(async move {
            // Determine exit status based on whether any messages were errors
            let exit_status = if self.messages.iter().any(|msg| msg.is_error) {
                ExitStatus::Failure
            } else {
                ExitStatus::Success
            };

            // Convert messages into Vec<String>
            let messages = self
                .messages
                .into_iter()
                .map(|msg| msg.message)
                .collect::<Vec<_>>();

            let response = DaemonResponse {
                exit_status,
                messages,
            };

            // Serialize and send the response
            if let Ok(encoded_response) = BINCODE_OPTIONS.serialize(&response) {
                if let Err(e) = self.socket.write_all(&encoded_response).await {
                    eprintln!("Failed to write response to socket: {e}");
                }
            }

            // cleanup; close connection
            let _ = self.socket.shutdown().await;
        });
    }
}
