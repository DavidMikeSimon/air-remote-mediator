mod i2c;
mod mqtt;
mod serial;
mod transactional_receiver;

use hotpath;
use jiff::Zoned;
use std::process;
use std::thread;
use std::time::{Duration, Instant};
use tokio::time::timeout;

use crate::serial::EnergySavingMode;
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

impl TvState {
    fn tv_is_on(&self) -> bool {
        matches!(
            self,
            TvState::Starting(_) | TvState::TvOnDennis | TvState::TvOnOther
        )
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum DennisState {
    Unknown,
    Off,
    On,
}

#[derive(Debug)]
enum InternalMessage {
    UpdateSunElevation(f32),
    UpdateTvState(TvState),
    WakeDennis,
    SleepDennis,
    SetDennisAutoSleepMode(bool),
    AsciiKey(u8),
    ConsumerCode(u8),
    KeyCode(u8),
    OkButton,
    PowerButton,
    PowerOn,
    PowerOff,
    UsbReadinessStateChange(bool),
}

fn get_passthru_flag_command(state: &TvState) -> I2CCommand {
    match *state {
        TvState::TvOnDennis | TvState::Unknown | TvState::Starting(_) => I2CCommand::PassthruEnable,
        TvState::TvOff | TvState::TvOnOther => I2CCommand::PassthruDisable,
    }
}

fn get_energy_saving_mode(sun_elevation: f32) -> EnergySavingMode {
    match (sun_elevation, Zoned::now().hour()) {
        // Night, sun is well below horizon
        (..-6.0, _) => EnergySavingMode::Maximum,
        // Early morning or late evening
        (-6.0..6.0, _) => EnergySavingMode::Medium,
        // Mid-morning tends to be extra bright in the living room
        (6.0..55.0, ..12) => EnergySavingMode::Off,
        // Daytime
        _ => EnergySavingMode::Minimum,
    }
}

#[tokio::main()]
#[hotpath::main()]
async fn main() {
    hotpath::tokio_runtime!();
    println!("Starting up");

    let (internal_message_tx, mut internal_message_rx) = hotpath::channel!(
        tokio::sync::mpsc::channel::<InternalMessage>(100),
        label = "internal_message",
        log = true
    );

    let mqtt_sender = internal_message_tx.clone();
    let (mqtt_out_tx, mqtt_out_rx) = hotpath::channel!(
        tokio::sync::mpsc::channel::<MqttCommand>(10),
        label = "mqtt_out",
        log = true,
    );
    let mqtt_thread_handle = tokio::task::spawn(mqtt::mqtt_thread(mqtt_sender, mqtt_out_rx));

    let i2c_sender = internal_message_tx.clone();
    let (i2c_out_tx, i2c_out_rx) = hotpath::channel!(
        tokio::sync::mpsc::channel::<I2CCommand>(10),
        label = "i2c_out",
        log = true,
    );
    let i2c_thread_handle = thread::Builder::new()
        .name("i2c_thread".to_owned())
        .spawn(move || i2c::blocking_i2c_thread(i2c_sender, i2c_out_rx))
        .expect("i2c thread spawn");

    let serial_sender = internal_message_tx.clone();
    let (serial_out_tx, serial_out_rx) = hotpath::channel!(
        tokio::sync::mpsc::channel::<SerialCommand>(100),
        label = "serial_out",
        log = true,
    );
    let serial_thread_handle = thread::Builder::new()
        .name("serial_thread".to_owned())
        .spawn(move || serial::blocking_serial_thread(serial_sender, serial_out_rx))
        .expect("serial thread spawn");

    let mut current_sun_elevation: Option<f32> = None;
    let mut current_energy_saving_mode: Option<EnergySavingMode> = None;
    let mut tv_state = TvState::Unknown;
    let mut dennis_state = DennisState::Unknown;
    let mut dennis_auto_sleep_state = true;
    let mut last_auto_sleep = Instant::now();

    let _ = i2c_out_tx.try_send(get_passthru_flag_command(&tv_state));
    let _ = mqtt_out_tx.try_send(MqttCommand::NoticeAutoSleepChange {
        state: dennis_auto_sleep_state,
    });

    loop {
        let timeout_or_msg = timeout(Duration::from_secs(1), internal_message_rx.recv()).await;

        if i2c_thread_handle.is_finished() {
            eprintln!("Error: I2C task died");
            process::exit(1);
        }
        if mqtt_thread_handle.is_finished() {
            eprintln!("Error: MQTT thread died");
            process::exit(1);
        }
        if serial_thread_handle.is_finished() {
            eprintln!("Error: Serial thread died");
            process::exit(1);
        }

        if (tv_state == TvState::TvOff || tv_state == TvState::TvOnOther)
            && dennis_state == DennisState::On
            && dennis_auto_sleep_state
            && Instant::now() - last_auto_sleep > Duration::from_secs(10)
        {
            println!("Dennis was left on, sending sleep command");
            let _ = i2c_out_tx.try_send(I2CCommand::Sleep);
            last_auto_sleep = Instant::now();
        }

        let msg = match timeout_or_msg {
            Err(_) => {
                continue;
            } // Timed out, no worries, just keep waiting
            Ok(None) => {
                eprintln!("Error: Internal message queue returned None");
                process::exit(1);
            }
            Ok(Some(msg)) => msg,
        };

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
                        println!("TV State: {:?}", &tv_state);
                        last_auto_sleep = Instant::now(); // Reset the auto sleep timer
                        let tv_is_on = tv_state.tv_is_on();
                        let _ = i2c_out_tx.try_send(get_passthru_flag_command(&tv_state));
                        let _ = mqtt_out_tx.try_send(MqttCommand::SetHyperHdr { state: tv_is_on });
                        let _ =
                            mqtt_out_tx.try_send(MqttCommand::NoticeTvChange { state: tv_is_on });
                        if new_state == TvState::TvOnDennis {
                            let _ = i2c_out_tx.try_send(I2CCommand::UsbWake);
                        }

                        if let Some(sun_elevation) = current_sun_elevation
                            && tv_is_on
                        {
                            let new_energy_saving_mode = get_energy_saving_mode(sun_elevation);
                            current_energy_saving_mode = Some(new_energy_saving_mode);
                            let _ = serial_out_tx.try_send(SerialCommand::SetEnergySavingMode(
                                new_energy_saving_mode,
                            ));
                        }
                    }
                }
            }
            InternalMessage::UpdateSunElevation(new_elevation) => {
                current_sun_elevation = Some(new_elevation);

                let new_energy_saving_mode = get_energy_saving_mode(new_elevation);
                if current_energy_saving_mode.is_none_or(|mode| mode != new_energy_saving_mode) {
                    if tv_state.tv_is_on() {
                        let _ = serial_out_tx
                            .try_send(SerialCommand::SetEnergySavingMode(new_energy_saving_mode));
                    }
                    current_energy_saving_mode = Some(new_energy_saving_mode);
                }
            }
            InternalMessage::WakeDennis => {
                last_auto_sleep = Instant::now(); // Reset the auto sleep timer
                let _ = i2c_out_tx.try_send(I2CCommand::UsbWake);
            }
            InternalMessage::SleepDennis => {
                let _ = i2c_out_tx.try_send(I2CCommand::Sleep);
            }
            InternalMessage::SetDennisAutoSleepMode(state) => {
                dennis_auto_sleep_state = state;
                let _ = mqtt_out_tx.try_send(MqttCommand::NoticeAutoSleepChange { state });
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
        }
    }
}
