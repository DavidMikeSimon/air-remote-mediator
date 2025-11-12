/*
 * ~3 seconds: TV off: homeassistant_statestream/media_player/sony_bravia_dlna/state: unavailable
 * ~3 seconds: TV on: homeassistant_statestream/media_player/sony_bravia_dlna/state: unknown OR idle OR presumably others too
 * ~3-8 seconds: Input pick: homeassistant_statestream/media_player/sony_bravia/media_title: "HDMI 1" or "HDMI 2" or "HDMI 3/ARC" or "HDMI 4"
 * ~3-8 seconds: homeassistant_statestream/media_player/sony_bravia/media_title: "Smart TV"
 */

mod i2c;
mod mqtt;
mod serial;
mod sony_commands;

use sony_commands::SonyCommand;
use std::time::Duration;
use tokio::sync::mpsc;

const HA_SCRIPT_TOGGLE_TV_AND_DENNIS: &str = "toggle_tv_and_dennis";
const HA_SCRIPT_NOTICE_DENNIS_USB_OFF: &str = "notice_dennis_usb_readiness_off";
const HA_SCRIPT_NOTICE_DENNIS_USB_ON: &str = "notice_dennis_usb_readiness_on";

const CONSUMER_CODE_VOLUME_UP: u8 = 0xE9;
const CONSUMER_CODE_VOLUME_DOWN: u8 = 0xEA;
const CONSUMER_CODE_MENU_ESCAPE: u8 = 0x46;
const CONSUMER_CODE_CHANNEL: u8 = 0x86;
const CONSUMER_CODE_MEDIA_SELECT_HOME: u8 = 0x9A;
const CONSUMER_CODE_PLAY_PAUSE: u8 = 0xCD;

const HID_KEY_ARROW_RIGHT: u8 = 0x4F;
const HID_KEY_ARROW_LEFT: u8 = 0x50;
const HID_KEY_ARROW_DOWN: u8 = 0x51;
const HID_KEY_ARROW_UP: u8 = 0x52;

#[derive(Debug)]
struct State {
    tv_is_on: bool,
    dennis_is_current_input: bool,
}

enum InternalMessage {
    UpdateTvState(bool),
    UpdateDennisIsInputState(bool),
    WakeDennis,
    AsciiKey(u8),
    ConsumerCode(u8),
    KeyCode(u8),
    OkButton,
    PowerButton,
    UsbReadinessStateChange(bool),
    InternalCheck,
}

fn get_passthru_flag_command(state: &State) -> u8 {
    let passthru_should_be_on = state.tv_is_on && state.dennis_is_current_input;
    return if passthru_should_be_on { b'P' } else { b'p' };
}

