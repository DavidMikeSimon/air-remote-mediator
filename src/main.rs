mod i2c;
mod mqtt;
mod serial;

use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::{mqtt::MqttCommand, serial::SerialCommand};

const CONSUMER_CODE_VOLUME_UP: u8 = 0xE9;
const CONSUMER_CODE_VOLUME_DOWN: u8 = 0xEA;
const CONSUMER_CODE_MENU_ESCAPE: u8 = 0x46;
const CONSUMER_CODE_CHANNEL: u8 = 0x86;
const CONSUMER_CODE_MEDIA_SELECT_HOME: u8 = 0x9A;
const CONSUMER_CODE_PLAY_PAUSE: u8 = 0xCD;

const HID_KEY_ARROW_RIGHT: u8 = 0x50;
const HID_KEY_ARROW_LEFT: u8 = 0x4F;
const HID_KEY_ARROW_DOWN: u8 = 0x51;
const HID_KEY_ARROW_UP: u8 = 0x52;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum TvState {
    Unknown,
    TvOff,
    TvOnDennis,
    TvOnOther,
}

#[derive(Debug)]
enum InternalMessage {
    UpdateTvState(TvState),
    WakeDennis,
    AsciiKey(u8),
    ConsumerCode(u8),
    KeyCode(u8),
    OkButton,
    PowerButton,
    UsbReadinessStateChange(bool),
    InternalCheck,
}

fn get_passthru_flag_command(state: &TvState) -> u8 {
    let passthru_should_be_on = match *state {
        TvState::TvOnDennis | TvState::Unknown => true,
        _ => false,
    };
    return if passthru_should_be_on { b'P' } else { b'p' };
}

