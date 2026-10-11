//! A deliberately tiny D-Bus client: just enough of the wire protocol to
//! call methods with string arguments and decode replies and signals.
//! Replaces the long-running `playerctl` + `jq` processes.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

/// Decoded D-Bus value. Numeric payloads are decoded to keep the reader in
/// sync, but this program only inspects strings and containers.
#[allow(dead_code)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Double(f64),
    Str(String),
    Array(Vec<Value>),
    /// Struct or dict entry.
    Struct(Vec<Value>),
    Variant(Box<Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Variant(v) => v.as_str(),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            Value::Variant(v) => v.as_array(),
            _ => None,
        }
    }
    /// Iterates `a{sv}` as (key, value) pairs.
    pub fn dict(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.as_array().unwrap_or(&[]).iter().filter_map(|e| match e {
            Value::Struct(kv) if kv.len() == 2 => Some((kv[0].as_str()?, &kv[1])),
            _ => None,
        })
    }
}

pub const METHOD_RETURN: u8 = 2;
pub const ERROR: u8 = 3;
pub const SIGNAL: u8 = 4;

#[derive(Default)]
pub struct Message {
    pub kind: u8,
    pub reply_serial: u32,
    pub sender: String,
    pub path: String,
    pub interface: String,
    pub member: String,
    pub body: Vec<Value>,
}

pub struct Conn {
    sock: UnixStream,
    serial: u32,
    buf: Vec<u8>,
}

impl Conn {
    /// Connects to the session bus and authenticates as the current uid.
    /// Does not wait for the Hello reply; it arrives through `read_messages`.
    pub fn session(uid: u32) -> io::Result<Conn> {
        let path = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .ok()
            .and_then(|a| {
                a.split(';')
                    .find_map(|t| t.strip_prefix("unix:path=").map(|p| p.split(',').next().unwrap_or(p).to_owned()))
            })
            .or_else(|| std::env::var("XDG_RUNTIME_DIR").ok().map(|d| format!("{d}/bus")))
            .ok_or_else(|| io::Error::other("no session bus address"))?;
        let mut sock = UnixStream::connect(path)?;

        let hex_uid: String = uid.to_string().bytes().map(|b| format!("{b:02x}")).collect();
        sock.write_all(format!("\0AUTH EXTERNAL {hex_uid}\r\n").as_bytes())?;
        let mut line = Vec::new();
        let mut b = [0u8; 1];
        while !line.ends_with(b"\r\n") {
            if sock.read(&mut b)? == 0 {
                return Err(io::Error::other("bus closed during auth"));
            }
            line.push(b[0]);
        }
        if !line.starts_with(b"OK ") {
            return Err(io::Error::other("bus rejected EXTERNAL auth"));
        }
        sock.write_all(b"BEGIN\r\n")?;

        let mut c = Conn { sock, serial: 0, buf: Vec::with_capacity(4096) };
        c.call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "Hello", &[])?;
        Ok(c)
    }

    pub fn fd(&self) -> i32 {
        use std::os::fd::AsRawFd;
        self.sock.as_raw_fd()
    }

    /// Sends a method call whose arguments are all strings. Returns its serial.
    pub fn call(&mut self, dest: &str, path: &str, iface: &str, member: &str, args: &[&str]) -> io::Result<u32> {
        self.serial += 1;
        let mut w = Writer(Vec::with_capacity(256));
        // Fixed header: little endian, METHOD_CALL, no flags, version 1.
        w.0.extend_from_slice(&[b'l', 1, 0, 1]);
        w.u32(0); // body length, patched below
        w.u32(self.serial);

        let sig = "s".repeat(args.len());
        // Header field array; offsets must be computed relative to the
        // message start, so build it in place.
        w.u32(0); // array length, patched below
        w.pad(8);
        let start = w.0.len();
        for (code, ty, val) in [(1u8, b'o', path), (2, b's', iface), (3, b's', member), (6, b's', dest)] {
            w.pad(8);
            w.0.extend_from_slice(&[code, 1, ty, 0]);
            w.str(val);
        }
        if !args.is_empty() {
            w.pad(8);
            w.0.extend_from_slice(&[8, 1, b'g', 0]);
            w.0.push(sig.len() as u8);
            w.0.extend_from_slice(sig.as_bytes());
            w.0.push(0);
        }
        let len = (w.0.len() - start) as u32;
        w.0[12..16].copy_from_slice(&len.to_le_bytes());
        w.pad(8);

        let body_start = w.0.len();
        for a in args {
            w.str(a);
        }
        let body_len = (w.0.len() - body_start) as u32;
        w.0[4..8].copy_from_slice(&body_len.to_le_bytes());

        self.sock.write_all(&w.0)?;
        Ok(self.serial)
    }

    /// Reads whatever is available (call only when the fd is readable) and
    /// hands every complete message to `f`. Returns false on EOF.
    pub fn read_messages(&mut self, mut f: impl FnMut(Message)) -> io::Result<bool> {
        let mut tmp = [0u8; 8192];
        let n = self.sock.read(&mut tmp)?;
        if n == 0 {
            return Ok(false);
        }
        self.buf.extend_from_slice(&tmp[..n]);
        while let Some(total) = message_len(&self.buf) {
            if self.buf.len() < total {
                break;
            }
            if let Some(m) = parse(&self.buf[..total]) {
                f(m);
            }
            self.buf.drain(..total);
        }
        // Do not let a burst keep a large allocation alive forever.
        if self.buf.is_empty() && self.buf.capacity() > 64 * 1024 {
            self.buf = Vec::with_capacity(4096);
        }
        Ok(true)
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn pad(&mut self, a: usize) {
        while !self.0.len().is_multiple_of(a) {
            self.0.push(0);
        }
    }
    fn u32(&mut self, v: u32) {
        self.pad(4);
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
        self.0.push(0);
    }
}

