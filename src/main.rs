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

const AIR_REMOTE_TOPIC: &str = "/air-remote/events";
const TV_STATE_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia_dlna/state";
const TV_INPUT_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia/media_title";

const AIR_REMOTE_PASSTHRU_TOPIC: &str = "/air-remote/passthru-setting";
const DENNIS_SWITCH_TOPIC: &str = "homeassistant_cmd/switch/dennis";
const TV_REMOTE_SWITCH_TOPIC: &str = "homeassistant_cmd/remote/sony_bravia";
const TV_REMOTE_COMMAND_TOPIC: &str = "homeassistant_cmd/remote_command/sony_bravia";

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

#[derive(PartialEq, Eq)]
enum TvState {
    Off,
    Dennis,
    Other,
}

struct State {
    tv: TvState,
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

fn send_passthru_flag_update(client: &mut Client, value: bool) {
    client
        .publish(
            AIR_REMOTE_PASSTHRU_TOPIC,
            QoS::AtLeastOnce,
            false,
            if value { "ON" } else { "OFF" },
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

fn handle_air_remote_event(event: &InputEvent, state: &State, client: &mut Client) {
    match event {
        InputEvent::PowerButton => {
            send_switch_update(client, TV_REMOTE_SWITCH_TOPIC, state.tv == TvState::Off);
            send_switch_update(client, DENNIS_SWITCH_TOPIC, state.tv == TvState::Off);
        }
        InputEvent::ConsumerCode { data } => match *data {
            CONSUMER_CODE_VOLUME_DOWN => send_sony_command(client, SonyCommand::VolumeDown),
            CONSUMER_CODE_VOLUME_UP => send_sony_command(client, SonyCommand::VolumeUp),
            CONSUMER_CODE_CHANNEL => send_sony_command(client, SonyCommand::Input),
            CONSUMER_CODE_MEDIA_SELECT_HOME => send_sony_command(client, SonyCommand::Home),
            CONSUMER_CODE_MENU_ESCAPE => send_sony_command(client, SonyCommand::Exit),
            CONSUMER_CODE_PLAY_PAUSE => send_sony_command(client, SonyCommand::Pause),
            _ => {
                println!("Unhandled consumer code: {:#04X}", data);
            }
        },
        InputEvent::KeyCode { data } => match *data {
            HID_KEY_ARROW_UP => send_sony_command(client, SonyCommand::Up),
            HID_KEY_ARROW_DOWN => send_sony_command(client, SonyCommand::Down),
            HID_KEY_ARROW_LEFT => send_sony_command(client, SonyCommand::Left),
            HID_KEY_ARROW_RIGHT => send_sony_command(client, SonyCommand::Right),
            _ => {
                println!("Unhandled key code: {:#04X}", data);
            }
        },
        InputEvent::OkButton => send_sony_command(client, SonyCommand::Confirm),
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

    let mut state = State { tv: TvState::Off };

    for notification in connection.iter().enumerate() {
        if let (_, Ok(Incoming(Publish(message)))) = notification {
            let payload: String = String::from_utf8(message.payload.into()).unwrap();
            match message.topic.as_str() {
                AIR_REMOTE_TOPIC => {
                    let event: InputEvent = serde_json::from_str(&payload).unwrap();
                    handle_air_remote_event(&event, &state, &mut client);
                }
                TV_STATE_TOPIC => {
                    if payload == "unavailable" {
                        state.tv = TvState::Off;
                        send_passthru_flag_update(&mut client, false);
                    }
                }
                TV_INPUT_TOPIC => {
                    if payload == "\"HDMI 1\"" {
                        state.tv = TvState::Dennis;
                        send_passthru_flag_update(&mut client, true);
                    } else {
                        state.tv = TvState::Other;
                        send_passthru_flag_update(&mut client, false);
                    }
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
