use std::collections::HashMap;

use serde_derive::{Deserialize, Serialize};

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
    #[serde(skip_deserializing)]
    Query {} = 0x0,
}

// /*
// Response wrapper which represents the status.
// NotReceived is a placeholder variant which could be used.
// Received means the value has actually come in on the wire.
// Terminated means the connection has been terminated, and no more responses will be received (a signal that the rx thread is dead).
// */
// #[derive(Serialize, Deserialize, Debug)]
// #[serde(untagged)]
// pub enum Response {
//     Received(RawResponse),
//     NotReceived,
//     Terminated,
// }

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

// Represents the observed state of the headset.
// If we want more data from the headset, it should be stored here.
#[derive(Serialize, Deserialize)]
pub struct HeadsetState {
    pub name: String,
    pub recv: usize,
    #[serde(skip)] // Let's omit this field when we sent headsets to the tablet.
    responses: Vec<Response>,
}

impl HeadsetState {
    pub fn push_response(&mut self, r: Response) {
        self.responses.push(r);
    }

    pub fn responses(&self) -> &Vec<Response> {
        &self.responses
    }
}

/*
Represents the state of the server. It keeps track of:
 - Relation between headset ID and an associated state.
 - Messages ordered to be sent by the server.
*/
pub struct ServerState {
    headsets: HashMap<String, HeadsetState>,
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

    pub fn headset(&self, id: &str) -> Option<&HeadsetState> {
        self.headsets.get(id)
    }

    pub fn headset_mut(&mut self, id: &str) -> Option<&mut HeadsetState> {
        self.headsets.get_mut(id)
    }

    pub fn drop_headset(&mut self, id: &str) {
        self.headsets.remove(id);
    }

    pub fn headsets(&self) -> &HashMap<String, HeadsetState> {
        &self.headsets
    }
    
    pub fn headset_iter(&self) -> impl Iterator<Item=(&str, &HeadsetState)> {
        self.headsets.iter()
            .map(|(stref, hs)| (stref.as_str(), hs))
    }

    pub fn push_headset(&mut self, id: String, session_name: String) {
        self.headsets.insert(id, HeadsetState {
            name: session_name,
            recv: 0,
            responses: Vec::new(),
        });
    }

    pub async fn num_headsets_waiting(&mut self) -> usize {
        let mut count = 0;

        for headset in self.headsets.values() {
            if headset.recv == self.messages.len() {
                count += 1;
            }
        }

        return count;
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

        assert!(query.id == 0);
        assert!(scene_change.id == 1);
        assert!(object_focus.id == 2);
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
