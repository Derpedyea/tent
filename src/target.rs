//! What to connect to: a telnet host or a serial port.

use std::fmt;

pub const DEFAULT_BAUD: u32 = 115_200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Tcp { host: String, port: u16 },
    Serial(SerialCfg),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialCfg {
    pub path: String,
    pub baud: u32,
    /// 5..=8
    pub data: u8,
    /// 'N', 'E' or 'O'
    pub parity: char,
    /// 1 or 2
    pub stop: u8,
}

impl SerialCfg {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into(), baud: DEFAULT_BAUD, data: 8, parity: 'N', stop: 1 }
    }

    /// "115200 8N1"
    pub fn settings(&self) -> String {
        format!("{} {}{}{}", self.baud, self.data, self.parity, self.stop)
    }
}

impl Target {
    /// Short name for titles and the status bar.
    pub fn label(&self) -> String {
        match self {
            Target::Tcp { host, port: 23 } => host.clone(),
            Target::Tcp { host, port } if host.contains(':') => format!("[{host}]:{port}"),
            Target::Tcp { host, port } => format!("{host}:{port}"),
            Target::Serial(s) => s.path.clone(),
        }
    }
}

/// Round-trips through [`parse_target`]; used for the recent-hosts file.
impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Target::Tcp { host, port } => write!(f, "{host} {port}"),
            Target::Serial(s) => write!(f, "{} {}", s.path, s.settings()),
        }
    }
}

/// Telnet: `host`, `host:port`, `host port`, `[v6addr]:port`, `telnet://host:port/`.
/// Serial: `/dev/ttyUSB0`, `COM3`, `serial:<port>`, each optionally followed by
/// a baud rate and framing, e.g. `COM3 9600`, `/dev/ttyACM0@115200 7E1`.
pub fn parse_target(s: &str) -> Result<Target, String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("serial:") {
        return parse_serial(rest.strip_prefix("//").unwrap_or(rest));
    }
    let first = s.split([' ', '\t', '@']).next().unwrap_or("");
    if first.starts_with("/dev/") || is_com_port(first) {
        return parse_serial(s);
    }
    parse_tcp(s)
}

fn is_com_port(s: &str) -> bool {
    let s = s.strip_prefix(r"\\.\").unwrap_or(s);
    s.len() > 3 && s[..3].eq_ignore_ascii_case("com") && s[3..].bytes().all(|b| b.is_ascii_digit())
}

fn parse_serial(s: &str) -> Result<Target, String> {
    let mut parts = s.split([' ', '\t', '@', ',']).filter(|p| !p.is_empty());
    let path = parts.next().ok_or("enter a serial port")?;
    let mut cfg = SerialCfg::new(if is_com_port(path) { path.to_ascii_uppercase() } else { path.to_string() });
    for p in parts {
        if let Ok(baud) = p.parse::<u32>() {
            if !(50..=10_000_000).contains(&baud) {
                return Err(format!("invalid baud rate “{p}”"));
            }
            cfg.baud = baud;
        } else if let [d @ b'5'..=b'8', par, st @ (b'1' | b'2')] = p.to_ascii_uppercase().as_bytes()
            && matches!(par, b'N' | b'E' | b'O')
        {
            (cfg.data, cfg.parity, cfg.stop) = (d - b'0', *par as char, st - b'0');
        } else {
            return Err(format!("expected baud or framing like 8N1, got “{p}”"));
        }
    }
    Ok(Target::Serial(cfg))
}

fn parse_tcp(s: &str) -> Result<Target, String> {
    let s = s.strip_prefix("telnet://").unwrap_or(s).trim_end_matches('/');
    let s = s.rsplit_once('@').map_or(s, |(_, h)| h); // drop user@
    if s.is_empty() {
        return Err("enter a host".into());
    }
    let (host, port) = if let Some((h, p)) = s.split_once(char::is_whitespace) {
        (h, Some(p.trim()))
    } else if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').ok_or("missing ]")?;
        (h, tail.strip_prefix(':'))
    } else if s.matches(':').count() == 1 {
        let (h, p) = s.split_once(':').unwrap();
        (h, Some(p))
    } else {
        (s, None) // bare host, or bare IPv6 address
    };
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("invalid host".into());
    }
    let port = match port {
        None | Some("") => 23,
        Some(p) => p.parse::<u16>().ok().filter(|&p| p != 0).ok_or(format!("invalid port “{p}”"))?,
    };
    Ok(Target::Tcp { host: host.to_string(), port })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp(h: &str, port: u16) -> Target {
        Target::Tcp { host: h.into(), port }
    }

    fn ser(path: &str, baud: u32, data: u8, parity: char, stop: u8) -> Target {
        Target::Serial(SerialCfg { path: path.into(), baud, data, parity, stop })
    }

    #[test]
    fn parses_targets() {
        let ok = |s: &str| parse_target(s).unwrap();
        assert_eq!(ok("towel.blinkenlights.nl"), tcp("towel.blinkenlights.nl", 23));
        assert_eq!(ok("  host:4000 "), tcp("host", 4000));
        assert_eq!(ok("host 4000"), tcp("host", 4000));
        assert_eq!(ok("telnet://user@host:2323/"), tcp("host", 2323));
        assert_eq!(ok("[::1]:24"), tcp("::1", 24));
        assert_eq!(ok("::1"), tcp("::1", 23));
        assert_eq!(ok("host:"), tcp("host", 23));
        assert_eq!(ok("comet.example"), tcp("comet.example", 23));

        assert_eq!(ok("/dev/ttyUSB0"), ser("/dev/ttyUSB0", 115_200, 8, 'N', 1));
        assert_eq!(ok("com3 9600"), ser("COM3", 9600, 8, 'N', 1));
        assert_eq!(ok("COM12@57600,7e2"), ser("COM12", 57600, 7, 'E', 2));
        assert_eq!(ok("/dev/ttyACM0 8O1 19200"), ser("/dev/ttyACM0", 19200, 8, 'O', 1));
        assert_eq!(ok("serial:/dev/pts/7"), ser("/dev/pts/7", 115_200, 8, 'N', 1));
        assert_eq!(ok("serial://COM1 300"), ser("COM1", 300, 8, 'N', 1));

        for bad in ["", "   ", "host:99999", "host:0", "host:abc", "[::1", ":23", "COM3 fast", "COM3 9N1", "COM3 8X1", "/dev/ttyS0 0", "serial:"] {
            assert!(parse_target(bad).is_err(), "{bad:?} should fail");
        }
        // Display round-trips (recent-hosts file format).
        for t in [tcp("::1", 24), tcp("h", 23), ser("COM4", 9600, 7, 'E', 2)] {
            assert_eq!(parse_target(&t.to_string()).unwrap(), t);
        }
    }
}