async fn internal_check_thread(internal_message_tx: mpsc::Sender<InternalMessage>) {
    loop {
        internal_message_tx
            .send(InternalMessage::InternalCheck)
            .await
            .expect("Internal ping send");
        // FIXME: Returning from main doesn't actually exit the program!
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main()]
async fn main() {
    println!("Starting up");

    let (internal_message_tx, mut internal_message_rx) = mpsc::channel::<InternalMessage>(100);

    let mqtt_sender = internal_message_tx.clone();
    let (mqtt_out_tx, mqtt_out_rx) = mpsc::channel::<MqttCommand>(10);
    let mqtt_thread_handle = tokio::task::spawn(mqtt::mqtt_thread(mqtt_sender, mqtt_out_rx));

    let i2c_sender = internal_message_tx.clone();
    let (i2c_out_tx, i2c_out_rx) = mpsc::channel::<u8>(10);
    let i2c_thread_handle =
        tokio::task::spawn_blocking(move || i2c::blocking_i2c_thread(i2c_sender, i2c_out_rx));

    let serial_sender = internal_message_tx.clone();
    let (serial_out_tx, serial_out_rx) = mpsc::channel::<SerialCommand>(10);
    let serial_thread_handle = tokio::task::spawn_blocking(move || {
        serial::blocking_serial_thread(serial_sender, serial_out_rx)
    });

    let internal_check_sender = internal_message_tx.clone();
    let internal_thread_handle = tokio::task::spawn(internal_check_thread(internal_check_sender));

    let mut state = TvState::Unknown;
    let mut anti_sneaky_window_start: Option<Instant> = None;
    let _ = i2c_out_tx.try_send(get_passthru_flag_command(&state));

    while let Some(msg) = internal_message_rx.recv().await {
        if i2c_thread_handle.is_finished() {
            eprintln!("Error: I2C task died");
            // FIXME: Returning from main doesn't actually exit the program!
            return;
        }
        if mqtt_thread_handle.is_finished() {
            eprintln!("Error: MQTT thread died");
            return;
        }
        if serial_thread_handle.is_finished() {
            eprintln!("Error: Serial thread died");
            return;
        }
        if internal_thread_handle.is_finished() {
            eprintln!("Error: Internal check thread died");
            return;
        }

        match msg {
            InternalMessage::UpdateTvState(new_state) => {
                if new_state != state {
                    state = new_state;
                    let _ = i2c_out_tx.try_send(get_passthru_flag_command(&state));
                    println!("State: {:?}", &state);

                    // TODO: Do we even need the anti-sneaky feature anymore?
                    if new_state == TvState::TvOnOther
                        && let Some(turned_on) = anti_sneaky_window_start
                        && Instant::now() - turned_on < Duration::from_secs(20)
                    {
                        println!("TV tried to sneakily switch to another input!");
                        let _ = serial_out_tx.try_send(SerialCommand::SelectInput(1));
                        let _ = i2c_out_tx.try_send(b'R');
                    } else if new_state == TvState::TvOnDennis {
                        let _ = i2c_out_tx.try_send(b'R');
                    }
                }
            }
            InternalMessage::WakeDennis => {
                let _ = i2c_out_tx.try_send(b'R');
            }
            InternalMessage::OkButton => {
                let _ = serial_out_tx.try_send(SerialCommand::Ok);
            }
            InternalMessage::PowerButton => match state {
                TvState::TvOff => {
                    anti_sneaky_window_start = Some(Instant::now());
                    let _ = i2c_out_tx.try_send(b'R');
                    let _ = serial_out_tx.try_send(SerialCommand::PowerOn);
                    let _ = serial_out_tx.try_send(SerialCommand::SelectInput(1));
                }
                TvState::TvOnDennis | TvState::TvOnOther => {
                    let _ = serial_out_tx.try_send(SerialCommand::PowerOff);
                }
                TvState::Unknown => {}
            },
            InternalMessage::ConsumerCode(data) => match data {
                CONSUMER_CODE_VOLUME_DOWN => {
                    let _ = serial_out_tx.try_send(SerialCommand::VolumeDown);
                }
                CONSUMER_CODE_VOLUME_UP => {
                    let _ = serial_out_tx.try_send(SerialCommand::VolumeUp);
                }
                CONSUMER_CODE_CHANNEL => {
                    anti_sneaky_window_start = None; // User is deliberately selecting another input
                    let _ = serial_out_tx.try_send(SerialCommand::Input);
                }
                CONSUMER_CODE_MEDIA_SELECT_HOME => {
                    let _ = serial_out_tx.try_send(SerialCommand::Settings);
                }
                CONSUMER_CODE_MENU_ESCAPE => {
                    let _ = serial_out_tx.try_send(SerialCommand::Back);
                }
                CONSUMER_CODE_PLAY_PAUSE => {
                    // Deliberately ignored
                }
                _ => {
                    println!("Unhandled consumer code: {:#04X}", data);
                }
            },
            InternalMessage::AsciiKey(data) => {
                println!("Unhandled ascii key: {:#04X}", data);
            }
            InternalMessage::KeyCode(data) => match data {
                HID_KEY_ARROW_UP => {
                    let _ = serial_out_tx.try_send(SerialCommand::CursorUp);
                }
                HID_KEY_ARROW_DOWN => {
                    let _ = serial_out_tx.try_send(SerialCommand::CursorDown);
                }
                HID_KEY_ARROW_LEFT => {
                    let _ = serial_out_tx.try_send(SerialCommand::CursorLeft);
                }
                HID_KEY_ARROW_RIGHT => {
                    let _ = serial_out_tx.try_send(SerialCommand::CursorRight);
                }
                _ => println!("Unhandled key code: {:#04X}", data),
            },
            InternalMessage::UsbReadinessStateChange(data) => {
                let _ = mqtt_out_tx.try_send(MqttCommand::NoticeUsbChange { state: data });
            }
            InternalMessage::InternalCheck => {
                // No specific action needed, this just triggers the thread
                // handle checks above to run again.
            }
        }
    }

    // FIXME: Returning from main doesn't actually exit the program!
    eprintln!("Error: Internal message queue returned None");
}
