/*
 * ~3 seconds: TV off: homeassistant_statestream/media_player/sony_bravia_dlna/state: unavailable
 * ~3 seconds: TV on: homeassistant_statestream/media_player/sony_bravia_dlna/state: unknown OR idle OR presumably others too
 * ~3-8 seconds: Input pick: homeassistant_statestream/media_player/sony_bravia/media_title: "HDMI 1" or "HDMI 2" or "HDMI 3/ARC" or "HDMI 4"
 * ~3-8 seconds: homeassistant_statestream/media_player/sony_bravia/media_title: "Smart TV"
 */

use std::{env, time::Duration};

use rumqttc::{Client, Event::Incoming, MqttOptions, Packet::Publish, QoS};
use serde::Deserialize;
use serde_hex::{SerHex, StrictCapPfx};

const AIR_REMOTE_TOPIC: &str = "/air-remote/events";
const TV_STATE_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia_dlna/state";
const TV_INPUT_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia/media_title";

const TV_REMOTE_COMMAND_TOPIC: &str = "homeassistant_cmd/remote/sony_bravia";

const OFF: &str = "off";
const ON: &str = "on";

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

    let mut tv_on: bool = false;

    for notification in connection.iter().enumerate() {
        if let (_, Ok(Incoming(Publish(message)))) = notification {
            let payload: String = String::from_utf8(message.payload.into()).unwrap();
            match (message.topic.as_str()) {
                AIR_REMOTE_TOPIC => {
                    let event: InputEvent = serde_json::from_str(&payload).unwrap();
                    match event {
                        InputEvent::PowerButton => {
                            client.publish(
                                TV_REMOTE_COMMAND_TOPIC,
                                QoS::AtLeastOnce,
                                false,
                                if (tv_on) { OFF } else { ON },
                            ).unwrap();
                        }
                        _ => {
                            println!("{:?}", event);
                        }
                    }
                }
                TV_STATE_TOPIC => {
                    tv_on = payload != "unavailable";
                    println!("TV STATE {:?}", tv_on);
                }
                TV_INPUT_TOPIC => {}
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
