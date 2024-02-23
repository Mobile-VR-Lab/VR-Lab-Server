
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use futures::{SinkExt, StreamExt};
use serde_derive::{Deserialize, Serialize};
use tokio::{net::{tcp::{OwnedReadHalf, OwnedWriteHalf}, TcpListener, TcpStream}, sync::{RwLock, mpsc::{channel, Receiver, Sender}}, task::JoinHandle};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use crate::{AsyncHandle, models::{Message, Response, ServerState}};

pub struct HeadsetResp {
    pub resp: Response,
    pub headset_id: String,
}

pub struct VRLabServer {
    response_sink: Sender<HeadsetResp>,
    state: AsyncHandle<ServerState>,
    listener: AsyncHandle<TcpListener>,
    connections: HashMap<SocketAddr, Sender<Message>>,
}


// Some types used for the initial handshake.
// This is to store the protocol version.
#[derive(Serialize)]
struct Version {
    pub version: usize,
}

#[derive(Deserialize)]
struct HeadsetIdResponse {
    pub id: String,
}

#[derive(Serialize)]
struct SessionName {
    pub session_name: String,
}

impl SessionName {
    pub fn get_new_name() -> SessionName {
        // TODO: Generate unique names that never conflict.
        // To replicate the Podman name gen, we could have sets of adjectives and nouns that we pull from.
        // Make them kid-friendly in vocab level so they can understand it. Or an alternative solution that works.
        SessionName {
            session_name: "Place Holder".to_owned()
        }
    }
}

impl VRLabServer {
    // For now, store this here.
    const VERSION: Version = Version { version: 1 };

    /*
    Construct a new VR lab server structure.
    */
    pub async fn new(response_sink: Sender<HeadsetResp>) -> VRLabServer {
        // The listener address should be made configurable.
        let addr = "0.0.0.0:15300".parse::<SocketAddr>().unwrap();
        let listener = TcpListener::bind(&addr).await.expect("Failed to bind to port");

        VRLabServer {
            response_sink,
            listener: Arc::new(RwLock::new(listener)),
            connections: HashMap::new(),
            state: Arc::new(RwLock::new(ServerState::new()))
        }
    }

    pub fn get_state_handle(&self) -> AsyncHandle<ServerState> {
        self.state.clone()
    }

    pub async fn broadcast(&mut self, message: Message) -> anyhow::Result<()> {
        {
            let mut state = self.state.write().await;
            state.push_message(message.clone());
        }

        for (_, sender) in self.connections.iter_mut() {
            sender.send_timeout(message.clone(), Duration::from_secs(5)).await?;
        }

        Ok(())
    }

    /*
    Initial connection handshake.
    */
    async fn initiate_connection(&mut self,
        rx: &mut FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
        tx: &mut FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>) -> Option<(String, String)> {

        // Send version struct.
        tx.send(serde_json::to_string(&Self::VERSION).unwrap().into()).await
        .expect("Failed to send initial version msg");

        // Get the headset ID.
        let headset_id = rx.next().await
        .map(|x| {
            x.ok()
        })
        .flatten()
        .map(|x| {
            String::from_utf8(x.into()).ok()
            .and_then(|str| {
                serde_json::from_str::<HeadsetIdResponse>(str.as_str())
                .map(|response| response.id)
                .ok()
            })
        })
        .flatten();

        // Gen a new random session name.
        let session_name = SessionName::get_new_name();

        // Send to the client to use for display.
        tx.send(
            serde_json::to_string(&session_name).unwrap().into()
        ).await.ok()
        .and_then( // Return a tuple of the headset ID and the session name.
            |_| headset_id.map(|hid| (hid, session_name.session_name))
        )
    }

