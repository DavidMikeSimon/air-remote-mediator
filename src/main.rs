/*
 * ~3 seconds: TV off: homeassistant_statestream/media_player/sony_bravia_dlna/state: unavailable
 * ~3 seconds: TV on: homeassistant_statestream/media_player/sony_bravia_dlna/state: unknown OR idle OR presumably others too
 * ~3-8 seconds: Input pick: homeassistant_statestream/media_player/sony_bravia/media_title: "HDMI 1" or "HDMI 2" or "HDMI 3/ARC" or "HDMI 4"
 * ~3-8 seconds: homeassistant_statestream/media_player/sony_bravia/media_title: "Smart TV"
 */

mod sony_commands;

use std::{env, time::Duration};

use rumqttc::{Client, Event::Incoming, MqttOptions, Packet::Publish, QoS};
use serde::Deserialize;
use serde_hex::{SerHex, StrictCapPfx};
use serde_variant::to_variant_name;
use sony_commands::SonyCommand;

const AIR_REMOTE_TOPIC: &str = "air-remote/events";
const TV_STATE_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia/state";
const TV_INPUT_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia/media_title";

const AIR_REMOTE_PASSTHRU_TOPIC: &str = "air-remote/passthru-setting";
const DENNIS_SWITCH_TOPIC: &str = "homeassistant_cmd/switch/dennis";
const TV_REMOTE_SWITCH_TOPIC: &str = "homeassistant_cmd/remote/sony_bravia";
const TV_REMOTE_COMMAND_TOPIC: &str = "homeassistant_cmd/remote_command/sony_bravia";
const TV_MEDIA_PLAYER_APP_TOPIC: &str = "homeassistant_cmd/media_player_app/sony_bravia";

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

#[derive(Deserialize, Debug)]
#[serde(tag = "event")]
enum InputEvent {
    #[serde(rename = "A")]
    AsciiKey {
        #[serde(with = "SerHex::<StrictCapPfx>")]
        data: u8,
    },
    #[serde(rename = "C")]
    ConsumerCode {
        #[serde(with = "SerHex::<StrictCapPfx>")]
        data: u8,
    },
    #[serde(rename = "K")]
    KeyCode {
        #[serde(with = "SerHex::<StrictCapPfx>")]
        data: u8,
    },
    #[serde(rename = "N")]
    NetworkConnected,
    #[serde(rename = "O")]
    OkButton,
    #[serde(rename = "W")]
    PowerButton,
}

#[derive(Debug)]
struct State {
    tv_is_on: bool,
    dennis_is_current_input: bool,
}

fn send_switch_update(client: &mut Client, topic: &str, value: bool) {
    client
        .publish(
            topic,
            QoS::AtLeastOnce,
            false,
            if value { "on" } else { "off" },
        )
        .unwrap();
}

fn send_passthru_flag_update(client: &mut Client, state: &State) {
    client
        .publish(
            AIR_REMOTE_PASSTHRU_TOPIC,
            QoS::AtLeastOnce,
            false,
            if state.tv_is_on && state.dennis_is_current_input {
                "ON"
            } else {
                "OFF"
            },
        )
        .unwrap();
}

fn send_sony_command(client: &mut Client, command: SonyCommand) {
    client
        .publish(
            TV_REMOTE_COMMAND_TOPIC,
            QoS::AtLeastOnce,
            false,
            to_variant_name(&command).unwrap(),
        )
        .unwrap();
}

fn open_sony_app(client: &mut Client, app_name: &str) {
    client
        .publish(
            TV_MEDIA_PLAYER_APP_TOPIC,
            QoS::AtLeastOnce,
            false,
            app_name,
        )
        .unwrap();
}

