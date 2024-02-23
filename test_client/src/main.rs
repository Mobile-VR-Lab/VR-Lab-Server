
use std::{io::{Read, Write}, net::{SocketAddrV4, TcpStream}, thread};

use anyhow::{anyhow, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use socket2::{self, Domain, Socket, Type};
use uuid::Uuid;

/*
A tiny client for reading the messages coming in.
Not written with tokio, that is overkill.
*/

#[derive(Deserialize)]
struct Version {
    pub version: usize,
}

#[derive(Serialize)]
struct HeadsetIdent {
    pub id: String
}

#[derive(Deserialize)]
struct SessionName {
    pub session_name: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    pub id: u32,
    pub kwargs: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
struct Reply {
    pub error: bool,
    pub response: serde_json::Value,
}

struct LengthDelimMessage {
    len: u32,
    bytes: Vec<u8>,
}

// Lets me turn Message and Response into LengthDelimMessages
impl<T> From<T> for LengthDelimMessage
    where T: Serialize {
    fn from(value: T) -> Self {
        let str = serde_json::to_string(&value).unwrap();
        LengthDelimMessage { len: str.len() as u32, bytes: str.into() }
    }
}

impl LengthDelimMessage {
    fn into_json<T: DeserializeOwned>(self) -> T {
        let bytes = self.bytes;

        println!("Length: {}", self.len);

        let str: String = String::from_utf8(bytes).unwrap();
        serde_json::from_str(str.as_str()).unwrap()
    }
}

impl LengthDelimMessage {
    fn from_stream(stream: &mut TcpStream) -> Result<Self> {
        let mut len: [u8; 4] = [0; 4];
        let read_bytes = stream.read(&mut len)?;
        if read_bytes == 4 {
            let len: u32 = u32::from_be_bytes(len);
            let mut buffer: Vec<u8> = vec![0; len as usize];
            stream.read(buffer.as_mut_slice())?;

            Ok(Self {
                len: len,
                bytes: buffer,
            })
        } else {
            Err(anyhow!("Expected 4 bytes, got {}", read_bytes))
        }
    }

    fn to_stream(&self, stream: &mut TcpStream) -> Result<()> {
        stream.write(&self.len.to_be_bytes())?;
        stream.write(self.bytes.as_slice())?;

        Ok(())
    }
}

fn main() {
    let tcpc = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
    let loopback = "127.0.0.1:15300".parse::<SocketAddrV4>().unwrap().into();
    
    if let Err(why) = tcpc.connect(&loopback) {
        println!("Failed to connect. Is the server running? Reason: {}", why);
        return;
    }
     
    let (prod, cons) = std::sync::mpsc::channel::<Message>();
    let mut recv = TcpStream::from(tcpc);
    let mut send = recv.try_clone().unwrap();

    let version: Version = LengthDelimMessage::from_stream(&mut recv).unwrap().into_json();
    
    println!("Proto version: {}", version.version);

    let id = Uuid::new_v4();

    print!("Sending ident {} ... ", id);
    let ident: LengthDelimMessage = HeadsetIdent { id: id.to_string() }.into();
    ident.to_stream(&mut send).unwrap();
    println!("Done!");

    let session_name_ident: SessionName = LengthDelimMessage::from_stream(&mut recv).unwrap().into_json();
    println!("Got session name: {}", session_name_ident.session_name);

    let txh = thread::spawn(move || {
        let mut recv = recv;
        let prod = prod;

        'recv: loop {
            let msg = LengthDelimMessage::from_stream(&mut recv);

            match msg {
                Ok(item) => {
                    if let Err(_) = prod.send(item.into_json()) { break 'recv; }
                }
                Err(why) => {
                    println!("It is so over for the receiver thread: {}", why);
                    break 'recv;
                }
            }
        }
    });

    let rxh = thread::spawn(move || {
        let mut send = send;
        let cons = cons;

        'send: loop {
            match cons.recv() {
                Ok(msg) => {
                    println!("Got message type {}: {}", msg.id, msg.kwargs);
                    let repl: LengthDelimMessage = match msg.id {
                        _ => Reply { error: false, response: serde_json::Value::Null }
                    }.into();

                    if let Err(why) = repl.to_stream(&mut send) {
                        println!("It is so over for the sender thread: {}", why);
                        break 'send;
                    }
                }
                Err(why) => {
                    println!("Channel decided to explode? Here's why: {}", why);
                    break 'send;
                }
            }
        }
    });

    txh.join().unwrap();
    rxh.join().unwrap();
}
