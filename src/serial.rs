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
    Settings,
    Input,
}

pub(crate) fn blocking_serial_thread(
    internal_message_tx: mpsc::Sender<InternalMessage>,
    mut serial_out_rx: mpsc::Receiver<SerialCommand>,
) {
    loop {
        let exit = serial_loop(&internal_message_tx, &mut serial_out_rx);
        if let Err(error) = exit {
            println!("Serial: Connection lost: {}", error);
        }

        std::thread::sleep(Duration::from_secs(1));
    }
}

fn serial_loop(
    internal_message_tx: &mpsc::Sender<InternalMessage>,
    serial_out_rx: &mut mpsc::Receiver<SerialCommand>,
) -> Result<(), std::io::Error> {
    println!("Serial: Connecting");

    let mut port = serialport::new("/dev/ttyUSB0", 9600)
        .timeout(Duration::from_millis(800))
        .open()
        .expect("Opening serial port");

    // Get an initial state reading to confirm we're connected.
    for _ in 1..100 {
        if let Ok(_) = get_state(&mut *port) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    println!("Serial: Ready");

    loop {
        let state = get_state(&mut *port)?;
        internal_message_tx
            .blocking_send(InternalMessage::UpdateTvState(state))
            .expect("Serial TV state send");

        while let Ok(cmd) = serial_out_rx.try_recv() {
            println!("Serial: Command {:?}", cmd);
            match cmd {
                SerialCommand::VolumeUp => send_key_code(&mut *port, KEY_CODE_VOLUME_UP)?,
                SerialCommand::VolumeDown => send_key_code(&mut *port, KEY_CODE_VOLUME_DOWN)?,
                SerialCommand::PowerOn => power_on(&mut *port)?,
                SerialCommand::PowerOff => power_off(&mut *port)?,
                SerialCommand::SelectInput(input) => select_hdmi_input(&mut *port, input)?,
                SerialCommand::CursorUp => send_key_code(&mut *port, KEY_CODE_CURSOR_UP)?,
                SerialCommand::CursorDown => send_key_code(&mut *port, KEY_CODE_CURSOR_DOWN)?,
                SerialCommand::CursorLeft => send_key_code(&mut *port, KEY_CODE_CURSOR_LEFT)?,
                SerialCommand::CursorRight => send_key_code(&mut *port, KEY_CODE_CURSOR_RIGHT)?,
                SerialCommand::Ok => send_key_code(&mut *port, KEY_CODE_OK)?,
                SerialCommand::Back => send_key_code(&mut *port, KEY_CODE_BACK)?,
                SerialCommand::Settings => send_key_code(&mut *port, KEY_CODE_SETTINGS)?,
                SerialCommand::Input => send_key_code(&mut *port, KEY_CODE_INPUT)?,
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

const KEY_CODE_VOLUME_UP: u8 = 0x02;
const KEY_CODE_VOLUME_DOWN: u8 = 0x03;
const KEY_CODE_INPUT: u8 = 0x0b;
const KEY_CODE_SETTINGS: u8 = 0x43;
const KEY_CODE_CURSOR_LEFT: u8 = 0x06;
const KEY_CODE_CURSOR_RIGHT: u8 = 0x07;
const KEY_CODE_CURSOR_UP: u8 = 0x40;
const KEY_CODE_CURSOR_DOWN: u8 = 0x41;
const KEY_CODE_OK: u8 = 0x44;
const KEY_CODE_BACK: u8 = 0x28;

const COMMAND_POWER: &str = "ka";
const COMMAND_INPUT: &str = "xb";
const COMMAND_KEY_CODE: &str = "mc";

const QUERY: u8 = 0xff;

const INPUT_HDMI: u8 = 0x90;

const TV_SET_ID: u8 = 0x01;

fn run_command(
    port: &mut dyn serialport::SerialPort,
    command: &str,
    data: u8,
) -> Result<u8, std::io::Error> {
    let cmd = format_args!("{} {:02x} {:02x}\n", command, TV_SET_ID, data);
    port.write_fmt(cmd)?;

    let mut resp_buf = [0; 24];
    let chars_read = port.read(&mut resp_buf)?;
    if chars_read != 10 {
        return Err(Error::new(
            ErrorKind::Other,
            format!(
                "Sent '{}', expected 10-byte response, got {} bytes",
                cmd.to_string().trim(),
                chars_read
            ),
        ));
    }
    let response = String::from_utf8_lossy(&resp_buf[0..chars_read]);
    if &response[5..7] != "OK" {
        return Err(Error::new(
            ErrorKind::Other,
            format!(
                "Sent '{}', expected OK response, got '{}' from '{}'",
                cmd.to_string().trim(),
                &response[6..8],
                response.trim(),
            ),
        ));
    }

    u8::from_str_radix(&response[7..9], 16).map_err(|_| {
        Error::new(
            ErrorKind::Other,
            format!(
                "Sent '{}', tried to parse number in response, got '{}'",
                cmd.to_string().trim(),
                response.trim(),
            ),
        )
    })
}

fn query(port: &mut dyn serialport::SerialPort, command: &str) -> Result<u8, std::io::Error> {
    run_command(port, command, QUERY)
}

fn send_key_code(
    port: &mut dyn serialport::SerialPort,
    key_code: u8,
) -> Result<(), std::io::Error> {
    run_command(port, COMMAND_KEY_CODE, key_code)?;
    Ok(())
}

fn is_powered_on(port: &mut dyn serialport::SerialPort) -> Result<bool, std::io::Error> {
    Ok(query(port, COMMAND_POWER)? == 1)
}

fn get_current_hdmi_input(
    port: &mut dyn serialport::SerialPort,
) -> Result<Option<u8>, std::io::Error> {
    match query(port, COMMAND_INPUT)? {
        val @ INPUT_HDMI.. => Ok(Some(1 + val - INPUT_HDMI)),
        _ => Ok(None),
    }
}

fn get_state(port: &mut dyn serialport::SerialPort) -> Result<TvState, std::io::Error> {
    let power = is_powered_on(&mut *port)?;
    std::thread::sleep(Duration::from_millis(10));
    let hdmi_input = if power {
        get_current_hdmi_input(&mut *port)?
    } else {
        None
    };
    return match (power, hdmi_input) {
        (false, _) => Ok(TvState::TvOff),
        (true, Some(1)) => Ok(TvState::TvOnDennis),
        (true, _) => Ok(TvState::TvOnOther),
    };
}

fn power_on(port: &mut dyn serialport::SerialPort) -> Result<(), std::io::Error> {
    run_command(port, COMMAND_POWER, 0x01)?;
    // for _ in 1..100 {
    //     std::thread::sleep(Duration::from_millis(10));
    //     if is_powered_on(&mut *port).unwrap_or(false) == true {
    //         println!("Serial: power_on: Confirmed power on");
    //         break;
    //     }
    // }
    // for _ in 1..75 {
    //     std::thread::sleep(Duration::from_millis(50));
    //     if get_current_input(&mut *port).is_ok() {
    //         println!("Serial power_on: Confirmed input available");
    //         break;
    //     }
    // }
    println!("Serial: power_on: Done waiting for power on");
    Ok(())
}

fn power_off(port: &mut dyn serialport::SerialPort) -> Result<(), std::io::Error> {
    run_command(port, COMMAND_POWER, 0x00)?;
    Ok(())
}

fn select_hdmi_input(
    port: &mut dyn serialport::SerialPort,
    hdmi_input: u8,
) -> Result<(), std::io::Error> {
    run_command(port, COMMAND_INPUT, INPUT_HDMI + (hdmi_input - 1))?;
    Ok(())
}
