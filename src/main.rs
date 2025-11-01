/*
 * ~3 seconds: TV off: homeassistant_statestream/media_player/sony_bravia_dlna/state: unavailable
 * ~3 seconds: TV on: homeassistant_statestream/media_player/sony_bravia_dlna/state: unknown OR idle OR presumably others too
 * ~3-8 seconds: Input pick: homeassistant_statestream/media_player/sony_bravia/media_title: "HDMI 1" or "HDMI 2" or "HDMI 3/ARC" or "HDMI 4"
 * ~3-8 seconds: homeassistant_statestream/media_player/sony_bravia/media_title: "Smart TV"
 */

mod sony_commands;

use std::env;
use std::thread::sleep;
use std::time::Duration;

use rppal::i2c::I2c;
use rumqttc::{
    AsyncClient,
    Event::{Incoming, Outgoing},
    EventLoop, MqttOptions,
    Packet::{ConnAck, Publish},
    QoS,
};
use serde_json::json;
use serde_variant::to_variant_name;
use tokio::{
    self,
    sync::mpsc::{channel, Receiver, Sender},
};

use sony_commands::SonyCommand;

const ADDR_AIR_REMOTE: u16 = 0x05;

const TV_STATE_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia/state";
const TV_INPUT_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia/media_title";
const WAKE_TOPIC: &str = "air-remote/usb-power-on";
const HOME_ASSISTANT_RUN_TOPIC: &str = "homeassistant_cmd/run";

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

async fn send_ha_command(client: &AsyncClient, topic: &str, payload: &str) {
    println!("Sending HA command to topic {}: {}", topic, payload);
    client
        .publish(
            format!("{}/{}", HOME_ASSISTANT_RUN_TOPIC, topic),
            QoS::AtLeastOnce,
            false,
            payload,
        )
        .await
        .expect("MQTT client publish HA command");
}

async fn send_ha_script_command(client: &AsyncClient, script_name: &str) {
    let payload = json!({
        "entity_id": format!("script.{}", script_name)
    })
    .to_string();
    send_ha_command(client, "script.turn_on", &payload).await;
}

async fn send_sony_command(client: &AsyncClient, command: SonyCommand) {
    let payload = json!({
        "entity_id": "remote.sony_bravia",
        "command": to_variant_name(&command).expect("Sony command to variant")
    })
    .to_string();
    send_ha_command(client, "remote.send_command", &payload).await;
}

async fn open_sony_app(client: &AsyncClient, app_name: &str) {
    let payload = json!({
        "entity_id": "media_player.sony_bravia",
        "media_content_id": app_name,
        "media_content_type": "app",
    })
    .to_string();
    send_ha_command(client, "media_player.play_media", &payload).await;
}

async fn send_media_player_command(client: &AsyncClient, command: &str) {
    let payload = json!({
        "entity_id": "media_player.sony_bravia",
    })
    .to_string();
    send_ha_command(
        client,
        format!("media_player.{}", command).as_ref(),
        &payload,
    )
    .await;
}

async fn mqtt_thread(
    mut mqtt_eventloop: EventLoop,
    mqtt_client: AsyncClient,
    internal_message_tx: Sender<InternalMessage>,
) {
    loop {
        match mqtt_eventloop.poll().await {
            Ok(Incoming(Publish(message))) => {
                let payload: String =
                    String::from_utf8(message.payload.into()).expect("Decode UTF-8 MQTT");
                match message.topic.as_str() {
                    TV_STATE_TOPIC => {
                        internal_message_tx
                            .send(InternalMessage::UpdateTvState(payload != "off"))
                            .await
                            .expect("Send TV state message");
                    }
                    TV_INPUT_TOPIC => {
                        internal_message_tx
                            .send(InternalMessage::UpdateDennisIsInputState(
                                "payload" == "\"HDMI 1\"",
                            ))
                            .await
                            .expect("Send Dennis state message");
                    }
                    WAKE_TOPIC => {
                        internal_message_tx
                            .send(InternalMessage::WakeDennis)
                            .await
                            .expect("Send wake Dennis message");
                    }
                    _ => {
                        println!("ERR: Message from unknown topic {:?}", message.topic);
                    }
                }
            }
            Ok(Incoming(ConnAck(_))) => {
                println!("Connected to MQTT");
                mqtt_client
                    .subscribe(TV_STATE_TOPIC, QoS::AtLeastOnce)
                    .await
                    .expect("Subscribe to TV state topic");
                mqtt_client
                    .subscribe(TV_INPUT_TOPIC, QoS::AtLeastOnce)
                    .await
                    .expect("Subscribe to TV input topic");
                mqtt_client
                    .subscribe(WAKE_TOPIC, QoS::AtLeastOnce)
                    .await
                    .expect("Subscribe to air remote power topic");
            }
            Ok(Incoming(_)) => {}
            Ok(Outgoing(_)) => {}
            Err(err) => {
                eprintln!("MQTT ERR: {}", err.to_string());
                return;
            }
        }
    }
}

