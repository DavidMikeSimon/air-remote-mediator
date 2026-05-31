use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, error::TryRecvError};

struct MessageWithTimestamp<T> {
    message: T,
    timestamp: Instant,
}

pub struct TransactionalReceiver<T> {
    receiver: mpsc::Receiver<T>,
    current_message: Option<MessageWithTimestamp<T>>,
    maximum_duration: Duration,
}

impl<T> TransactionalReceiver<T> {
    pub fn new(
        receiver: mpsc::Receiver<T>,
        maximum_duration: Duration,
    ) -> TransactionalReceiver<T> {
        return TransactionalReceiver {
            receiver,
            current_message: None,
            maximum_duration,
        };
    }

    pub fn try_recv_vs_timestamp(&mut self, now: &Instant) -> Result<&T, TryRecvError> {
        if let Some(MessageWithTimestamp { timestamp, .. }) = self.current_message
            && *now - timestamp > self.maximum_duration
        {
            self.current_message = None
        }

        if self.current_message.is_none() {
            let next_message_maybe = self.receiver.try_recv();
            match next_message_maybe {
                Ok(next_message) => {
                    self.current_message = Some(MessageWithTimestamp {
                        message: next_message,
                        timestamp: *now,
                    });
                }
                Err(error) => return Err(error),
            };
        }

        match &self.current_message {
            Some(MessageWithTimestamp { message, .. }) => Ok(&message),
            None => unreachable!(), // Message should always have been set above
        }
    }

    pub fn try_recv(&mut self) -> Result<&T, TryRecvError> {
        self.try_recv_vs_timestamp(&Instant::now())
    }

    pub fn commit(&mut self) {
        match self.current_message {
            Some(_) => {
                self.current_message = None;
            }
            None => panic!("Cannot commit, no message currently being handled!"),
        }
    }
}
