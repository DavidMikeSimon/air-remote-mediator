// Based on https://github.com/andrewrabert/sony-bravia-cli

use crate::InternalMessage;
use std::io::{Error, ErrorKind};
use std::time::Duration;
use tokio::sync::mpsc;

const CONTROL_REQUEST: u8 = 0x8c;
const QUERY_REQUEST: u8 = 0x83;
const CATEGORY: u8 = 0x00;
const POWER_FUNCTION: u8 = 0x00;
const INPUT_SELECT_FUNCTION: u8 = 0x02;
const VOLUME_CONTROL_FUNCTION: u8 = 0x05;
const PICTURE_FUNCTION: u8 = 0x0d;
const DISPLAY_FUNCTION: u8 = 0x0f;
const BRIGHTNESS_CONTROL_FUNCTION: u8 = 0x24;
const MUTING_FUNCTION: u8 = 0x06;

const INPUT_TYPE_HDMI: u8 = 0x04;

const RESPONSE_HEADER: u8 = 0x70;
const RESPONSE_ANSWER: u8 = 0x00;

fn checksum(command: &[u8]) -> u8 {
    let s: u8 = command.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    s % 255
}

fn write_command(
    port: &mut dyn serialport::SerialPort,
    contents: Vec<u8>,
) -> Result<Vec<u8>, std::io::Error> {
    let mut vec = contents.clone();
    let c = checksum(&vec);
    vec.push(c);
    port.write_all(&vec).unwrap();

    let mut resp_buf = vec![0; 3];
    port.read_exact(resp_buf.as_mut_slice())?;

    if resp_buf[0] != RESPONSE_HEADER {
        return Err(Error::new(ErrorKind::Other, "unexpected response header"));
    }
    if resp_buf[1] != RESPONSE_ANSWER {
        return Err(Error::new(ErrorKind::Other, "unexpected response answer"));
    }
    if vec[0] == QUERY_REQUEST {
        let mut resp_data_buf = vec![0; resp_buf[2] as usize];
        port.read_exact(resp_data_buf.as_mut_slice())?;
        let resp_checksum = resp_data_buf
            .pop()
            .ok_or(Error::new(ErrorKind::Other, "empty response checksum"))?;
        resp_buf.extend(resp_data_buf.clone());
        if resp_checksum != checksum(&resp_buf) {
            return Err(Error::new(ErrorKind::Other, "invalid response checksum"));
        }
        Ok(resp_data_buf)
    } else {
        let resp_checksum = resp_buf
            .pop()
            .ok_or(Error::new(ErrorKind::Other, "empty response checksum"))?;
        if resp_checksum != checksum(&resp_buf) {
            return Err(Error::new(ErrorKind::Other, "invalid response checksum"));
        }
        Ok(vec![0; 0])
    }
}

fn is_powered_on(port: &mut dyn serialport::SerialPort) -> Result<bool, std::io::Error> {
    let args = vec![QUERY_REQUEST, CATEGORY, POWER_FUNCTION, 0xff, 0xff];
    return write_command(port, args).map(|data| data[0] == 1);
}

fn serial_loop(
    port: &mut dyn serialport::SerialPort,
    internal_message_tx: &mpsc::Sender<InternalMessage>,
    serial_out_rx: &mut mpsc::Receiver<u8>,
) -> Result<(), std::io::Error> {
    loop {
        let start = std::time::Instant::now();
        let power = is_powered_on(&mut *port)?;
        println!("POWER: {:?} ({}ms)", power, start.elapsed().as_millis());
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub(crate) fn serial_thread(
    internal_message_tx: mpsc::Sender<InternalMessage>,
    mut serial_out_rx: mpsc::Receiver<u8>,
) {
    loop {
        println!("Connecting to Sony serial");

        let mut port = serialport::new("/dev/ttyUSB0", 9600)
            .timeout(Duration::from_millis(500))
            .open()
            .expect("Opening serial port");

        let exit = serial_loop(&mut *port, &internal_message_tx, &mut serial_out_rx);
        if let Err(error) = exit {
            println!("Serial connection lost: {}", error);
        }

        std::thread::sleep(Duration::from_secs(1));
    }
}