fn i2c_thread(
    i2c: &mut I2c,
    internal_message_tx: Sender<InternalMessage>,
    mut i2c_out_rx: Receiver<u8>,
) {
    let mut buf = [0u8; 2];

    // Drain any events that were backed up and throw them away, they're probably no longer relevant
    loop {
        i2c.read(&mut buf).expect("I2C read");
        let [code, _data] = buf;
        if code == 0 {
            break;
        }
    }

    // Now we can actually process events
    loop {
        i2c.read(&mut buf).expect("I2C read");
        let [code, data] = buf;
        if code > 0 {
            if let Some(msg) = match code {
                b'A' => Some(InternalMessage::AsciiKey(data)),
                b'C' => Some(InternalMessage::ConsumerCode(data)),
                b'K' => Some(InternalMessage::KeyCode(data)),
                b'O' => Some(InternalMessage::OkButton),
                b'W' => Some(InternalMessage::PowerButton),
                b'U' => Some(InternalMessage::UsbReadinessStateChange(data == b'Y')),
                _ => None,
            } {
                internal_message_tx
                    .blocking_send(msg)
                    .expect("I2C recv send");
            }
        }

        if let Ok(out) = i2c_out_rx.try_recv() {
            i2c.write(&[out]).expect("I2C write");
        }

        sleep(Duration::from_millis(1));
    }
}

async fn internal_check_thread(internal_message_tx: Sender<InternalMessage>) {
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
    let mut i2c = I2c::new().expect("I2C init");

    i2c.set_slave_address(ADDR_AIR_REMOTE)
        .expect("I2C set address");
    i2c.set_timeout(10).expect("I2C set timeout");

    let mut mqtt_options =
        MqttOptions::new("air-remote-mediator-pi", "mqtt.sinclair.pipsimon.com", 1883);
    mqtt_options.set_credentials(
        "lcars",
        env::var("MQTT_PASS").expect("Need env var MQTT_PASS"),
    );
    mqtt_options.set_keep_alive(Duration::from_secs(5));

    let (mqtt_client, mqtt_connection) = AsyncClient::new(mqtt_options, 10);

    println!("Starting up");

    let (internal_message_tx, mut internal_message_rx) = channel::<InternalMessage>(100);

    let mqtt_sender = internal_message_tx.clone();
    let mqtt_thread_handle = tokio::task::spawn(mqtt_thread(
        mqtt_connection,
        mqtt_client.clone(),
        mqtt_sender,
    ));

    let i2c_sender = internal_message_tx.clone();
    let (i2c_out_tx, i2c_out_rx) = channel::<u8>(10);
    let i2c_thread_handle =
        tokio::task::spawn_blocking(move || i2c_thread(&mut i2c, i2c_sender, i2c_out_rx));

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
                send_sony_command(&mqtt_client, SonyCommand::Confirm).await;
            }
            InternalMessage::PowerButton => {
                send_ha_script_command(&mqtt_client, HA_SCRIPT_TOGGLE_TV_AND_DENNIS).await;
            }
            InternalMessage::ConsumerCode(data) => match data {
                CONSUMER_CODE_VOLUME_DOWN => {
                    send_media_player_command(&mqtt_client, "volume_down").await
                }
                CONSUMER_CODE_VOLUME_UP => {
                    send_media_player_command(&mqtt_client, "volume_up").await
                }
                CONSUMER_CODE_CHANNEL => send_sony_command(&mqtt_client, SonyCommand::Input).await,
                CONSUMER_CODE_MEDIA_SELECT_HOME => open_sony_app(&mqtt_client, "HALauncher").await,
                CONSUMER_CODE_MENU_ESCAPE => {
                    send_sony_command(&mqtt_client, SonyCommand::Return).await
                }
                CONSUMER_CODE_PLAY_PAUSE => {
                    if !state.dennis_is_current_input {
                        send_sony_command(&mqtt_client, SonyCommand::Pause).await
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
                HID_KEY_ARROW_UP => send_sony_command(&mqtt_client, SonyCommand::Up).await,
                HID_KEY_ARROW_DOWN => send_sony_command(&mqtt_client, SonyCommand::Down).await,
                HID_KEY_ARROW_LEFT => send_sony_command(&mqtt_client, SonyCommand::Left).await,
                HID_KEY_ARROW_RIGHT => send_sony_command(&mqtt_client, SonyCommand::Right).await,
                _ => println!("Unhandled key code: {:#04X}", data),
            },
            InternalMessage::UsbReadinessStateChange(data) => match data {
                false => {
                    send_ha_script_command(&mqtt_client, HA_SCRIPT_NOTICE_DENNIS_USB_OFF).await
                }
                true => send_ha_script_command(&mqtt_client, HA_SCRIPT_NOTICE_DENNIS_USB_ON).await,
            },
            InternalMessage::InternalCheck => {
                // No specific action needed, this just triggers the thread
                // handle checks above to run again.
            }
        }
    }

    eprintln!("Error: Internal message queue returned None");
}
