//! Telnet: protocol codec (RFC 854/855/1073/1091/858/857) and a TCP session.
//!
//! Telnet is plaintext. Callers must warn the user (see `Protocol::is_plaintext`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;

const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
const TTYPE_SEND: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Iac,
    Negotiate(u8),
    Sb,
    SbIac,
    /// After CR: a following NUL (or LF) belongs to the CR.
    Cr,
}

/// Incremental telnet decoder with option negotiation.
pub struct Codec {
    state: State,
    sb: Vec<u8>,
    cols: u16,
    rows: u16,
    term: String,
    /// Options we have agreed to perform (we sent WILL).
    local: HashSet<u8>,
    /// Options the peer performs (we sent DO).
    remote: HashSet<u8>,
}

/// Result of feeding bytes: terminal data and bytes to send back.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decoded {
    pub data: Vec<u8>,
    pub reply: Vec<u8>,
}

impl Codec {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            state: State::Data,
            sb: Vec::new(),
            cols,
            rows,
            term: "xterm-256color".into(),
            local: HashSet::new(),
            remote: HashSet::new(),
        }
    }

    fn naws(&self) -> Vec<u8> {
        let mut v = vec![IAC, SB, OPT_NAWS];
        for b in self
            .cols
            .to_be_bytes()
            .into_iter()
            .chain(self.rows.to_be_bytes())
        {
            v.push(b);
            if b == IAC {
                v.push(IAC);
            }
        }
        v.extend([IAC, SE]);
        v
    }

    fn negotiate(&mut self, verb: u8, opt: u8, out: &mut Vec<u8>) {
        match verb {
            DO => match opt {
                OPT_NAWS | OPT_TTYPE | OPT_SGA => {
                    if self.local.insert(opt) {
                        out.extend([IAC, WILL, opt]);
                        if opt == OPT_NAWS {
                            out.extend(self.naws());
                        }
                    }
                }
                _ => out.extend([IAC, WONT, opt]),
            },
            DONT if self.local.remove(&opt) => out.extend([IAC, WONT, opt]),
            WILL => match opt {
                OPT_ECHO | OPT_SGA => {
                    if self.remote.insert(opt) {
                        out.extend([IAC, DO, opt]);
                    }
                }
                _ => out.extend([IAC, DONT, opt]),
            },
            WONT if self.remote.remove(&opt) => out.extend([IAC, DONT, opt]),
            _ => {}
        }
    }

    fn subnegotiation(&mut self, out: &mut Vec<u8>) {
        if self.sb.first() == Some(&OPT_TTYPE)
            && self.sb.get(1) == Some(&TTYPE_SEND)
            && self.local.contains(&OPT_TTYPE)
        {
            out.extend([IAC, SB, OPT_TTYPE, TTYPE_IS]);
            out.extend(self.term.as_bytes());
            out.extend([IAC, SE]);
        }
    }

    /// True when the server echoes (so the client must not echo locally).
    pub fn server_echoes(&self) -> bool {
        self.remote.contains(&OPT_ECHO)
    }

    pub fn decode(&mut self, input: &[u8]) -> Decoded {
        let mut d = Decoded::default();
        for &b in input {
            self.state = match (self.state, b) {
                (State::Cr, 0) => State::Data, // CR NUL = bare CR, already emitted
                (State::Cr | State::Data, IAC) => State::Iac,
                (State::Cr | State::Data, b'\r') => {
                    d.data.push(b);
                    State::Cr
                }
                (State::Cr | State::Data, _) => {
                    d.data.push(b);
                    State::Data
                }
                (State::Iac, IAC) => {
                    d.data.push(IAC);
                    State::Data
                }
                (State::Iac, WILL | WONT | DO | DONT) => State::Negotiate(b),
                (State::Iac, SB) => {
                    self.sb.clear();
                    State::Sb
                }
                (State::Iac, _) => State::Data, // NOP, GA, etc.
                (State::Negotiate(verb), opt) => {
                    self.negotiate(verb, opt, &mut d.reply);
                    State::Data
                }
                (State::Sb, IAC) => State::SbIac,
                (State::Sb, _) => {
                    self.sb.push(b);
                    State::Sb
                }
                (State::SbIac, SE) => {
                    self.subnegotiation(&mut d.reply);
                    State::Data
                }
                (State::SbIac, IAC) => {
                    self.sb.push(IAC);
                    State::Sb
                }
                (State::SbIac, _) => State::Data,
            };
        }
        d
    }

    /// Encode keyboard input: escape IAC, and turn a bare CR into CR NUL.
    pub fn encode(&self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            match input[i] {
                IAC => out.extend([IAC, IAC]),
                b'\r' if input.get(i + 1) == Some(&b'\n') => {
                    out.extend(b"\r\n");
                    i += 1;
                }
                b'\r' => out.extend([b'\r', 0]),
                b => out.push(b),
            }
            i += 1;
        }
        out
    }

    /// Window size change; empty unless the server asked for NAWS.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Vec<u8> {
        self.cols = cols;
        self.rows = rows;
        if self.local.contains(&OPT_NAWS) {
            self.naws()
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TelnetEvent {
    Data(Vec<u8>),
    Closed,
}

/// Cloneable handle for sending input and resizing.
#[derive(Clone)]
pub struct TelnetWriter {
    codec: Arc<Mutex<Codec>>,
    half: Arc<tokio::sync::Mutex<OwnedWriteHalf>>,
}

impl TelnetWriter {
    pub async fn write(&self, input: &[u8]) -> std::io::Result<()> {
        let bytes = lock(&self.codec).encode(input);
        self.half.lock().await.write_all(&bytes).await
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> std::io::Result<()> {
        let bytes = lock(&self.codec).resize(cols, rows);
        if bytes.is_empty() {
            return Ok(());
        }
        self.half.lock().await.write_all(&bytes).await
    }
}

pub struct TelnetSession {
    reader: OwnedReadHalf,
    writer: TelnetWriter,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl TelnetSession {
    pub async fn connect(host: &str, port: u16, cols: u16, rows: u16) -> std::io::Result<Self> {
        let (reader, write) = TcpStream::connect((host, port)).await?.into_split();
        Ok(Self {
            reader,
            writer: TelnetWriter {
                codec: Arc::new(Mutex::new(Codec::new(cols, rows))),
                half: Arc::new(tokio::sync::Mutex::new(write)),
            },
        })
    }

    pub fn writer(&self) -> TelnetWriter {
        self.writer.clone()
    }

    /// Next chunk of terminal data (negotiation is handled internally).
    pub async fn next(&mut self) -> TelnetEvent {
        let mut buf = [0u8; 4096];
        loop {
            let n = match self.reader.read(&mut buf).await {
                Ok(0) | Err(_) => return TelnetEvent::Closed,
                Ok(n) => n,
            };
            let d = lock(&self.writer.codec).decode(&buf[..n]);
            if !d.reply.is_empty()
                && self
                    .writer
                    .half
                    .lock()
                    .await
                    .write_all(&d.reply)
                    .await
                    .is_err()
            {
                return TelnetEvent::Closed;
            }
            if !d.data.is_empty() {
                return TelnetEvent::Data(d.data);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_data_and_iac_escape() {
        let mut c = Codec::new(80, 24);
        let d = c.decode(&[b'a', IAC, IAC, b'b']);
        assert_eq!(d.data, [b'a', 255, b'b']);
        assert!(d.reply.is_empty());
        assert_eq!(c.encode(&[b'x', 255]), [b'x', 255, 255]);
    }

    #[test]
    fn cr_nul_and_cr_lf() {
        let mut c = Codec::new(80, 24);
        assert_eq!(c.decode(b"a\r\0b\r\nc").data, b"a\rb\r\nc");
        assert_eq!(c.encode(b"ls\r"), b"ls\r\0");
        assert_eq!(c.encode(b"ls\r\n"), b"ls\r\n");
    }

    #[test]
    fn naws_negotiation_and_resize() {
        let mut c = Codec::new(80, 24);
        let d = c.decode(&[IAC, DO, OPT_NAWS]);
        assert_eq!(
            d.reply,
            [IAC, WILL, OPT_NAWS, IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE]
        );
        // repeated DO must not be answered again (no negotiation loops)
        assert!(c.decode(&[IAC, DO, OPT_NAWS]).reply.is_empty());
        assert_eq!(
            c.resize(255, 24),
            [IAC, SB, OPT_NAWS, 0, IAC, IAC, 0, 24, IAC, SE]
        );
    }

    #[test]
    fn resize_without_naws_is_silent() {
        assert!(Codec::new(80, 24).resize(100, 30).is_empty());
    }

    #[test]
    fn terminal_type() {
        let mut c = Codec::new(80, 24);
        assert_eq!(
            c.decode(&[IAC, DO, OPT_TTYPE]).reply,
            [IAC, WILL, OPT_TTYPE]
        );
        let d = c.decode(&[IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE]);
        let mut want = vec![IAC, SB, OPT_TTYPE, TTYPE_IS];
        want.extend(b"xterm-256color");
        want.extend([IAC, SE]);
        assert_eq!(d.reply, want);
    }

    #[test]
    fn server_echo_and_unknown_options() {
        let mut c = Codec::new(80, 24);
        assert!(!c.server_echoes());
        assert_eq!(c.decode(&[IAC, WILL, OPT_ECHO]).reply, [IAC, DO, OPT_ECHO]);
        assert!(c.server_echoes());
        assert_eq!(c.decode(&[IAC, DO, 99]).reply, [IAC, WONT, 99]);
        assert_eq!(c.decode(&[IAC, WILL, 98]).reply, [IAC, DONT, 98]);
        assert_eq!(
            c.decode(&[IAC, WONT, OPT_ECHO]).reply,
            [IAC, DONT, OPT_ECHO]
        );
        assert!(!c.server_echoes());
    }

    #[test]
    fn split_input_across_calls() {
        let mut c = Codec::new(80, 24);
        let mut data = c.decode(&[b'h', IAC]).data;
        data.extend(c.decode(&[DO]).data);
        let d = c.decode(&[OPT_SGA, b'i']);
        data.extend(&d.data);
        assert_eq!(data, b"hi");
        assert_eq!(d.reply, [IAC, WILL, OPT_SGA]);
    }
}
