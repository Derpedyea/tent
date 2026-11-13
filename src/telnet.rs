//! Telnet protocol (RFC 854/855) state machine. Pure, no I/O.
//!
//! Supported options: BINARY, ECHO, SGA, TTYPE (RFC 1091), NAWS (RFC 1073).
//! Everything else is refused, so negotiation can't loop.

pub const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

pub const OPT_BINARY: u8 = 0;
pub const OPT_ECHO: u8 = 1;
pub const OPT_SGA: u8 = 3;
pub const OPT_TTYPE: u8 = 24;
pub const OPT_NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;
const TERMINAL_TYPE: &[u8] = b"XTERM-256COLOR";
/// Cap on subnegotiation payloads so a hostile server can't grow memory forever.
const MAX_SB: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Data,
    Cr,
    Iac,
    Verb(u8),
    Sb,
    SbIac,
}

#[derive(Debug)]
pub struct Telnet {
    state: State,
    sb: Vec<u8>,
    /// Options the server has enabled on its side (WILL acked by our DO).
    remote: [bool; 256],
    /// Options we have enabled on our side (DO acked by our WILL).
    local: [bool; 256],
    size: (u16, u16),
}

/// Output of feeding bytes from the network.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Received {
    /// Application data for the terminal emulator.
    pub data: Vec<u8>,
    /// Protocol replies to send back to the server.
    pub reply: Vec<u8>,
}