async fn internal_check_thread(internal_message_tx: mpsc::Sender<InternalMessage>) {
    loop {
        internal_message_tx
            .send(InternalMessage::InternalCheck)
            .await
            .expect("Internal ping send");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main()]
async fn main() {
    let mut mqtt_options =
        rumqttc::MqttOptions::new("air-remote-mediator-pi", "mqtt.sinclair.pipsimon.com", 1883);
    mqtt_options.set_credentials(
        "lcars",
        std::env::var("MQTT_PASS").expect("Need env var MQTT_PASS"),
    );
    mqtt_options.set_keep_alive(Duration::from_secs(5));

    let (mqtt_client, mqtt_connection) = rumqttc::AsyncClient::new(mqtt_options, 10);

    println!("Starting up");

    let (internal_message_tx, mut internal_message_rx) = mpsc::channel::<InternalMessage>(100);

    let mqtt_sender = internal_message_tx.clone();
    let mqtt_thread_handle = tokio::task::spawn(mqtt::mqtt_thread(
        mqtt_connection,
        mqtt_client.clone(),
        mqtt_sender,
    ));

    let i2c_sender = internal_message_tx.clone();
    let (i2c_out_tx, i2c_out_rx) = mpsc::channel::<u8>(10);
    let i2c_thread_handle =
        tokio::task::spawn_blocking(move || i2c::i2c_thread(i2c_sender, i2c_out_rx));

    let serial_sender = internal_message_tx.clone();
    let (serial_out_tx, serial_out_rx) = mpsc::channel::<u8>(10);
    let serial_thread_handle =
        tokio::task::spawn_blocking(move || serial::serial_thread(serial_sender, serial_out_rx));

    let internal_check_sender = internal_message_tx.clone();
    let internal_thread_handle = tokio::task::spawn(internal_check_thread(internal_check_sender));

    let mut state = State {
        tv_is_on: false,
        dennis_is_current_input: false,
    };

    while let Some(msg) = internal_message_rx.recv().await {
        if i2c_thread_handle.is_finished() {
            eprintln!("Error: I2C task died");
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
            InternalMessage::UpdateTvState(tv_is_on) => {
                state.tv_is_on = tv_is_on;
                i2c_out_tx
                    .send(get_passthru_flag_command(&state))
                    .await
                    .expect("Send passthru flag command after TV state change");
                println!("State: {:?}", &state);
            }
            InternalMessage::UpdateDennisIsInputState(dennis_is_current_input) => {
                state.dennis_is_current_input = dennis_is_current_input;
                i2c_out_tx
                    .send(get_passthru_flag_command(&state))
                    .await
                    .expect("Send passthru flag command after input state change");
                println!("State: {:?}", &state);
            }
            InternalMessage::WakeDennis => {
                i2c_out_tx
                    .send(b'R')
                    .await
                    .expect("Send wake Dennis command");
                println!("Waking Dennis");
            }
            InternalMessage::OkButton => {
                mqtt::send_sony_command(&mqtt_client, SonyCommand::Confirm).await;
            }
            InternalMessage::PowerButton => {
                mqtt::send_ha_script_command(&mqtt_client, HA_SCRIPT_TOGGLE_TV_AND_DENNIS).await;
            }
            InternalMessage::ConsumerCode(data) => match data {
                CONSUMER_CODE_VOLUME_DOWN => {
                    mqtt::send_media_player_command(&mqtt_client, "volume_down").await
                }
                CONSUMER_CODE_VOLUME_UP => {
                    mqtt::send_media_player_command(&mqtt_client, "volume_up").await
                }
                CONSUMER_CODE_CHANNEL => {
                    mqtt::send_sony_command(&mqtt_client, SonyCommand::Input).await
                }
                CONSUMER_CODE_MEDIA_SELECT_HOME => {
                    mqtt::open_sony_app(&mqtt_client, "HALauncher").await
                }
                CONSUMER_CODE_MENU_ESCAPE => {
                    mqtt::send_sony_command(&mqtt_client, SonyCommand::Return).await
                }
                CONSUMER_CODE_PLAY_PAUSE => {
                    if !state.dennis_is_current_input {
                        mqtt::send_sony_command(&mqtt_client, SonyCommand::Pause).await
                    }
                }
                _ => {
                    println!("Unhandled consumer code: {:#04X}", data);
                }
            },
            InternalMessage::AsciiKey(data) => {
                println!("Unhandled ascii key: {:#04X}", data);
            }
            InternalMessage::KeyCode(data) => match data {
                HID_KEY_ARROW_UP => mqtt::send_sony_command(&mqtt_client, SonyCommand::Up).await,
                HID_KEY_ARROW_DOWN => {
                    mqtt::send_sony_command(&mqtt_client, SonyCommand::Down).await
                }
                HID_KEY_ARROW_LEFT => {
                    mqtt::send_sony_command(&mqtt_client, SonyCommand::Left).await
                }
                HID_KEY_ARROW_RIGHT => {
                    mqtt::send_sony_command(&mqtt_client, SonyCommand::Right).await
                }
                _ => println!("Unhandled key code: {:#04X}", data),
            },
            InternalMessage::UsbReadinessStateChange(data) => match data {
                false => {
                    mqtt::send_ha_script_command(&mqtt_client, HA_SCRIPT_NOTICE_DENNIS_USB_OFF)
                        .await
                }
                true => {
                    mqtt::send_ha_script_command(&mqtt_client, HA_SCRIPT_NOTICE_DENNIS_USB_ON).await
                }
            },
            InternalMessage::InternalCheck => {
                // No specific action needed, this just triggers the thread
                // handle checks above to run again.
            }
        }
    }

    eprintln!("Error: Internal message queue returned None");
}
