// Based on https://github.com/andrewrabert/sony-bravia-cli

use crate::{InternalMessage, TvState};
use std::io::{Error, ErrorKind};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug)]
pub(crate) enum SerialCommand {
    VolumeUp,
    VolumeDown,
    PowerOn,
    PowerOff,
    SelectInput(u8),
}

pub(crate) fn blocking_serial_thread(
    internal_message_tx: mpsc::Sender<InternalMessage>,
    mut serial_out_rx: mpsc::Receiver<SerialCommand>,
) {
    loop {
        println!("Connecting to Sony serial");

        let mut port = serialport::new("/dev/ttyUSB0", 9600)
            .timeout(Duration::from_millis(800))
            .open()
            .expect("Opening serial port");

        let exit = serial_loop(&mut *port, &internal_message_tx, &mut serial_out_rx);
        if let Err(error) = exit {
            println!("Serial connection lost: {}", error);
        }

        std::thread::sleep(Duration::from_secs(1));
    }
}

fn serial_loop(
    port: &mut dyn serialport::SerialPort,
    internal_message_tx: &mpsc::Sender<InternalMessage>,
    serial_out_rx: &mut mpsc::Receiver<SerialCommand>,
) -> Result<(), std::io::Error> {
    // FIXME: The initial power query right after connecting seems
    // unreliable.
    loop {
        internal_message_tx
            .blocking_send(InternalMessage::UpdateTvState(get_state(&mut *port)?))
            .expect("Serial TV state send");

        while let Ok(cmd) = serial_out_rx.try_recv() {
            println!("Sending Serial command: {:?}", cmd);
            match cmd {
                SerialCommand::VolumeUp => volume_up(&mut *port)?,
                SerialCommand::VolumeDown => volume_down(&mut *port)?,
                SerialCommand::PowerOn => power_on(&mut *port)?,
                SerialCommand::PowerOff => power_off(&mut *port)?,
                SerialCommand::SelectInput(input) => select_input(&mut *port, input)?,
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

const CONTROL_REQUEST: u8 = 0x8c;
const QUERY_REQUEST: u8 = 0x83;
const CATEGORY: u8 = 0x00;
const POWER_FUNCTION: u8 = 0x00;
const STANDBY_FUNCTION: u8 = 0x01;
const INPUT_SELECT_FUNCTION: u8 = 0x02;
const VOLUME_CONTROL_FUNCTION: u8 = 0x05;
const PICTURE_FUNCTION: u8 = 0x0d;
const DISPLAY_FUNCTION: u8 = 0x0f;
const BRIGHTNESS_CONTROL_FUNCTION: u8 = 0x24;
const MUTING_FUNCTION: u8 = 0x06;
const SIRCS_FUNCTION: u8 = 0x67;

const SIRCS_INPUT: (u8, u8) = (0x01, 0x25);

const INPUT_TYPE_HDMI: u8 = 0x04;

const RESPONSE_HEADER: u8 = 0x70;
const RESPONSE_CODE_OK: u8 = 0x00;

fn checksum(command: &[u8]) -> u8 {
    let s: u8 = command.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    s % 255
}

fn write_request(
    port: &mut dyn serialport::SerialPort,
    contents: Vec<u8>,
) -> Result<Vec<u8>, std::io::Error> {
    let mut vec = contents.clone();
    let c = checksum(&vec);
    vec.push(c);
    port.write_all(&vec)?;

    let mut resp_buf = vec![0; 3];
    port.read_exact(resp_buf.as_mut_slice())?;

    if resp_buf[0] != RESPONSE_HEADER {
        return Err(Error::new(
            ErrorKind::Other,
            format!("unexpected response header {}", resp_buf[0]),
        ));
    }
    if resp_buf[1] != RESPONSE_CODE_OK {
        return Err(Error::new(
            ErrorKind::Other,
            format!("unexpected response answer {}", resp_buf[1]),
        ));
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

fn write_command(
    port: &mut dyn serialport::SerialPort,
    contents: Vec<u8>,
) -> Result<(), std::io::Error> {
    write_request(port, contents).map(|_| ())
}

fn is_powered_on(port: &mut dyn serialport::SerialPort) -> Result<bool, std::io::Error> {
    write_request(
        port,
        vec![QUERY_REQUEST, CATEGORY, POWER_FUNCTION, 0xff, 0xff],
    )
    .map(|data| data.get(0) == Some(&1))
}

fn get_current_input(port: &mut dyn serialport::SerialPort) -> Result<(u8, u8), std::io::Error> {
    write_request(
        port,
        vec![QUERY_REQUEST, CATEGORY, INPUT_SELECT_FUNCTION, 0xff, 0xff],
    )
    .map(|data| (*data.get(0).unwrap_or(&0), *data.get(1).unwrap_or(&0)))
}

fn get_state(port: &mut dyn serialport::SerialPort) -> Result<TvState, std::io::Error> {
    let power = is_powered_on(&mut *port)?;
    std::thread::sleep(Duration::from_millis(10));
    let input = if power {
        get_current_input(&mut *port)?
    } else {
        (0, 0)
    };
    return match (power, input) {
        (false, _) => Ok(TvState::TvOff),
        (true, (INPUT_TYPE_HDMI, 1)) => Ok(TvState::TvOnDennis),
        (true, _) => Ok(TvState::TvOnOther),
    };
}

fn volume_up(port: &mut dyn serialport::SerialPort) -> Result<(), std::io::Error> {
    write_command(
        port,
        vec![
            CONTROL_REQUEST,
            CATEGORY,
            VOLUME_CONTROL_FUNCTION,
            0x03,
            0x00,
            0x00,
        ],
    )
}

fn volume_down(port: &mut dyn serialport::SerialPort) -> Result<(), std::io::Error> {
    write_command(
        port,
        vec![
            CONTROL_REQUEST,
            CATEGORY,
            VOLUME_CONTROL_FUNCTION,
            0x03,
            0x00,
            0x01,
        ],
    )
}

fn power_on(port: &mut dyn serialport::SerialPort) -> Result<(), std::io::Error> {
    write_command(
        port,
        vec![CONTROL_REQUEST, CATEGORY, POWER_FUNCTION, 0x02, 0x01],
    )?;
    for _ in 1..100 {
        std::thread::sleep(Duration::from_millis(10));
        if is_powered_on(&mut *port).unwrap_or(false) == true {
            println!("Confirmed power on");
            break;
        }
    }
    for _ in 1..75 {
        std::thread::sleep(Duration::from_millis(50));
        if get_current_input(&mut *port).is_ok() {
            println!("Confirmed input available");
            break;
        }
    }
    println!("Done waiting for power on");
    Ok(())
}

fn power_off(port: &mut dyn serialport::SerialPort) -> Result<(), std::io::Error> {
    write_command(
        port,
        vec![CONTROL_REQUEST, CATEGORY, POWER_FUNCTION, 0x02, 0x00],
    )
}

fn select_input(port: &mut dyn serialport::SerialPort, input: u8) -> Result<(), std::io::Error> {
    write_command(
        port,
        vec![
            CONTROL_REQUEST,
            CATEGORY,
            INPUT_SELECT_FUNCTION,
            0x03,
            0x04,
            input,
        ],
    )
}

fn sircs_command(
    port: &mut dyn serialport::SerialPort,
    sircs_command: (u8, u8),
) -> Result<(), std::io::Error> {
    write_command(
        port,
        vec![
            CONTROL_REQUEST,
            CATEGORY,
            SIRCS_FUNCTION,
            0x03,
            sircs_command.0,
            sircs_command.1,
        ],
    )
}