impl Telnet {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            state: State::Data,
            sb: Vec::new(),
            remote: [false; 256],
            local: [false; 256],
            size: (cols, rows),
        }
    }

    /// Whether the server echoes our input (otherwise we're in NVT line mode).
    pub fn remote_echo(&self) -> bool {
        self.remote[OPT_ECHO as usize]
    }

    fn binary_out(&self) -> bool {
        self.local[OPT_BINARY as usize]
    }

    fn binary_in(&self) -> bool {
        self.remote[OPT_BINARY as usize]
    }

    pub fn receive(&mut self, input: &[u8]) -> Received {
        let mut out = Received::default();
        for &b in input {
            self.state = match self.state {
                State::Data | State::Cr => {
                    let after_cr = self.state == State::Cr;
                    match b {
                        IAC => State::Iac,
                        // NVT: CR NUL means a bare CR; drop the NUL.
                        0 if after_cr && !self.binary_in() => State::Data,
                        b'\r' => {
                            out.data.push(b);
                            State::Cr
                        }
                        _ => {
                            out.data.push(b);
                            State::Data
                        }
                    }
                }
                State::Iac => match b {
                    IAC => {
                        out.data.push(IAC);
                        State::Data
                    }
                    WILL | WONT | DO | DONT => State::Verb(b),
                    SB => {
                        self.sb.clear();
                        State::Sb
                    }
                    // NOP, GA, DM, AYT, etc.: nothing to do.
                    _ => State::Data,
                },
                State::Verb(verb) => {
                    self.negotiate(verb, b, &mut out.reply);
                    State::Data
                }
                State::Sb => {
                    if b == IAC {
                        State::SbIac
                    } else {
                        if self.sb.len() < MAX_SB {
                            self.sb.push(b);
                        }
                        State::Sb
                    }
                }
                State::SbIac => match b {
                    SE => {
                        self.subnegotiation(&mut out.reply);
                        State::Data
                    }
                    IAC => {
                        if self.sb.len() < MAX_SB {
                            self.sb.push(IAC);
                        }
                        State::Sb
                    }
                    // Malformed: IAC followed by something other than SE/IAC
                    // inside SB. Abort the subnegotiation and treat it as a command.
                    _ => {
                        self.sb.clear();
                        State::Data
                    }
                },
            };
        }
        out
    }

    fn negotiate(&mut self, verb: u8, opt: u8, reply: &mut Vec<u8>) {
        let i = opt as usize;
        match verb {
            WILL => {
                let accept = matches!(opt, OPT_ECHO | OPT_SGA | OPT_BINARY);
                if accept {
                    if !self.remote[i] {
                        self.remote[i] = true;
                        reply.extend_from_slice(&[IAC, DO, opt]);
                    }
                } else {
                    reply.extend_from_slice(&[IAC, DONT, opt]);
                }
            }
            WONT if self.remote[i] => {
                self.remote[i] = false;
                reply.extend_from_slice(&[IAC, DONT, opt]);
            }
            DO => {
                let accept = matches!(opt, OPT_TTYPE | OPT_NAWS | OPT_SGA | OPT_BINARY);
                if accept {
                    if !self.local[i] {
                        self.local[i] = true;
                        reply.extend_from_slice(&[IAC, WILL, opt]);
                    }
                    if opt == OPT_NAWS {
                        reply.extend(self.naws_bytes());
                    }
                } else {
                    reply.extend_from_slice(&[IAC, WONT, opt]);
                }
            }
            DONT if self.local[i] => {
                self.local[i] = false;
                reply.extend_from_slice(&[IAC, WONT, opt]);
            }
            _ => {}
        }
    }

    fn subnegotiation(&mut self, reply: &mut Vec<u8>) {
        if self.sb.as_slice() == [OPT_TTYPE, TTYPE_SEND] && self.local[OPT_TTYPE as usize] {
            reply.extend_from_slice(&[IAC, SB, OPT_TTYPE, TTYPE_IS]);
            reply.extend_from_slice(TERMINAL_TYPE);
            reply.extend_from_slice(&[IAC, SE]);
        }
        self.sb.clear();
    }

    /// Record a new window size. Returns the NAWS update to send, if negotiated.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Option<Vec<u8>> {
        if self.size == (cols, rows) {
            return None;
        }
        self.size = (cols, rows);
        self.local[OPT_NAWS as usize].then(|| self.naws_bytes())
    }

    fn naws_bytes(&self) -> Vec<u8> {
        let (c, r) = self.size;
        let mut v = vec![IAC, SB, OPT_NAWS];
        for b in c.to_be_bytes().into_iter().chain(r.to_be_bytes()) {
            v.push(b);
            if b == IAC {
                v.push(IAC);
            }
        }
        v.extend_from_slice(&[IAC, SE]);
        v
    }

    /// Encode application bytes (keystrokes) for the wire: double IAC and,
    /// outside binary mode, turn a bare CR into CR NUL.
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 2);
        let mut it = data.iter().peekable();
        while let Some(&b) = it.next() {
            out.push(b);
            if b == IAC {
                out.push(IAC);
            } else if b == b'\r' && !self.binary_out() && it.peek() != Some(&&b'\n') {
                out.push(0);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_data_and_malformed_input() {
        let mut t = Telnet::new(80, 24);

        // Good: typical telnetd opening + escaped IAC + CR NUL in data.
        let r = t.receive(&[
            IAC, DO, OPT_TTYPE, IAC, DO, OPT_NAWS, IAC, WILL, OPT_ECHO, IAC, DO, 39, b'h', b'i',
            b'\r', 0, IAC, IAC,
        ]);
        assert_eq!(r.data, b"hi\r\xff");
        assert_eq!(
            r.reply,
            [
                &[IAC, WILL, OPT_TTYPE][..],
                &[IAC, WILL, OPT_NAWS],
                &[IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE],
                &[IAC, DO, OPT_ECHO],
                &[IAC, WONT, 39],
            ]
            .concat()
        );
        assert!(t.remote_echo());

        // Repeated request for an already-enabled option must not be re-acked (no loops).
        assert!(t.receive(&[IAC, WILL, OPT_ECHO]).reply.is_empty());

        // TTYPE SEND, split across reads at every byte.
        let mut reply = Vec::new();
        for b in [IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE] {
            reply.extend(t.receive(&[b]).reply);
        }
        assert_eq!(reply, [&[IAC, SB, OPT_TTYPE, TTYPE_IS][..], TERMINAL_TYPE, &[IAC, SE]].concat());

        // NAWS update escapes a 255 byte.
        assert_eq!(
            t.resize(255, 50),
            Some(vec![IAC, SB, OPT_NAWS, 0, 255, 255, 0, 50, IAC, SE])
        );
        assert_eq!(t.resize(255, 50), None);

        // Malformed: IAC + garbage inside SB aborts it; data after still flows.
        let r = t.receive(&[IAC, SB, OPT_TTYPE, 1, IAC, b'x', b'o', b'k']);
        assert_eq!(r.data, b"ok");
        assert!(r.reply.is_empty());

        // Oversized SB is capped, not buffered forever.
        let mut big = vec![IAC, SB, 99];
        big.extend(std::iter::repeat_n(7u8, MAX_SB * 4));
        big.extend([IAC, SE, b'!']);
        assert_eq!(t.receive(&big).data, b"!");

        // WONT for an option we never enabled: ignored.
        assert!(t.receive(&[IAC, WONT, OPT_SGA]).reply.is_empty());
    }

    #[test]
    fn encode_escapes_iac_and_cr() {
        let t = Telnet::new(80, 24);
        assert_eq!(t.encode(b"a\r"), b"a\r\0");
        assert_eq!(t.encode(b"a\r\n"), b"a\r\n");
        assert_eq!(t.encode(&[IAC]), [IAC, IAC]);
    }
}