fn rd_u32(b: &[u8], little: bool) -> u32 {
    let a = [b[0], b[1], b[2], b[3]];
    if little { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) }
}

fn message_len(b: &[u8]) -> Option<usize> {
    if b.len() < 16 {
        return None;
    }
    let little = b[0] == b'l';
    let body = rd_u32(&b[4..], little) as usize;
    let fields = rd_u32(&b[12..], little) as usize;
    let hdr = (16 + fields).next_multiple_of(8);
    Some(hdr + body)
}

fn parse(b: &[u8]) -> Option<Message> {
    let little = b[0] == b'l';
    let mut r = Reader { b, pos: 12, little };
    let mut m = Message { kind: b[1], ..Default::default() };
    let mut sig = String::new();
    if let Value::Array(fields) = r.value(b"a(yv)")?.0 {
        for f in fields {
            let Value::Struct(kv) = f else { continue };
            let (Value::Uint(code), Value::Variant(v)) = (&kv[0], &kv[1]) else { continue };
            match (code, &**v) {
                (1, Value::Str(s)) => m.path = s.clone(),
                (2, Value::Str(s)) => m.interface = s.clone(),
                (3, Value::Str(s)) => m.member = s.clone(),
                (5, Value::Uint(s)) => m.reply_serial = *s as u32,
                (7, Value::Str(s)) => m.sender = s.clone(),
                (8, Value::Str(s)) => sig = s.clone(),
                _ => {}
            }
        }
    }
    r.align(8);
    let mut s = sig.as_bytes();
    while !s.is_empty() {
        let (v, rest) = r.value(s)?;
        m.body.push(v);
        s = rest;
    }
    Some(m)
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
    little: bool,
}

impl<'a> Reader<'a> {
    fn align(&mut self, a: usize) {
        self.pos = self.pos.next_multiple_of(a);
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.align(N);
        let mut a: [u8; N] = self.take(N)?.try_into().ok()?;
        if !self.little {
            a.reverse();
        }
        Some(a)
    }
    fn u32(&mut self) -> Option<u32> {
        self.fixed::<4>().map(u32::from_le_bytes)
    }
    fn string(&mut self, len: usize) -> Option<String> {
        let s = self.take(len + 1)?;
        Some(String::from_utf8_lossy(&s[..len]).into_owned())
    }

    /// Decodes one complete type from the front of `sig`; returns the value
    /// and the remaining signature.
    fn value<'s>(&mut self, sig: &'s [u8]) -> Option<(Value, &'s [u8])> {
        let (&t, rest) = sig.split_first()?;
        let v = match t {
            b'y' => Value::Uint(self.take(1)?[0] as u64),
            b'b' => Value::Bool(self.u32()? != 0),
            b'n' => Value::Int(i16::from_le_bytes(self.fixed()?) as i64),
            b'q' => Value::Uint(u16::from_le_bytes(self.fixed()?) as u64),
            b'i' => Value::Int(i32::from_le_bytes(self.fixed()?) as i64),
            b'u' | b'h' => Value::Uint(self.u32()? as u64),
            b'x' => Value::Int(i64::from_le_bytes(self.fixed()?)),
            b't' => Value::Uint(u64::from_le_bytes(self.fixed()?)),
            b'd' => Value::Double(f64::from_le_bytes(self.fixed()?)),
            b's' | b'o' => {
                let n = self.u32()? as usize;
                Value::Str(self.string(n)?)
            }
            b'g' => {
                let n = self.take(1)?[0] as usize;
                Value::Str(self.string(n)?)
            }
            b'v' => {
                let n = self.take(1)?[0] as usize;
                let inner = self.take(n + 1)?[..n].to_vec();
                let (v, _) = self.value(&inner)?;
                Value::Variant(Box::new(v))
            }
            b'a' => {
                let n = self.u32()? as usize;
                let elem_end = complete_type_len(rest)?;
                let elem = &rest[..elem_end];
                self.align(type_align(elem[0]));
                let end = self.pos + n;
                let mut items = Vec::new();
                while self.pos < end {
                    items.push(self.value(elem)?.0);
                }
                return Some((Value::Array(items), &rest[elem_end..]));
            }
            b'(' | b'{' => {
                self.align(8);
                let close = complete_type_len(sig)?;
                let mut inner = &sig[1..close - 1];
                let mut fields = Vec::new();
                while !inner.is_empty() {
                    let (v, r) = self.value(inner)?;
                    fields.push(v);
                    inner = r;
                }
                return Some((Value::Struct(fields), &sig[close..]));
            }
            _ => return None, // unix fds and unknown types are never expected here
        };
        Some((v, rest))
    }
}

fn type_align(t: u8) -> usize {
    match t {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 4,
    }
}

/// Length in bytes of the first complete type in `sig`.
fn complete_type_len(sig: &[u8]) -> Option<usize> {
    match *sig.first()? {
        b'a' => Some(1 + complete_type_len(&sig[1..])?),
        b'(' | b'{' => {
            let mut depth = 0;
            for (i, &c) in sig.iter().enumerate() {
                match c {
                    b'(' | b'{' => depth += 1,
                    b')' | b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        _ => Some(1),
    }
}
