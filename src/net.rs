//! Blocking TCP on background threads, events delivered over an async channel.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

pub enum NetEvent {
    Connected,
    Data(Vec<u8>),
    Closed(Option<String>),
}

pub struct Connection {
    tx: mpsc::Sender<Vec<u8>>,
    stream: Arc<Mutex<Option<TcpStream>>>,
}

impl Connection {
    pub fn open(host: String, port: u16) -> (Self, async_channel::Receiver<NetEvent>) {
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let stream = Arc::new(Mutex::new(None));
        let slot = stream.clone();

        thread::Builder::new()
            .name("tent-net".into())
            .spawn(move || {
                let tcp = match dial(&host, port) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ev_tx.send_blocking(NetEvent::Closed(Some(e)));
                        return;
                    }
                };
                let _ = tcp.set_nodelay(true);
                let (Ok(mut reader), Ok(mut writer)) = (tcp.try_clone(), tcp.try_clone()) else {
                    let _ = ev_tx.send_blocking(NetEvent::Closed(Some("socket clone failed".into())));
                    return;
                };
                *slot.lock().unwrap() = Some(tcp);
                let _ = ev_tx.send_blocking(NetEvent::Connected);

                thread::spawn(move || {
                    while let Ok(buf) = rx.recv() {
                        if writer.write_all(&buf).is_err() {
                            break;
                        }
                    }
                });

                let mut buf = vec![0u8; 64 * 1024];
                let reason = loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break None,
                        Ok(n) => {
                            if ev_tx.send_blocking(NetEvent::Data(buf[..n].to_vec())).is_err() {
                                return; // UI side went away
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => break Some(clean(e)),
                    }
                };
                let _ = ev_tx.send_blocking(NetEvent::Closed(reason));
            })
            .expect("spawn net thread");

        (Self { tx, stream }, ev_rx)
    }

    pub fn send(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.tx.send(bytes);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(s) = self.stream.lock().unwrap().as_ref() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

fn dial(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|e| {
            let e = clean(e);
            format!("can't resolve: {}", e.rsplit(": ").next().unwrap_or(&e).to_lowercase())
        })?;
    let mut last = format!("no addresses for {host}");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(s) => return Ok(s),
            Err(e) => last = clean(e),
        }
    }
    Err(last)
}

/// "Connection refused (os error 111)" -> "connection refused"
fn clean(e: std::io::Error) -> String {
    let s = e.to_string();
    let s = s.split(" (os error").next().unwrap_or(&s);
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}
