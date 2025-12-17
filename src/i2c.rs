use crate::InternalMessage;
use rppal::i2c::I2c;
use std::time::Duration;
use tokio::sync::mpsc;

const ADDR_AIR_REMOTE: u16 = 0x05;

pub(crate) fn blocking_i2c_thread(
    internal_message_tx: mpsc::Sender<InternalMessage>,
    mut i2c_out_rx: mpsc::Receiver<u8>,
) {
    println!("I2C: Connecting");

    let mut i2c = I2c::new().expect("I2C init");

    i2c.set_slave_address(ADDR_AIR_REMOTE)
        .expect("I2C set address");
    i2c.set_timeout(10).expect("I2C set timeout");

    let mut buf = [0u8; 2];

    // Drain any events that were backed up and throw them away, they're probably no longer relevant
    loop {
        i2c.read(&mut buf).expect("I2C read");
        let [code, _data] = buf;
        if code == 0 {
            break;
        }
    }

    println!("I2C: Ready");

    // Now we can actually process events
    loop {
        i2c.read(&mut buf).expect("I2C read");
        let [code, data] = buf;
        if code > 0 {
            if let Some(msg) = match code {
                b'A' => Some(InternalMessage::AsciiKey(data)),
                b'C' => Some(InternalMessage::ConsumerCode(data)),
                b'K' => Some(InternalMessage::KeyCode(data)),
                b'O' => Some(InternalMessage::OkButton),
                b'W' => Some(InternalMessage::PowerButton),
                b'U' => Some(InternalMessage::UsbReadinessStateChange(data == b'Y')),
                _ => None,
            } {
                internal_message_tx
                    .blocking_send(msg)
                    .expect("I2C recv send");
            }
        }

        while let Ok(out) = i2c_out_rx.try_recv() {
            let c = char::from(out);
            println!("I2C: Command {}", c);
            i2c.write(&[out]).expect("I2C write");
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}
