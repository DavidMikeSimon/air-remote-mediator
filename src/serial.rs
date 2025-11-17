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
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    Ok,
    Back,
    NextInput,
}

pub(crate) fn blocking_serial_thread(
    internal_message_tx: mpsc::Sender<InternalMessage>,
    mut serial_out_rx: mpsc::Receiver<SerialCommand>,
) {
    loop {
        let exit = serial_loop(&internal_message_tx, &mut serial_out_rx);
        if let Err(error) = exit {
            println!("Serial connection lost: {}", error);
        }

        std::thread::sleep(Duration::from_secs(1));
    }
}

fn serial_loop(
    internal_message_tx: &mpsc::Sender<InternalMessage>,
    serial_out_rx: &mut mpsc::Receiver<SerialCommand>,
) -> Result<(), std::io::Error> {
    println!("Connecting to Sony serial");

    let mut port = serialport::new("/dev/ttyUSB0", 9600)
        .timeout(Duration::from_millis(800))
        .open()
        .expect("Opening serial port");

    // Get an initial state reading to confirm we're connected, but throw it
    // away, since it's often inaccurate.
    loop {
        if let (Ok(_)) = get_state(&mut *port) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    loop {
        if let Ok(state) = get_state(&mut *port) {
            internal_message_tx
                .blocking_send(InternalMessage::UpdateTvState(state))
                .expect("Serial TV state send");
        }

        while let Ok(cmd) = serial_out_rx.try_recv() {
            println!("Sending Serial command: {:?}", cmd);
            match cmd {
                SerialCommand::VolumeUp => volume_up(&mut *port)?,
                SerialCommand::VolumeDown => volume_down(&mut *port)?,
                SerialCommand::PowerOn => power_on(&mut *port)?,
                SerialCommand::PowerOff => power_off(&mut *port)?,
                SerialCommand::SelectInput(input) => select_input(&mut *port, input)?,
                SerialCommand::CursorUp => sircs_command(&mut *port, SIRCS_CURSOR_UP)?,
                SerialCommand::CursorDown => sircs_command(&mut *port, SIRCS_CURSOR_DOWN)?,
                SerialCommand::CursorLeft => sircs_command(&mut *port, SIRCS_CURSOR_LEFT)?,
                SerialCommand::CursorRight => sircs_command(&mut *port, SIRCS_CURSOR_RIGHT)?,
                SerialCommand::Ok => sircs_command(&mut *port, SIRCS_SELECT)?,
                SerialCommand::Back => sircs_command(&mut *port, SIRCS_RETURN)?,
                SerialCommand::NextInput => next_input(&mut *port)?,
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
const INPUT_SELECT_FUNCTION: u8 = 0x02;
const VOLUME_CONTROL_FUNCTION: u8 = 0x05;
const BRIGHTNESS_CONTROL_FUNCTION: u8 = 0x24;
const SIRCS_FUNCTION: u8 = 0x67;

const SIRCS_CURSOR_UP: (u8, u8) = (0x01, 0x74);
const SIRCS_CURSOR_DOWN: (u8, u8) = (0x01, 0x75);
const SIRCS_CURSOR_LEFT: (u8, u8) = (0x01, 0x34);
const SIRCS_CURSOR_RIGHT: (u8, u8) = (0x01, 0x33);
const SIRCS_SELECT: (u8, u8) = (0x01, 0x65);
const SIRCS_RETURN: (u8, u8) = (0x97, 0x23);

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
            format!(
                "unexpected response header {} after request {:02X?}",
                resp_buf[0], contents
            ),
        ));
    }
    if resp_buf[1] != RESPONSE_CODE_OK {
        return Err(Error::new(
            ErrorKind::Other,
            format!(
                "unexpected response answer {} after request {:02X?}",
                resp_buf[1], contents
            ),
        ));
    }
    if vec[0] == QUERY_REQUEST {
        let mut resp_data_buf = vec![0; resp_buf[2] as usize];
        port.read_exact(resp_data_buf.as_mut_slice())?;
        let resp_checksum = resp_data_buf.pop().ok_or(Error::new(
            ErrorKind::Other,
            format!("empty response checksum after query {:02X?}", contents),
        ))?;
        resp_buf.extend(resp_data_buf.clone());
        if resp_checksum != checksum(&resp_buf) {
            return Err(Error::new(
                ErrorKind::Other,
                format!("invalid response checksum after query {:02X?}", contents),
            ));
        }
        Ok(resp_data_buf)
    } else {
        let resp_checksum = resp_buf.pop().ok_or(Error::new(
            ErrorKind::Other,
            format!("empty response checksum after command {:02X?}", contents),
        ))?;
        if resp_checksum != checksum(&resp_buf) {
            return Err(Error::new(
                ErrorKind::Other,
                format!("invalid response checksum after command {:02X?}", contents),
            ));
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
            println!("Serial power_on: Confirmed power on");
            break;
        }
    }
    for _ in 1..75 {
        std::thread::sleep(Duration::from_millis(50));
        if get_current_input(&mut *port).is_ok() {
            println!("Serial power_on: Confirmed input available");
            break;
        }
    }
    println!("Serial power_on: Done waiting for power on");
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

fn next_input(port: &mut dyn serialport::SerialPort) -> Result<(), std::io::Error> {
    let current_input = get_current_input(port).unwrap_or((0, 0));
    let new_input = match current_input {
        (4, 1) => 2,
        (4, 2) => 4,
        _ => 1,
    };
    select_input(port, new_input)
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
