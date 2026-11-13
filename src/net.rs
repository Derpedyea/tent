//! Blocking TCP / serial I/O on background threads, events delivered over an
//! async channel.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use crate::target::{SerialCfg, Target};

pub enum NetEvent {
    Connected,
    Data(Vec<u8>),
    Closed(Option<String>),
}

pub struct Connection {
    tx: mpsc::Sender<Vec<u8>>,
    closer: Arc<Closer>,
}

/// Lets the UI side stop a blocked reader thread.
#[derive(Default)]
struct Closer {
    closed: AtomicBool,
    tcp: Mutex<Option<TcpStream>>,
}

type Events = async_channel::Sender<NetEvent>;

impl Connection {
    pub fn open(target: Target) -> (Self, async_channel::Receiver<NetEvent>) {
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let closer = Arc::new(Closer::default());
        let c = closer.clone();

        thread::Builder::new()
            .name("tent-io".into())
            .spawn(move || {
                let opened = match &target {
                    Target::Tcp { host, port } => open_tcp(host, *port, &c),
                    Target::Serial(cfg) => open_serial(cfg),
                };
                let (reader, writer) = match opened {
                    Ok(rw) => rw,
                    Err(e) => {
                        let _ = ev_tx.send_blocking(NetEvent::Closed(Some(e)));
                        return;
                    }
                };
                if c.closed.load(Ordering::Relaxed) {
                    return; // UI went away while we were connecting
                }
                let _ = ev_tx.send_blocking(NetEvent::Connected);
                spawn_writer(writer, rx);
                pump(reader, &ev_tx, &c.closed, matches!(target, Target::Serial(_)));
            })
            .expect("spawn io thread");

        (Self { tx, closer }, ev_rx)
    }

    pub fn send(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.tx.send(bytes);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.closer.closed.store(true, Ordering::Relaxed);
        if let Some(s) = self.closer.tcp.lock().unwrap().as_ref() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

type Pair = (Box<dyn Read + Send>, Box<dyn Write + Send>);

fn open_tcp(host: &str, port: u16, closer: &Closer) -> Result<Pair, String> {
    let tcp = dial(host, port)?;
    let _ = tcp.set_nodelay(true);
    let (reader, writer) = tcp
        .try_clone()
        .and_then(|r| Ok((r, tcp.try_clone()?)))
        .map_err(|_| "socket clone failed".to_string())?;
    *closer.tcp.lock().unwrap() = Some(tcp);
    Ok((Box::new(reader), Box::new(writer)))
}

fn open_serial(cfg: &SerialCfg) -> Result<Pair, String> {
    use serialport::{DataBits, FlowControl, Parity, StopBits};
    let port = serialport::new(&cfg.path, cfg.baud)
        .data_bits(match cfg.data {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        })
        .parity(match cfg.parity {
            'E' => Parity::Even,
            'O' => Parity::Odd,
            _ => Parity::None,
        })
        .stop_bits(if cfg.stop == 2 { StopBits::Two } else { StopBits::One })
        .flow_control(FlowControl::None)
        // Short timeout so the reader can notice `closed`.
        .timeout(Duration::from_millis(100))
        .open()
        .map_err(|e| serial_error(&cfg.path, e))?;
    let writer = port.try_clone().map_err(|e| serial_error(&cfg.path, e))?;
    Ok((Box::new(port), Box::new(writer)))
}

fn spawn_writer(mut writer: Box<dyn Write + Send>, rx: mpsc::Receiver<Vec<u8>>) {
    thread::spawn(move || {
        while let Ok(buf) = rx.recv() {
            if writer.write_all(&buf).and_then(|_| writer.flush()).is_err() {
                break;
            }
        }
    });
}

fn pump(mut reader: Box<dyn Read + Send>, ev_tx: &Events, closed: &AtomicBool, serial: bool) {
    let mut buf = vec![0u8; 64 * 1024];
    let reason = loop {
        if closed.load(Ordering::Relaxed) {
            return;
        }
        match reader.read(&mut buf) {
            Ok(0) => break None,
            Ok(n) => {
                if ev_tx.send_blocking(NetEvent::Data(buf[..n].to_vec())).is_err() {
                    return; // UI side went away
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
            // An unplugged USB adapter shows up as EIO / broken pipe / hangup.
            Err(_) if serial => break Some("device disconnected".into()),
            Err(e) => break Some(clean(&e.to_string())),
        }
    };
    let _ = ev_tx.send_blocking(NetEvent::Closed(reason));
}

fn dial(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs = (host, port).to_socket_addrs().map_err(|e| {
        let e = clean(&e.to_string());
        format!("can't resolve: {}", e.rsplit(": ").next().unwrap_or(&e).to_lowercase())
    })?;
    let mut last = format!("no addresses for {host}");
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(s) => return Ok(s),
            Err(e) => last = clean(&e.to_string()),
        }
    }
    Err(last)
}

fn serial_error(path: &str, e: serialport::Error) -> String {
    use serialport::ErrorKind as K;
    const BUSY: &str = "port is busy: is it open in another program?";
    let msg = clean(&e.description);
    let lower = msg.to_lowercase();
    match e.kind {
        // Windows reports both missing and in-use ports as NoDevice.
        K::NoDevice if lower.contains("access is denied") => BUSY.into(),
        K::NoDevice if lower.contains("cannot find") => format!("{path} not found"),
        K::NoDevice => BUSY.into(), // EBUSY from TIOCEXCL on Unix
        K::Io(ErrorKind::NotFound) => format!("{path} not found"),
        #[cfg(unix)]
        K::Io(ErrorKind::PermissionDenied) => match device_group(path) {
            Some(g) => format!("permission denied: add yourself to the “{g}” group"),
            None => msg,
        },
        _ => msg,
    }
}

/// Name of the group that owns a device node, e.g. "uucp" or "dialout".
#[cfg(unix)]
fn device_group(path: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let gid = std::fs::metadata(path).ok()?.gid().to_string();
    std::fs::read_to_string("/etc/group")
        .ok()?
        .lines()
        .find(|l| l.split(':').nth(2) == Some(gid.as_str()))
        .and_then(|l| l.split(':').next())
        .map(str::to_string)
}

/// "Connection refused (os error 111)" -> "connection refused"
fn clean(s: &str) -> String {
    let s = s.split(" (os error").next().unwrap_or(s).trim().trim_end_matches('.');
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

/// Serial ports the OS knows about, best first (USB adapters before legacy UARTs).
pub fn serial_ports() -> Vec<String> {
    use serialport::SerialPortType::*;
    let mut ports = serialport::available_ports().unwrap_or_default();
    // Linux registers ttyS0..31 whether or not a UART exists; type 0 means none.
    #[cfg(target_os = "linux")]
    ports.retain(|p| match p.port_name.strip_prefix("/dev/") {
        Some(n) if n.starts_with("ttyS") => {
            std::fs::read_to_string(format!("/sys/class/tty/{n}/type")).is_ok_and(|t| t.trim() != "0")
        }
        _ => true,
    });
    ports.sort_by_key(|p| (!matches!(p.port_type, UsbPort(_)), p.port_name.clone()));
    ports.into_iter().map(|p| p.port_name).collect()
}
