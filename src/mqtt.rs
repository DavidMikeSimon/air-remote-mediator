use std::time::Duration;

use rumqttc::{
    ClientError,
    Event::{Incoming, Outgoing},
    QoS,
};
use serde_json::json;
use tokio::select;
use tokio::sync::mpsc;

use crate::InternalMessage;

const WAKE_TOPIC: &str = "air-remote/usb-power-on";
const HOME_ASSISTANT_RUN_TOPIC: &str = "homeassistant_cmd/run";
const HYPER_HDR_TOPIC: &str = "HyperHDR/JsonAPI";

const HA_SCRIPT_NOTICE_DENNIS_USB_OFF: &str = "notice_dennis_usb_readiness_off";
const HA_SCRIPT_NOTICE_DENNIS_USB_ON: &str = "notice_dennis_usb_readiness_on";

#[derive(Clone, Debug)]
pub(crate) enum MqttCommand {
    NoticeUsbChange { state: bool },
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
                        WAKE_TOPIC => {
                            internal_message_tx
                                .send(InternalMessage::WakeDennis)
                                .await
                                .expect("Send wake Dennis message");
                        }
                        _ => {
                            println!("ERR: Message from unknown topic {:?}", message.topic);
                        }
                    },
                    Incoming(rumqttc::Packet::ConnAck(_)) => {
                        println!("MQTT: Ready");
                        mqtt_client
                            .subscribe(WAKE_TOPIC, QoS::AtLeastOnce)
                            .await
                            .expect("Subscribe to air remote power topic");
                    }
                    Incoming(_) => {}
                    Outgoing(_) => {}
                }
            },
            mqtt_command = mqtt_out_rx.recv() => {
                match mqtt_command {
                    None => return Ok(()),
                    Some(MqttCommand::NoticeUsbChange { state }) => {
                        send_ha_script_command(&mqtt_client,
                            match state {
                                true => HA_SCRIPT_NOTICE_DENNIS_USB_ON,
                                false => HA_SCRIPT_NOTICE_DENNIS_USB_OFF,
                            }
                        ).await.map_err(|err| err.to_string())?;
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

async fn send_ha_command(
    client: &rumqttc::AsyncClient,
    topic: &str,
    payload: &str,
) -> Result<(), ClientError> {
    println!("Sending HA command to topic {}: {}", topic, payload);
    client
        .publish(
            format!("{}/{}", HOME_ASSISTANT_RUN_TOPIC, topic),
            QoS::AtLeastOnce,
            false,
            payload,
        )
        .await
}

async fn send_ha_script_command(
    client: &rumqttc::AsyncClient,
    script_name: &str,
) -> Result<(), ClientError> {
    let payload = json!({
            "entity_id": format!("script.{}", script_name)
    })
    .to_string();
    send_ha_command(client, "script.turn_on", &payload).await
}
