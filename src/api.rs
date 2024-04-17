
use std::{collections::HashMap, net::SocketAddr};

use poem::{error::InternalServerError, get, http::{Method, StatusCode}, listener::TcpListener, post, Endpoint, Request, Response, Route, Server};
use serde::Serialize;
use tokio::task::JoinHandle;

use crate::{models::{ConnectionHealth, Message, MessageVariant, ServerState}, server::VRLabServer, AsyncHandle};

#[derive(Serialize)]
struct HeadsetInfo {
    name: String,
    recv: usize,
    connection_health: ConnectionHealth,
}

impl HeadsetInfo {
    pub async fn collect_from_state(state: &ServerState) -> HashMap<String, HeadsetInfo> {
        let mut infos = HashMap::new();

        for (id, headset) in state.headset_iter() {
            let rh = headset.read().await;
            
            infos.insert(id.to_owned(), HeadsetInfo {
                name: rh.name.clone(),
                recv: rh.recv,
                connection_health: rh.connection_health(),
            });
        }

        infos
    }
}

/*
Endpoint to read the headsets connected.
*/
struct HeadsetsEndpoint(AsyncHandle<ServerState>);

#[poem::async_trait]
impl Endpoint for HeadsetsEndpoint {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        if req.method() == Method::GET {
            let body = {
                let state = self.0.read().await;
                let headset_info = HeadsetInfo::collect_from_state(&state).await;
                serde_json::to_string_pretty(&headset_info).map_err(InternalServerError)?
            };

            Ok(Response::builder()
                .status(StatusCode::OK)
                .body(body))
        } else {
            Ok(Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .body(()))
        }
    }
}

/*
Endpoint to get the list of sent messages.
*/

struct GetCommandsEndpoint(AsyncHandle<ServerState>);

#[poem::async_trait]
impl Endpoint for GetCommandsEndpoint {
    type Output = Response;

    async fn call(&self, _: Request) -> poem::Result<Response> {
        let body = {
            let state = self.0.read().await;
            serde_json::to_string_pretty(state.messages()).map_err(InternalServerError)?
        };

        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(body))
    }
}

/*
Endpoint to send messages.
*/

struct PostCommandEndpoint(AsyncHandle<VRLabServer>);

#[poem::async_trait]
impl Endpoint for PostCommandEndpoint {
    type Output = Response;

    async fn call(&self, mut req: Request) -> poem::Result<Response> {
        let mut server = self.0.write().await;
        let msg_type = req.path_params::<usize>()?;
        let msg: Message = req.take_body()
            .into_json::<MessageVariant>().await?
            .into();

        log::debug!("Recv message with id: {}", msg.id());

        if msg_type == msg.id() {
            if let Err(why) = server.broadcast(msg).await {
                Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(format!("Failed to broadcast message: {}", why))
                )
            } else {
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .finish())
            }
        } else {
            Ok(
                Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .finish()
            )
        }
    }
}

// Start up the web server.
pub async fn init_api(addr: &str, vrlab_server: AsyncHandle<VRLabServer>) -> JoinHandle<Result<(), std::io::Error>> {
    let state = vrlab_server.read().await.get_state_handle();

    let api = Route::new()
        .at("/headsets", get(HeadsetsEndpoint(state.clone())))
        .at("/commands", get(GetCommandsEndpoint(state.clone())))
        .at("/command/:type", post(PostCommandEndpoint(vrlab_server.clone())));

    let addr: SocketAddr = addr.parse()
        .expect("Invalid socket address provided for the web API");

    let server = Server::new(TcpListener::bind(addr))
        .name("vr-lab-api");

    // Spawn the web server in a separate task executor.
    tokio::spawn(async move {
        log::info!("Starting the API server...");
        server.run(api).await
    })
}
