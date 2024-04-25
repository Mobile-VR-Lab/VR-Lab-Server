
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use anyhow::anyhow;
use futures::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use quanta::Clock;
use serde_derive::{Deserialize, Serialize};
use tokio::{net::{tcp::{OwnedReadHalf, OwnedWriteHalf}, TcpListener, TcpStream}, sync::{mpsc::{channel, Receiver, Sender}, RwLock}, task::JoinHandle};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use crate::{models::{HeadsetState, Message, Response, ServerState}, AsyncHandle};

struct VRClient {
    mtx: Sender<Message>,
    closed: Arc<RwLock<bool>>,
}

impl VRClient {
    /*
    Initial connection handshake.
    */
    async fn initiate_connection(rx: &mut FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
        tx: &mut FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>) -> Option<(String, String)> {

        log::debug!("Starting initial connection handshake, first sending protocol version...");

        // Send version struct.
        if let Err(why) = tx.send(serde_json::to_string(&VRLabServer::VERSION).unwrap().into()).await {
            log::error!("Failed to send the protocol version to the headset: {}", why)
        };

        // Get the headset ID.
        let hset_id_bytes = rx.next().await
        .map(|x| {
            x.inspect_err(|why| {
                log::error!("Headset failed to respond with its ID: {}", why)
            }).ok()
        })
        .flatten();

        if hset_id_bytes.is_none() {
            log::error!("Headset closed the connection unexpectedly instead of replying with its ID");
        }

        let headset_id = hset_id_bytes.map(|x| {
            String::from_utf8(x.into())
            .inspect_err(|why| {
                log::error!("Encountered decode error on headset ID response. Is it valid UTF-8? Reason: {}", why)
            })
            .ok()
            .and_then(|str| {
                serde_json::from_str::<HeadsetIdResponse>(str.as_str())
                .map(|response| response.id)
                .inspect_err(|why| {
                    log::error!("Failed to serialize headset ID response: {}", why)
                })
                .ok()
            })
        })
        .flatten();

        if headset_id.is_none() {
            log::error!("Headset did not successfully reply with an ID.");
        } else {
            log::debug!("Received the headset's ID...")
        }

        // Gen a new random session name.
        let session_name = SessionName::get_new_name().await;

        // Send to the client to use for display.
        tx.send(
            serde_json::to_string(&session_name).unwrap().into()
        ).await
        .inspect_err(|why| {
            log::error!("Failed to transmit session name back to headset: {}", why)
        })
        .ok()
        .and_then( // Return a tuple of the headset ID and the session name.
            |_| headset_id.map(|hid| (hid, session_name.session_name))
        )
    }