fn handle_air_remote_event(event: &InputEvent, state: &State, client: &mut Client) {
    println!("Input: {:?}", &event);
    match event {
        InputEvent::PowerButton => {
            send_switch_update(client, TV_REMOTE_SWITCH_TOPIC, !state.tv_is_on);
            send_switch_update(client, DENNIS_SWITCH_TOPIC, !state.tv_is_on);
        }
        InputEvent::ConsumerCode { data } => match *data {
            CONSUMER_CODE_VOLUME_DOWN => send_sony_command(client, SonyCommand::VolumeDown),
            CONSUMER_CODE_VOLUME_UP => send_sony_command(client, SonyCommand::VolumeUp),
            CONSUMER_CODE_CHANNEL => send_sony_command(client, SonyCommand::Input),
            CONSUMER_CODE_MEDIA_SELECT_HOME => open_sony_app(client, "HALauncher"),
            CONSUMER_CODE_MENU_ESCAPE => send_sony_command(client, SonyCommand::Return),
            CONSUMER_CODE_PLAY_PAUSE => {
                if !state.dennis_is_current_input {
                    send_sony_command(client, SonyCommand::Pause)
                }
            }
            _ => {
                println!("Unhandled consumer code: {:#04X}", data);
            }
        },
        InputEvent::KeyCode { data } => match *data {
            HID_KEY_ARROW_UP => {
                if !state.dennis_is_current_input {
                    send_sony_command(client, SonyCommand::Up)
                }
            }
            HID_KEY_ARROW_DOWN => {
                if !state.dennis_is_current_input {
                    send_sony_command(client, SonyCommand::Down)
                }
            }
            HID_KEY_ARROW_LEFT => {
                if !state.dennis_is_current_input {
                    send_sony_command(client, SonyCommand::Left)
                }
            }
            HID_KEY_ARROW_RIGHT => {
                if !state.dennis_is_current_input {
                    send_sony_command(client, SonyCommand::Right)
                }
            }
            _ => {
                println!("Unhandled key code: {:#04X}", data);
            }
        },
        InputEvent::OkButton => {
            if state.dennis_is_current_input {
                if state.tv_is_on {
                    send_switch_update(client, DENNIS_SWITCH_TOPIC, true);
                }
            } else {
                send_sony_command(client, SonyCommand::Confirm);
            }
        },
        _ => {
            println!("Event: {:?}", event);
        }
    }
}

fn main() {
    let mut mqtt_options =
        MqttOptions::new("air-remote-mediator", "mqtt.sinclair.pipsimon.com", 1883);
    mqtt_options.set_credentials(
        "lcars",
        env::var("MQTT_PASS").expect("Need env var MQTT_PASS"),
    );
    mqtt_options.set_keep_alive(Duration::from_secs(5));

    let (mut client, mut connection) = Client::new(mqtt_options, 10);

    client.subscribe(AIR_REMOTE_TOPIC, QoS::AtMostOnce).unwrap();
    client.subscribe(TV_STATE_TOPIC, QoS::AtLeastOnce).unwrap();
    client.subscribe(TV_INPUT_TOPIC, QoS::AtLeastOnce).unwrap();

    let mut state = State {
        tv_is_on: false,
        dennis_is_current_input: false,
    };

    println!("Starting up");

    for notification in connection.iter().enumerate() {
        if let (_, Ok(Incoming(Publish(message)))) = notification {
            let payload: String = String::from_utf8(message.payload.into()).unwrap();
            match message.topic.as_str() {
                AIR_REMOTE_TOPIC => {
                    let event: InputEvent = serde_json::from_str(&payload).unwrap();
                    handle_air_remote_event(&event, &state, &mut client);
                }
                TV_STATE_TOPIC => {
                    if payload == "off" {
                        state.tv_is_on = false;
                    } else {
                        state.tv_is_on = true;
                    }
                    send_passthru_flag_update(&mut client, &state);
                    println!("State: {:?}", &state);
                }
                TV_INPUT_TOPIC => {
                    if payload == "\"HDMI 1\"" {
                        state.dennis_is_current_input = true;
                    } else {
                        state.dennis_is_current_input = false;
                    }
                    send_passthru_flag_update(&mut client, &state);
                    println!("State: {:?}", &state);
                }
                _ => {
                    println!(
                        "Message from unknown topic {:?}: {:?}",
                        message.topic, payload
                    );
                }
            }
        }
    }
}
