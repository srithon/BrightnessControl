use tokio::{
    net::UnixStream,
    io::AsyncWriteExt
};

pub struct SocketMessage {
    message: String,
    log_socket_error: bool
}

pub struct SocketMessageHolder {
    // queue messages into this and then push them all to the socket at the end
    messages: Vec<SocketMessage>,
    socket: UnixStream
}

impl SocketMessageHolder {
    pub fn new(socket: UnixStream) -> SocketMessageHolder {
        SocketMessageHolder {
            messages: Vec::with_capacity(5),
            socket
        }
    }

    pub fn queue_message<T>(&mut self, message: T, log_socket_error: bool)
    where T: Into<String> {
        self.messages.push(
            SocketMessage {
                message: message.into(),
                log_socket_error
            }
        )
    }

    pub fn queue_success<T>(&mut self, message: T)
    where T: Into<String> {
        self.queue_message(message, true)
    }

    pub fn queue_error<T>(&mut self, message: T)
    where T: Into<String> {
        self.queue_message(message, false)
    }

    // NOTE remember to consume before it goes out of scope
    pub fn consume(mut self) {
        // write all messages to the socket
        tokio::spawn(
            async move {
                for (index, message_struct) in self.messages.into_iter().enumerate() {
                    let message = message_struct.message;

                    // add newline separator before every line OTHER than the first line
                    // (so we dont have a blank line in the beginning)
                    if index != 0 {
                        // consider doing something with this information
                        let _ = self.socket.write_all("\n".as_bytes()).await;
                    }

                    if let Err(e) = self.socket.write_all(&message.as_bytes()).await {
                        if message_struct.log_socket_error {
                            eprintln!("Failed to write \"{}\" to socket: {}", message, e);
                        }
                    }
                }

                // cleanup; close connection
                let _ = self.socket.shutdown();
            }
        );
    }
}
