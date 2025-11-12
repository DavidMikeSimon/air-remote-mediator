use rumqttc::{
    Event::{Incoming, Outgoing},
    QoS,
};
use serde_json::json;
use serde_variant::to_variant_name;
use tokio::sync::mpsc;

use crate::sony_commands::SonyCommand;
use crate::InternalMessage;

const TV_STATE_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia/state";
const TV_INPUT_TOPIC: &str = "homeassistant_statestream/media_player/sony_bravia/media_title";
const WAKE_TOPIC: &str = "air-remote/usb-power-on";
const HOME_ASSISTANT_RUN_TOPIC: &str = "homeassistant_cmd/run";

pub(crate) async fn send_ha_command(client: &rumqttc::AsyncClient, topic: &str, payload: &str) {
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

pub(crate) async fn send_ha_script_command(client: &rumqttc::AsyncClient, script_name: &str) {
    let payload = json!({
            "entity_id": format!("script.{}", script_name)
    })
    .to_string();
    send_ha_command(client, "script.turn_on", &payload).await;
}

pub(crate) async fn send_sony_command(client: &rumqttc::AsyncClient, command: SonyCommand) {
    let payload = json!({
            "entity_id": "remote.sony_bravia",
            "command": to_variant_name(&command).expect("Sony command to variant")
    })
    .to_string();
    send_ha_command(client, "remote.send_command", &payload).await;
}

pub(crate) async fn open_sony_app(client: &rumqttc::AsyncClient, app_name: &str) {
    let payload = json!({
            "entity_id": "media_player.sony_bravia",
            "media_content_id": app_name,
            "media_content_type": "app",
    })
    .to_string();
    send_ha_command(client, "media_player.play_media", &payload).await;
}

pub(crate) async fn send_media_player_command(client: &rumqttc::AsyncClient, command: &str) {
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

pub(crate) async fn mqtt_thread(
    mut mqtt_eventloop: rumqttc::EventLoop,
    mqtt_client: rumqttc::AsyncClient,
    internal_message_tx: mpsc::Sender<InternalMessage>,
) {
    loop {
        match mqtt_eventloop.poll().await {
            Ok(Incoming(rumqttc::Packet::Publish(message))) => {
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
                                payload == "\"HDMI 1\"",
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
            Ok(Incoming(rumqttc::Packet::ConnAck(_))) => {
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
