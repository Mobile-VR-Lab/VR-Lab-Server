use std::{collections::HashMap, sync::Arc};

use quanta::Instant;
use serde_derive::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::AsyncHandle;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    id: usize,
    kwargs: MessageVariant,
}

impl Message {
    pub fn id(&self) -> usize {
        self.id
    }
}

impl From<MessageVariant> for Message {
    fn from(value: MessageVariant) -> Self {
        Message {
            // The discriminant is like an unnamed usize field at the start of the associated struct.
            id: unsafe { *(&value as *const MessageVariant as *const usize) },
            kwargs: value
        }
    }
}

/*
Message variants go here.
These correspond one-to-one with the messages in the HCP document.
*/
#[repr(usize)]
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)] // This tells Serde to not include the enum variant name in the JSON repr.
pub enum MessageVariant {
    ChangeScene {
        scene: usize,
    } = 0x1,
    FocusOnObject {
        object: String,
    } = 0x2,
    SetAttentionMode {
        attention: bool,
    } = 0x3,
    SetTransparencyMode {
        transparency: bool,
    } = 0x4,
    #[serde(skip_deserializing)]
    Query {} = 0x0,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Response {
    pub error: bool,
    pub response: Option<ResponseType>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum ResponseType {
    Error { message: String, detail: serde_json::Value },
    Query { /* ... */ }
}

/*
A representation of a headset's connection health.
*/
#[derive(Serialize)]
pub enum ConnectionHealth {
    Excellent,
    Good,
    Poor,
    //Unknown
}

/*
Less than 50 millis means excellent, less than 200 millis means good, and worse than that is poor.
*/
impl ConnectionHealth {
    pub fn from_times<'t>(count: usize, times: impl Iterator<Item = (&'t quanta::Instant, &'t quanta::Instant)>) -> ConnectionHealth {
        if count == 0 {
            return Self::Excellent;
        }

        let taken = count.min(10);
        
        let mean_latency = times.take(taken).map(|(sen, rec)| (*rec - *sen).as_millis()).sum::<u128>() as usize / taken;

        log::debug!("Headset mean latency: {}", mean_latency);

        if mean_latency < 50 {
            Self::Excellent
        } else if mean_latency < 200 {
            Self::Good
        } else {
            Self::Poor
        }
    }
}

// Represents the observed state of the headset.
// If we want more data from the headset, it should be stored here.
pub struct HeadsetState {
    pub name: String,
    pub recv: usize,
    //#[serde(skip)] // Let's omit this field when we sent headsets to the tablet.
    responses: Vec<Response>,
    //#[serde(skip)]
    recv_times: Vec<Instant>,
    //#[serde(skip)]
    send_times: Vec<Instant>,
}

impl HeadsetState {
    pub fn new(hid: String) -> AsyncHandle<Self> {
        Arc::new(RwLock::new(HeadsetState {
            name: hid,
            recv: 0,
            responses: Vec::new(),
            recv_times: Vec::new(),
            send_times: Vec::new(),
        }))
    }

    pub fn push_response(&mut self, r: Response) {
        self.responses.push(r);
    }

    pub fn push_recv_time(&mut self, time: quanta::Instant) {
        self.recv_times.push(time);
    }

    pub fn push_send_time(&mut self, time: quanta::Instant) {
        self.send_times.push(time);
    }

    pub fn connection_health(&self) -> ConnectionHealth {
        let recv_times = self.recv_times.iter();
        let send_times = self.send_times.iter();
        let recv_len = self.recv_times.len();
        let send_len = self.send_times.len();

        if recv_len < send_len {
            ConnectionHealth::from_times(recv_len, send_times.skip(send_len - recv_len).zip(recv_times))
        } else {
            ConnectionHealth::from_times(recv_len, send_times.zip(recv_times))
        }
    }
}

/*
Represents the state of the server. It keeps track of:
 - Relation between headset ID and an associated state.
 - Messages ordered to be sent by the server.
*/
pub struct ServerState {
    headsets: HashMap<String, AsyncHandle<HeadsetState>>,
    messages: Vec<Message>,
}

impl ServerState {
    pub fn new() -> ServerState {
        // Create a new server. Semaphore will start with zero permits.
        // We can notify waiting headsets using this.
        ServerState {
            headsets: HashMap::new(),
            messages: Vec::new(),
        }
    }

    pub fn messages(&self) -> &Vec<Message> {
        &self.messages
    }

    pub fn drop_headset(&mut self, id: &str) {
        self.headsets.remove(id);
    }
    
    pub fn headset_iter(&self) -> impl Iterator<Item=(&str, &AsyncHandle<HeadsetState>)> {
        self.headsets.iter()
            .map(|(stref, hs)| (stref.as_str(), hs))
    }

    pub fn push_headset(&mut self, id: String, hset: AsyncHandle<HeadsetState>) {
        self.headsets.insert(id, hset);
    }

    /*
    Push in a new message.
    */
    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
    }
}

mod test {
    use crate::models::Message;

    use super::{MessageVariant, Response, ResponseType};

    #[test]
    fn message_type_discriminants() {
        let query: Message = MessageVariant::Query {}.into();
        let scene_change: Message = MessageVariant::ChangeScene { scene: 1 }.into();
        let object_focus: Message = MessageVariant::FocusOnObject { object: "object".to_owned() }.into();
        let attn: Message = MessageVariant::SetAttentionMode { attention: true }.into();
        let trans: Message = MessageVariant::SetTransparencyMode { transparency: true }.into();

        assert!(query.id == 0);
        assert!(scene_change.id == 1);
        assert!(object_focus.id == 2);
        assert!(attn.id == 3);
        assert!(trans.id == 4);
    }

    #[test]
    fn serde_messages() {
        let query: Message = MessageVariant::Query {}.into();
        let serialized: String = serde_json::to_string(&query).unwrap();
        let expected = "{\"id\":0,\"kwargs\":{}}";

        assert_eq!(serialized, expected)
    }

    #[test]
    fn serde_responses() {
        let resp = Response { 
            error: true,
            response: Some(
                ResponseType::Error {
                    message: "Object did not exist".to_owned(),
                    detail: serde_json::Value::Null
                }
            )
        };

        let serialized: String = serde_json::to_string(&resp).unwrap();
        let expected = "{\"error\":true,\"response\":{\"message\":\"Object did not exist\",\"detail\":null}}";

        assert_eq!(serialized, expected)
    }
}
