use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, error::TryRecvError};

struct MessageWithTimestamp<T> {
    message: T,
    timestamp: Instant,
}

struct TransactionalReceiver<T> {
    receiver: mpsc::Receiver<T>,
    current_message: Option<MessageWithTimestamp<T>>,
    maximum_duration: Duration,
}

impl<T> TransactionalReceiver<T> {
    fn new(receiver: mpsc::Receiver<T>, maximum_duration: Duration) -> TransactionalReceiver<T> {
        return TransactionalReceiver {
            receiver,
            current_message: None,
            maximum_duration,
        };
    }

    fn try_recv_vs_timestamp(&mut self, now: &Instant) -> Result<&T, TryRecvError> {
        let mut timed_out = false;
        if let Some(MessageWithTimestamp { timestamp, .. }) = self.current_message {
            if *now - timestamp > self.maximum_duration {
                timed_out = true;
            }
        }

        if timed_out {
            self.current_message = None;
        }

        if self.current_message.is_none() {
            let next_message_maybe = self.receiver.try_recv();
            return match next_message_maybe {
                Ok(next_message) => {
                    self.current_message = Some(MessageWithTimestamp {
                        message: next_message,
                        timestamp: Instant::now(),
                    });
                    match &self.current_message {
                        Some(MessageWithTimestamp { message, .. }) => Ok(&message),
                        None => unreachable!(),
                    }
                }
                Err(error) => Err(error),
            };
        }

        if let Some(MessageWithTimestamp { message, .. }) = &self.current_message {
            return Ok(&message);
        }

        unreachable!();
    }

    fn try_recv(&mut self) -> Result<&T, TryRecvError> {
        self.try_recv_vs_timestamp(&Instant::now())
    }

    fn commit(&mut self) {
        match self.current_message {
            Some(_) => {
                self.current_message = None;
            }
            None => panic!("Cannot commit, no message currently being handled!"),
        }
    }
}
