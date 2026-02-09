mod i2c;
mod mqtt;
mod serial;

use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::{i2c::I2CCommand, mqtt::MqttCommand, serial::SerialCommand};

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
    Starting(Instant),
    TvOff,
    TvOnDennis,
    TvOnOther,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum DennisState {
    Unknown,
    Off,
    On,
}

#[derive(Debug)]
enum InternalMessage {
    UpdateTvState(TvState),
    WakeDennis,
    SleepDennis,
    AsciiKey(u8),
    ConsumerCode(u8),
    KeyCode(u8),
    OkButton,
    PowerButton,
    PowerOn,
    PowerOff,
    UsbReadinessStateChange(bool),
    InternalCheck,
}

fn get_passthru_flag_command(state: &TvState) -> I2CCommand {
    return match *state {
        TvState::TvOnDennis | TvState::Unknown | TvState::Starting(_) => I2CCommand::PassthruEnable,
        TvState::TvOff | TvState::TvOnOther => I2CCommand::PassthruDisable,
    };
}

async fn internal_check_thread(internal_message_tx: mpsc::Sender<InternalMessage>) {
    loop {
        internal_message_tx
            .send(InternalMessage::InternalCheck)
            .await
            .expect("Internal ping send");
        tokio::time::sleep(Duration::from_secs(1)).await;
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
    let (i2c_out_tx, i2c_out_rx) = mpsc::channel::<I2CCommand>(10);
    let i2c_thread_handle =
        tokio::task::spawn_blocking(move || i2c::blocking_i2c_thread(i2c_sender, i2c_out_rx));

    let serial_sender = internal_message_tx.clone();
    let (serial_out_tx, serial_out_rx) = mpsc::channel::<SerialCommand>(10);
    let serial_thread_handle = tokio::task::spawn_blocking(move || {
        serial::blocking_serial_thread(serial_sender, serial_out_rx)
    });

    let internal_check_sender = internal_message_tx.clone();
    let internal_thread_handle = tokio::task::spawn(internal_check_thread(internal_check_sender));

    let mut tv_state = TvState::Unknown;
    let mut dennis_state = DennisState::Unknown;
    let mut last_auto_sleep = Instant::now();

    let _ = i2c_out_tx.try_send(get_passthru_flag_command(&tv_state));

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

        if (tv_state == TvState::TvOff || tv_state == TvState::TvOnOther)
            && dennis_state == DennisState::On
            && Instant::now() - last_auto_sleep > Duration::from_secs(10)
        {
            println!("Dennis was left on, sending sleep command");
            let _ = i2c_out_tx.try_send(I2CCommand::Sleep);
            last_auto_sleep = Instant::now();
        }

        match msg {
            InternalMessage::UpdateTvState(new_state) => {
                if new_state != tv_state {
                    if let TvState::Starting(startup_time) = tv_state
                        && new_state == TvState::TvOff
                        && Instant::now() - startup_time < Duration::from_secs(10)
                    {
                        // Ignore TV still seeming to be off just after we sent
                        // it a power on command, it takes a few seconds
                        // sometimes
                    } else {
                        tv_state = new_state;
                        last_auto_sleep = Instant::now(); // Reset the auto sleep timer
                        let tv_is_on = matches!(
                            tv_state,
                            TvState::Starting(_) | TvState::TvOnDennis | TvState::TvOnOther
                        );
                        let _ = i2c_out_tx.try_send(get_passthru_flag_command(&tv_state));
                        let _ = mqtt_out_tx.try_send(MqttCommand::SetHyperHdr { state: tv_is_on });
                        let _ =
                            mqtt_out_tx.try_send(MqttCommand::NoticeTvChange { state: tv_is_on });
                        println!("TV State: {:?}", &tv_state);

                        if new_state == TvState::TvOnDennis {
                            let _ = i2c_out_tx.try_send(I2CCommand::UsbWake);
                        }
                    }
                }
            }
            InternalMessage::WakeDennis => {
                last_auto_sleep = Instant::now(); // Reset the auto sleep timer
                let _ = i2c_out_tx.try_send(I2CCommand::UsbWake);
            }
            InternalMessage::SleepDennis => {
                let _ = i2c_out_tx.try_send(I2CCommand::Sleep);
            }
            InternalMessage::OkButton => {
                let _ = serial_out_tx.try_send(SerialCommand::Ok);
            }
            InternalMessage::PowerButton => {
                last_auto_sleep = Instant::now(); // Reset the auto sleep timer
                match tv_state {
                    TvState::TvOff => {
                        let _ = internal_message_tx.try_send(InternalMessage::PowerOn);
                    }
                    TvState::TvOnDennis | TvState::TvOnOther => {
                        let _ = internal_message_tx.try_send(InternalMessage::PowerOff);
                    }
                    TvState::Unknown | TvState::Starting(_) => {}
                }
            }
            InternalMessage::PowerOff => {
                let _ = i2c_out_tx.try_send(I2CCommand::Sleep);
                let _ = serial_out_tx.try_send(SerialCommand::PowerOff);
                let _ = mqtt_out_tx.try_send(MqttCommand::NoticeTvChange { state: false });
            }
            InternalMessage::PowerOn => {
                let _ = i2c_out_tx.try_send(I2CCommand::UsbWake);
                let _ = mqtt_out_tx.try_send(MqttCommand::SetHyperHdr { state: true });
                let _ = serial_out_tx.try_send(SerialCommand::PowerOn);
                let _ = mqtt_out_tx.try_send(MqttCommand::NoticeTvChange { state: true });
                let _ = serial_out_tx.try_send(SerialCommand::SelectInput(1));
                tv_state = TvState::Starting(Instant::now());
            }
            InternalMessage::ConsumerCode(data) => match data {
                CONSUMER_CODE_VOLUME_DOWN => {
                    let _ = serial_out_tx.try_send(SerialCommand::VolumeDown);
                }
                CONSUMER_CODE_VOLUME_UP => {
                    let _ = serial_out_tx.try_send(SerialCommand::VolumeUp);
                }
                CONSUMER_CODE_CHANNEL => {
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
            InternalMessage::AsciiKey(_data) => {
                // Deliberately ignored
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
                dennis_state = match data {
                    false => DennisState::Off,
                    true => DennisState::On,
                };
                println!("Dennis State: {:?}", dennis_state);
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
