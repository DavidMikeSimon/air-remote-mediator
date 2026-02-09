use std::time::Duration;

use const_format::formatcp;
use rumqttc::{
    ClientError,
    Event::{Incoming, Outgoing},
    QoS,
};
use serde_json::json;
use tokio::select;
use tokio::sync::mpsc;

use crate::InternalMessage;

const HYPER_HDR_TOPIC: &str = "HyperHDR/JsonAPI";
const HA_DISCOVERY_TOPIC: &str = "homeassistant/device/air-remote-living-room/config";

const HA_DEVICE_TOPIC_BASE: &str = "air-remote-living-room";
const DENNIS_STATE_TOPIC: &str = formatcp!("{HA_DEVICE_TOPIC_BASE}/dennis/state");
const DENNIS_COMMAND_TOPIC: &str = formatcp!("{HA_DEVICE_TOPIC_BASE}/dennis/command");
const TV_STATE_TOPIC: &str = formatcp!("{HA_DEVICE_TOPIC_BASE}/tv/state");
const TV_COMMAND_TOPIC: &str = formatcp!("{HA_DEVICE_TOPIC_BASE}/tv/command");

#[derive(Clone, Debug)]
pub(crate) enum MqttCommand {
    NoticeUsbChange { state: bool },
    NoticeTvChange { state: bool },
    SetHyperHdr { state: bool },
}

pub(crate) async fn mqtt_thread(
    internal_message_tx: mpsc::Sender<InternalMessage>,
    mut mqtt_out_rx: mpsc::Receiver<MqttCommand>,
) {
    loop {
        let exit = mqtt_loop(&internal_message_tx, &mut mqtt_out_rx).await;
        if let Err(error) = exit {
            println!("MQTT connection lost: {}", error);
        }

        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn mqtt_loop(
    internal_message_tx: &mpsc::Sender<InternalMessage>,
    mqtt_out_rx: &mut mpsc::Receiver<MqttCommand>,
) -> Result<(), String> {
    let mut mqtt_options =
        rumqttc::MqttOptions::new("air-remote-mediator-pi", "mqtt.sinclair.pipsimon.com", 1883);
    mqtt_options.set_credentials(
        "lcars",
        std::env::var("MQTT_PASS").expect("Need env var MQTT_PASS"),
    );
    mqtt_options.set_keep_alive(Duration::from_secs(5));

    println!("MQTT: Connecting");
    let (mqtt_client, mut mqtt_eventloop) = rumqttc::AsyncClient::new(mqtt_options, 10);

    loop {
        select! {
            mqtt_event = mqtt_eventloop.poll() => {
                match mqtt_event.map_err(|err| err.to_string())? {
                    Incoming(rumqttc::Packet::Publish(message)) => match message.topic.as_str() {
                        DENNIS_COMMAND_TOPIC => {
                            match str::from_utf8(&message.payload).expect("Parse Dennis command message") {
                                "ON" => internal_message_tx
                                    .send(InternalMessage::WakeDennis)
                                    .await
                                    .expect("Send wake Dennis message"),
                                "OFF" => internal_message_tx
                                    .send(InternalMessage::SleepDennis)
                                    .await
                                    .expect("Send sleep Dennis message"),
                                other => println!("ERR: Unknown message {:?} on Dennis command topic", other)
                            }
                        }
                        TV_COMMAND_TOPIC => {
                            match str::from_utf8(&message.payload).expect("Parse TV command message") {
                                "ON" => internal_message_tx
                                    .send(InternalMessage::PowerOn)
                                    .await
                                    .expect("Send wake Dennis message"),
                                "OFF" => internal_message_tx
                                    .send(InternalMessage::PowerOff)
                                    .await
                                    .expect("Send sleep Dennis message"),
                                other => println!("ERR: Unknown message {:?} on TV command topic", other)
                            }
                        }
                        _ => {
                            println!("ERR: Message from unknown topic {:?}", message.topic);
                        }
                    },
                    Incoming(rumqttc::Packet::ConnAck(_)) => {
                        println!("MQTT: Ready");
                        send_discovery_payload(&mqtt_client).await.map_err(|err| err.to_string())?;
                        mqtt_client
                            .subscribe(DENNIS_COMMAND_TOPIC, QoS::AtLeastOnce)
                            .await
                            .expect("Subscribe to dennis command topic");
                        mqtt_client
                            .subscribe(TV_COMMAND_TOPIC, QoS::AtLeastOnce)
                            .await
                            .expect("Subscribe to tv command topic");
                    }
                    Incoming(_) => {}
                    Outgoing(_) => {}
                }
            },
            mqtt_command = mqtt_out_rx.recv() => {
                match mqtt_command {
                    None => return Ok(()),
                    Some(MqttCommand::NoticeUsbChange { state }) => {
                        set_binary_state(&mqtt_client, DENNIS_STATE_TOPIC, state).await.map_err(|err| err.to_string())?;
                    },
                    Some(MqttCommand::NoticeTvChange { state }) => {
                        set_binary_state(&mqtt_client, TV_STATE_TOPIC, state).await.map_err(|err| err.to_string())?;
                    },
                    Some(MqttCommand::SetHyperHdr { state }) => {
                        set_hyper_hdr(&mqtt_client, state).await.map_err(|err| err.to_string())?;
                    },
                }
            },
        }
    }
}

async fn set_hyper_hdr(client: &rumqttc::AsyncClient, state: bool) -> Result<(), ClientError> {
    println!("Sending HyperHDR state {}", state);
    client
        .publish(
            HYPER_HDR_TOPIC,
            QoS::AtLeastOnce,
            true,
            json!({
                "command":"componentstate",
                "componentstate":
                {
                        "component":"ALL",
                        "state": state
                }
            })
            .to_string(),
        )
        .await
}

async fn set_binary_state(
    client: &rumqttc::AsyncClient,
    topic: &str,
    state: bool,
) -> Result<(), ClientError> {
    println!("Setting {} state {}", topic, state);
    client
        .publish(
            topic,
            QoS::AtLeastOnce,
            true,
            match state {
                true => "ON",
                false => "OFF",
            },
        )
        .await
}

async fn send_discovery_payload(client: &rumqttc::AsyncClient) -> Result<(), ClientError> {
    println!("Sending discovery payload");
    client
        .publish(
            HA_DISCOVERY_TOPIC,
            QoS::AtLeastOnce,
            true,
            json!({
                "device": {
                    "identifiers": "air-remote-living-room",
                    "name": "Air Remote Living Room",
                },
                "origin": {
                    "name": "air-remote-mediator",
                },
                "components": {
                    "dennis": {
                        "name": "Air Remote Living Room Dennis",
                        "platform": "switch",
                        "unique_id": "dennis",
                        "payload_off": "OFF",
                        "payload_on": "ON",
                        "state_topic": DENNIS_STATE_TOPIC,
                        "command_topic": DENNIS_COMMAND_TOPIC,
                    },
                    "tv": {
                        "name": "Air Remote Living Room TV",
                        "platform": "switch",
                        "unique_id": "tv",
                        "payload_off": "OFF",
                        "payload_on": "ON",
                        "state_topic": TV_STATE_TOPIC,
                        "command_topic": TV_COMMAND_TOPIC,
                    },
                },
            })
            .to_string(),
        )
        .await
}