    pub async fn add_connection(&mut self, origin: SocketAddr, stream: TcpStream) {
        // The buffer length is arbitrary but it could be made configurable.
        // Channel for sending messages into a connection thread.
        let (mtx, mrx) = channel(8);
        // Channel for sending responses back to the middleman thread.
        let rtx = self.response_sink.clone();

        let state = self.get_state_handle();

        let (response_rx, message_tx) = stream.into_split();

        // Wrap streams as a source and sink respectively, using the Length Delimited Codec.
        // It writes the length of the byte payload as an unsigned 32-bit integer prior to writing the bytes.
        let mut response_rx = FramedRead::new(response_rx, LengthDelimitedCodec::new());
        let mut message_tx = FramedWrite::new(message_tx, LengthDelimitedCodec::new());

        // Connection initiation stuff goes here.
        let ident = self.initiate_connection(&mut response_rx, &mut message_tx).await;

        if ident.is_none() {
            log::warn!("Failed to initiate connection with headset connecting from address {}.", origin);
            log::warn!("Headset did not complete the handshake.");
            return;
        }

        // This is 100% safe past this point.
        let (hid, session_name) = ident.unwrap();

        self.connections.insert(origin, mtx);
        
        {
            let mut state = self.state.write().await;
            state.push_headset(hid.clone(), session_name);
        }

        /*
        Spawn new threads to handle the connection.
        */

        // This is a consumer thread, eating incoming messages and sending them over the owned TCP socket write half.
        tokio::spawn(async move {
            let mut message_recv: Receiver<Message>  = mrx; // Move the message receiver into the new task.
            let mut message_tx = message_tx; // Move the write half into this consumer thread.

            'net: loop {
                let msg = message_recv.recv().await;

                if let Some(msg) = msg {
                    let msg_json = serde_json::to_string(&msg).unwrap();

                    if let Err(why) = message_tx.send(msg_json.into()).await {
                        log::info!("Producer thread terminated. Reason: {}", why);
                        break 'net;
                    }
                } else {
                    message_tx.close().await.expect("Channel to shut down");
                    break 'net;
                }
            }
        });

        // Receives responses and passes them over to a thread which mutates the server state.
        tokio::spawn(async move {
            let response_send: Sender<HeadsetResp> = rtx; // Move the response sender.
            let mut response_rx = response_rx; // Move the read half into this producer thread.
            let hid = hid;

            'net: loop {
                let response = response_rx.next()
                    .await;

                match response {
                    None => {
                        log::debug!("Consumer thread terminated because the stream ended");
                        break 'net;
                    }
                    Some(Err(why)) => {
                        log::info!("Consumer thread terminated. Reason: {}", why);
                        break 'net;
                    }
                    Some(Ok(resp)) => {
                        // This should be moved into its own function.
                        let response: anyhow::Result<anyhow::Result<Response>> = String::from_utf8(resp.into())
                            .map(|response| { serde_json::from_str::<Response>(response.as_str()).map_err(|x| x.into()) })
                            .map_err(|x| x.into());

                        match response {
                            Ok(Ok(resp)) => {
                                response_send.send(
                                    HeadsetResp {
                                        headset_id: hid.clone(),
                                        resp: resp
                                    }
                                ).await.unwrap();
                            }
                            Err(why) | Ok(Err(why)) => {
                                log::error!("Got bad JSON from a headset. Serialization failed. Reason: {}", why);
                                break 'net;
                            }
                        }
                    }
                }
            }

            let mut wrl = state.write().await;
            wrl.drop_headset(hid.as_str());
        });
    }

    /*
    Get a reference to the TCP listener.
    */
    pub fn listener(&self) -> AsyncHandle<TcpListener> {
        self.listener.clone()
    }
}

pub async fn initialize_server() -> (AsyncHandle<VRLabServer>, JoinHandle<()>, JoinHandle<()>) {
    // We need a large channel to handle incoming responses.
    let (rtx, rrx) = channel(1024);

    let lab_server = VRLabServer::new(rtx).await;
    let state_handle: AsyncHandle<ServerState> = lab_server.get_state_handle();
    let handle: AsyncHandle<VRLabServer> = Arc::new(RwLock::new(lab_server));
    let gen_handle: AsyncHandle<VRLabServer> = handle.clone();

    /*
    This is a consumer thread, which eats the responses on the receive side
    of the channel and writes to the server state.
    */
    let marshal_handler = tokio::spawn(async move {
        let mut response_source: Receiver<HeadsetResp> = rrx;
        let state_handle = state_handle;
        
        'ing: loop {
            let resp = response_source.recv().await;

            if let Some(msg) = resp {
                // Acquire a write lock on the server state (dropped once out of the scope of the if-let statement)
                let mut state = state_handle.write().await;

                if let Some(headset) = state.headset_mut(msg.headset_id.as_str()) {
                    // Perform operations here to add info to the state based on the headset response.

                    headset.push_response(msg.resp);
                    headset.recv += 1;
                }
            } else {
                log::info!("All senders were dropped");
                break 'ing;
            }
        }
    });

    // Thread which handles incoming connections.
    // This may exit while the server is still running.
    let connection_handler_handle = tokio::spawn(async move {
        let handle = gen_handle;
        let listener_handle = {
            let server = handle.read().await;
            server.listener()
        };

        loop {
            // This block reads from the lock.
            let (stream, addr) = {
                listener_handle.read().await.accept().await
                .expect("Failed to accept incoming connection")
            };

            // This block writes to it.
            {
                let mut server = handle.write().await;
                server.add_connection(addr, stream).await;
            }
        }
    });

    (handle, marshal_handler, connection_handler_handle)
}