    async fn send_loop(mut message_tx: FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
        mut message_recv: Receiver<Message>,
        headset_state: AsyncHandle<HeadsetState>) {
        let clock = Clock::new();

        'net: loop {
            let msg = message_recv.recv().await;
            let send_time = clock.now();

            if let Some(msg) = msg {
                let msg_json = serde_json::to_string(&msg).unwrap();

                {
                    let mut wh = headset_state.write().await;
                    wh.push_send_time(send_time);
                }

                if let Err(why) = message_tx.send(msg_json.into()).await {
                    log::info!("Producer thread terminated. Reason: {}", why);
                    break 'net;
                }
            } else {
                log::debug!("Producer thread terminated because the stream ended");
                break 'net;
            }
        }
    }

    /*
    Loop that constantly reads responses coming from headsets, and passes them up to the server
    through another channel.
    */
    async fn recv_loop(mut response_rx: FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
        headset_state: AsyncHandle<HeadsetState>) {
        let clock = Clock::new();

        'net: loop {
            let response = response_rx.next()
                .await;

            let recv_time = clock.now();

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
                            let mut wh = headset_state.write().await;
                            wh.push_response(resp);
                            wh.push_recv_time(recv_time);
                            wh.recv += 1;
                        }
                        Err(why) | Ok(Err(why)) => {
                            log::error!("Got bad JSON from a headset. Serialization failed. Reason: {}", why);
                            break 'net;
                        }
                    }
                }
            }
        }
    }

    async fn do_retransmissions(client: VRClient, state: AsyncHandle<ServerState>) -> anyhow::Result<VRClient> {
        /*
        Collect relevant messages and retransmit them to a connecting client.
        */
        let state = state.read().await;

        if let Some(msg) = state.messages().iter().filter(|msg| msg.id() == 1).last() {
            client.mtx().send_timeout(msg.to_owned(), Duration::from_secs(5)).await?;
        }

        Ok(client)
    }

    /*
    Initializes a new connection and returns an object containing the sink for messages.
    */
    async fn new(stream: TcpStream, state: AsyncHandle<ServerState>) -> Result<VRClient, anyhow::Error> {
        // The buffer length is arbitrary but it could be made configurable.
        // Channel for sending messages into a connection thread.
        let (mtx, mrx) = channel(8);

        // Split stream into read/write sides
        let (response_rx, message_tx) = stream.into_split();

        // Wrap streams as a source and sink respectively, using the Length Delimited Codec.
        // It writes the length of the byte payload as an unsigned 32-bit integer prior to writing the bytes.
        let mut response_rx = FramedRead::new(response_rx, LengthDelimitedCodec::new());
        let mut message_tx = FramedWrite::new(message_tx, LengthDelimitedCodec::new());

        // Connection initiation stuff goes here.
        let ident = Self::initiate_connection(&mut response_rx, &mut message_tx).await;

        if ident.is_none() {
            return Err(anyhow!("Failed to complete the initial handshake"));
        } else {
            log::debug!("Headset has completed the initial handshake.");
        }

        // This is 100% safe past this point.
        let (hid, session_name) = ident.unwrap();
        let headset_state = HeadsetState::new(session_name.clone());
        let headset_state_2 = headset_state.clone();
        let state_2 = state.clone();

        {
            let mut wh = state.write().await;
            wh.push_headset(hid.clone(), headset_state.clone());
        }

        /*
        Spawn new threads to handle the connection.
        */

        // Allocate closed flag.
        let closed = Arc::new(RwLock::new(false));
        
        // Copies for the read and write threads.
        let csl = closed.clone();
        let crl = closed.clone();

        tokio::spawn(async move {
            Self::send_loop(message_tx, mrx, headset_state_2).await;

            *(csl.write().await) = true;
        });

        tokio::spawn(async move {
            Self::recv_loop(response_rx, headset_state).await;
            
            *(crl.write().await) = true;

            // Tidy up and drop headset once done.
            let mut wrl = state.write().await;
            wrl.drop_headset(hid.as_str());
        });

        let vrc = VRClient {
            mtx,
            closed
        };

        Self::do_retransmissions(vrc, state_2).await
    }

    fn mtx(&self) -> &Sender<Message> {
        &self.mtx
    }

    pub async fn is_closed(&self) -> bool {
        *self.closed.read().await
    }
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

static COUNTER: Lazy<Arc<RwLock<u32>>> = Lazy::new(|| {Arc::new(RwLock::new(0))});

impl SessionName {
    pub async fn get_new_name() -> SessionName {
        let handle = COUNTER.clone();
        let mut wg = handle.write().await;

        *wg += 1;
        
        SessionName {
            session_name: format!("Student {}", *wg)
        }
    }
}

pub struct VRLabServer {
    state: AsyncHandle<ServerState>,
    listener: AsyncHandle<TcpListener>,
    connections: HashMap<SocketAddr, VRClient>,
}

impl VRLabServer {
    // For now, store this here.
    const VERSION: Version = Version { version: 1 };

    /*
    Construct a new VR lab server structure.
    */
    pub async fn new(addr: &str) -> VRLabServer {
        // The listener address should be made configurable.
        let addr = addr.parse::<SocketAddr>().unwrap();
        let listener = TcpListener::bind(&addr).await.expect("Failed to bind to port");

        VRLabServer {
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

        let mut dead_connections = vec![];
        for (origin, sender) in self.connections.iter() {
            let result = sender.mtx().send_timeout(message.clone(), Duration::from_secs(5)).await;
            
            if sender.is_closed().await || result.is_err() {
                dead_connections.push(origin.to_owned())
            }
        }

        for d in dead_connections {
            log::debug!("Cleaning up connection to {}", d);
            self.connections.remove(&d);
        }

        Ok(())
    }

    pub async fn add_connection(&mut self, origin: SocketAddr, stream: TcpStream) {
        match VRClient::new(
            stream,
            self.get_state_handle(),
        ).await {
            Ok(client) => {
                self.connections.insert(origin, client);
            }
            Err(why) => {
                log::error!("Headset at addr {} failed to connect: {}", origin, why)
            }
        }
    }

    /*
    Get a reference to the TCP listener.
    */
    pub fn listener(&self) -> AsyncHandle<TcpListener> {
        self.listener.clone()
    }
}

pub async fn initialize_server(addr: &str) -> (AsyncHandle<VRLabServer>, JoinHandle<()>) {
    // We need a large channel to handle incoming responses.
    let lab_server = VRLabServer::new(addr).await;
    let handle: AsyncHandle<VRLabServer> = Arc::new(RwLock::new(lab_server));
    let gen_handle: AsyncHandle<VRLabServer> = handle.clone();

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

    (handle, connection_handler_handle)
}
