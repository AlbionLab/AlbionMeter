//! Photon Protocol18 message decoding (`Protocol18.cs`), into the boxed-value model the observations read.
//!
//! Decoding is zero-copy: strings and byte arrays borrow the payload, and arrays and parameter lists live in a
//! [`Bump`] arena the caller resets between messages, so the steady-state decode path allocates nothing. Messages
//! built by hand (tests, replayed arrivals) own their data through the same `Cow`s.

use crate::cs::Guid;
use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;
use std::borrow::Cow;

/// A decoded value. The variants mirror the .NET types `Protocol18.Decode` produces, because the observations test
/// them (`value is byte or short or int or long`, `value is byte[]`, `value is Array`).
#[derive(Clone, Debug, PartialEq)]
pub enum Value<'a> {
    Null,
    Bool(bool),
    Byte(u8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(Cow<'a, str>),
    /// `byte[]`
    Bytes(Cow<'a, [u8]>),
    /// `bool[]`, kept packed: its count and the bit bytes.
    Bools(usize, Cow<'a, [u8]>),
    /// `object?[]`
    Array(Cow<'a, [Value<'a>]>),
    /// `KeyValuePair<object?, object?>[]`
    Map(Cow<'a, [(Value<'a>, Value<'a>)]>),
    Opaque(u8, Cow<'a, [u8]>),
    /// `OpaqueValue[]`
    Opaques(u8, Cow<'a, [Cow<'a, [u8]>]>),
    /// A nested message (always exactly one element; a slice keeps the enum small).
    Message(Cow<'a, [WireMessage<'a>]>),
}

/// One element of an array-like value, as `Array.GetValue` boxes it.
#[derive(Clone, Copy, Debug)]
pub enum Element<'v, 'a> {
    Value(&'v Value<'a>),
    Byte(u8),
    Other,
}

impl<'a> Value<'a> {
    /// `value switch { byte b => b, short s => s, int i => i, long l => l, _ => null }`
    pub fn integer(&self) -> Option<i64> {
        match *self {
            Value::Byte(v) => Some(i64::from(v)),
            Value::Short(v) => Some(i64::from(v)),
            Value::Int(v) => Some(i64::from(v)),
            Value::Long(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }
    /// `value is byte[] { Length: 16 } bytes ? new Guid(bytes)`
    pub fn as_guid(&self) -> Option<Guid> {
        self.as_bytes().and_then(Guid::from_slice)
    }
    /// The length when the value is a .NET `Array`.
    pub fn array_len(&self) -> Option<usize> {
        match self {
            Value::Bytes(v) => Some(v.len()),
            Value::Bools(count, _) => Some(*count),
            Value::Array(v) => Some(v.len()),
            Value::Map(v) => Some(v.len()),
            Value::Opaques(_, v) => Some(v.len()),
            _ => None,
        }
    }
    pub fn element(&self, index: usize) -> Element<'_, 'a> {
        match self {
            Value::Bytes(v) => Element::Byte(v[index]),
            Value::Array(v) => Element::Value(&v[index]),
            _ => Element::Other,
        }
    }
    /// The elements when the value is an `Array`.
    pub fn elements(&self) -> Option<impl Iterator<Item = Element<'_, 'a>>> {
        let length = self.array_len()?;
        Some((0..length).map(move |index| self.element(index)))
    }
    /// A packed `bool[]`.
    pub fn bools(values: &[bool]) -> Value<'static> {
        let mut packed = vec![0u8; values.len().div_ceil(8)];
        for (i, &v) in values.iter().enumerate() {
            if v {
                packed[i / 8] |= 1 << (i % 8);
            }
        }
        Value::Bools(values.len(), Cow::Owned(packed))
    }
    /// A deep copy that owns all its data.
    pub fn into_owned(&self) -> Value<'static> {
        fn bytes(b: &[u8]) -> Cow<'static, [u8]> {
            Cow::Owned(b.to_vec())
        }
        match self {
            Value::Null => Value::Null,
            Value::Bool(v) => Value::Bool(*v),
            Value::Byte(v) => Value::Byte(*v),
            Value::Short(v) => Value::Short(*v),
            Value::Int(v) => Value::Int(*v),
            Value::Long(v) => Value::Long(*v),
            Value::Float(v) => Value::Float(*v),
            Value::Double(v) => Value::Double(*v),
            Value::String(v) => Value::String(Cow::Owned(v.to_string())),
            Value::Bytes(v) => Value::Bytes(bytes(v)),
            Value::Bools(count, v) => Value::Bools(*count, bytes(v)),
            Value::Array(v) => Value::Array(Cow::Owned(v.iter().map(Value::into_owned).collect())),
            Value::Map(v) => Value::Map(Cow::Owned(v.iter().map(|(k, v)| (k.into_owned(), v.into_owned())).collect())),
            Value::Opaque(kind, v) => Value::Opaque(*kind, bytes(v)),
            Value::Opaques(kind, v) => Value::Opaques(*kind, Cow::Owned(v.iter().map(|b| bytes(b)).collect())),
            Value::Message(v) => Value::Message(Cow::Owned(v.iter().map(WireMessage::into_owned).collect())),
        }
    }
}

impl Element<'_, '_> {
    pub fn integer(self) -> Option<i64> {
        match self {
            Element::Value(value) => value.integer(),
            Element::Byte(byte) => Some(i64::from(byte)),
            Element::Other => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Element::Value(value) => value.as_str(),
            _ => None,
        }
    }
}

impl From<bool> for Value<'static> {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<u8> for Value<'static> {
    fn from(v: u8) -> Self {
        Value::Byte(v)
    }
}
impl From<i16> for Value<'static> {
    fn from(v: i16) -> Self {
        Value::Short(v)
    }
}
impl From<i32> for Value<'static> {
    fn from(v: i32) -> Self {
        Value::Int(v)
    }
}
impl From<i64> for Value<'static> {
    fn from(v: i64) -> Self {
        Value::Long(v)
    }
}
impl From<f32> for Value<'static> {
    fn from(v: f32) -> Self {
        Value::Float(v)
    }
}
impl From<f64> for Value<'static> {
    fn from(v: f64) -> Self {
        Value::Double(v)
    }
}
impl From<&str> for Value<'static> {
    fn from(v: &str) -> Self {
        Value::String(Cow::Owned(v.to_owned()))
    }
}
impl From<String> for Value<'static> {
    fn from(v: String) -> Self {
        Value::String(Cow::Owned(v))
    }
}
impl From<Vec<u8>> for Value<'static> {
    fn from(v: Vec<u8>) -> Self {
        Value::Bytes(Cow::Owned(v))
    }
}
impl From<Guid> for Value<'static> {
    fn from(v: Guid) -> Self {
        Value::Bytes(Cow::Owned(v.to_bytes().to_vec()))
    }
}
impl From<&[i16]> for Value<'static> {
    fn from(v: &[i16]) -> Self {
        Value::Array(Cow::Owned(v.iter().map(|&x| Value::Short(x)).collect()))
    }
}
impl<const N: usize> From<[i16; N]> for Value<'static> {
    fn from(v: [i16; N]) -> Self {
        Value::from(&v[..])
    }
}
impl From<Vec<Value<'static>>> for Value<'static> {
    fn from(v: Vec<Value<'static>>) -> Self {
        Value::Array(Cow::Owned(v))
    }
}

/// A Photon request (2), response (3) or event (4).
#[derive(Clone, Debug, PartialEq)]
pub struct WireMessage<'a> {
    pub kind: u8,
    pub code: u8,
    pub return_code: Option<i16>,
    pub parameters: Cow<'a, [(u8, Value<'a>)]>,
}

impl WireMessage<'static> {
    pub fn new(kind: u8, code: u8, return_code: Option<i16>, parameters: Vec<(u8, Value<'static>)>) -> Self {
        Self { kind, code, return_code, parameters: Cow::Owned(parameters) }
    }
}

impl<'a> WireMessage<'a> {
    /// `Parameters.GetValueOrDefault(key)`, with a stored null read as missing.
    pub fn get(&self, key: u8) -> Option<&Value<'a>> {
        self.parameters.iter().find(|(k, _)| *k == key).map(|(_, v)| v).filter(|v| !matches!(v, Value::Null))
    }
    pub fn contains(&self, key: u8) -> bool {
        self.parameters.iter().any(|(k, _)| *k == key)
    }
    pub fn integer(&self, key: u8) -> Option<i64> {
        self.get(key).and_then(Value::integer)
    }
    /// A deep copy that owns all its data (for messages kept past their packet, like dungeon arrivals).
    pub fn into_owned(&self) -> WireMessage<'static> {
        WireMessage {
            kind: self.kind,
            code: self.code,
            return_code: self.return_code,
            parameters: Cow::Owned(self.parameters.iter().map(|(k, v)| (*k, v.into_owned())).collect()),
        }
    }
}

pub struct WireDecodeResult<'a> {
    pub status: &'static str,
    pub message: Option<WireMessage<'a>>,
    pub detail: Option<&'static str>,
}

enum Failure {
    Malformed(&'static str),
    Unsupported,
}

/// Decodes into an owned message; convenient outside the hot path.
pub fn decode(payload: &[u8]) -> WireDecodeResult<'static> {
    let arena = Bump::new();
    let result = decode_in(payload, &arena);
    WireDecodeResult { status: result.status, message: result.message.map(|m| m.into_owned()), detail: result.detail }
}

/// Decodes borrowing the payload and the arena: nothing is allocated once the arena has grown to the largest message.
pub fn decode_in<'a>(payload: &'a [u8], arena: &'a Bump) -> WireDecodeResult<'a> {
    let fail = |status, detail| WireDecodeResult { status, message: None, detail };
    if payload.len() < 2 {
        return fail("malformedMessage", None);
    }
    if payload[0] != 0xf3 {
        return fail("unsupportedMessageMagic", None);
    }
    if payload[1] & 0x80 != 0 {
        return fail("encryptedMessage", None);
    }
    if !matches!(payload[1], 2..=4) {
        return fail("unsupportedMessageKind", None);
    }
    let mut reader = Reader { remaining: &payload[2..], budget: 65_536, arena };
    match reader.message(payload[1], 0) {
        Ok(_) if !reader.remaining.is_empty() => fail("malformedMessage", Some("Trailing bytes.")),
        Ok(message) => WireDecodeResult { status: "decodedMessage", message: Some(message), detail: None },
        Err(Failure::Unsupported) => fail("unsupportedValueType", None),
        Err(Failure::Malformed(detail)) => fail("malformedMessage", Some(detail)),
    }
}

struct Reader<'a> {
    remaining: &'a [u8],
    budget: i64,
    arena: &'a Bump,
}

type Read<T> = Result<T, Failure>;

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Read<&'a [u8]> {
        if count > self.remaining.len() {
            return Err(Failure::Malformed("Incomplete value."));
        }
        let (value, rest) = self.remaining.split_at(count);
        self.remaining = rest;
        Ok(value)
    }
    fn byte(&mut self) -> Read<u8> {
        Ok(self.take(1)?[0])
    }
    fn short(&mut self) -> Read<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
    fn unsigned(&mut self, bits: u32) -> Read<u64> {
        let mut result = 0u64;
        let mut shift = 0;
        while shift < bits {
            let value = self.byte()?;
            if bits - shift < 7 && u64::from(value & 0x7f) >= 1 << (bits - shift) {
                return Err(Failure::Malformed("Variable integer overflow."));
            }
            result |= u64::from(value & 0x7f) << shift;
            if value & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err(Failure::Malformed("Unterminated variable integer."))
    }
    fn signed(&mut self, bits: u32) -> Read<i64> {
        let value = self.unsigned(bits)?;
        Ok(((value >> 1) as i64) ^ -((value & 1) as i64))
    }
    fn length(&mut self, maximum: u32) -> Read<usize> {
        let count = self.unsigned(32)?;
        if count > u64::from(maximum) {
            return Err(Failure::Malformed("Value exceeds the supported limit."));
        }
        Ok(count as usize)
    }
    fn blob(&mut self) -> Read<&'a [u8]> {
        let length = self.length(1_048_576)?;
        self.take(length)
    }
    fn message(&mut self, kind: u8, depth: u32) -> Read<WireMessage<'a>> {
        if depth > 16 {
            return Err(Failure::Malformed("Nested message limit exceeded."));
        }
        let code = self.byte()?;
        let mut return_code = None;
        if kind == 3 {
            return_code = Some(self.short()? as i16);
            let debug = self.value(0, depth + 1)?;
            if !matches!(debug, Value::Null | Value::String(_)) {
                return Err(Failure::Malformed("Invalid response debug value."));
            }
        }
        let count = self.byte()?;
        let mut parameters = BumpVec::with_capacity_in(usize::from(count), self.arena);
        for _ in 0..count {
            let key = self.byte()?;
            let value = self.value(0, depth + 1)?;
            if parameters.iter().any(|(k, _): &(u8, Value)| *k == key) {
                return Err(Failure::Malformed("Duplicate parameter key."));
            }
            parameters.push((key, value));
        }
        Ok(WireMessage { kind, code, return_code, parameters: Cow::Borrowed(parameters.into_bump_slice()) })
    }
    fn value(&mut self, mut kind: u8, depth: u32) -> Read<Value<'a>> {
        self.budget -= 1;
        if depth > 16 || self.budget < 0 {
            return Err(Failure::Malformed("Value budget exceeded."));
        }
        if kind == 0 {
            kind = self.byte()?;
        }
        if (128..=228).contains(&kind) {
            return Ok(Value::Opaque(kind - 128, Cow::Borrowed(self.blob()?)));
        }
        Ok(match kind {
            2 => Value::Bool(self.byte()? != 0),
            3 => Value::Byte(self.byte()?),
            4 => Value::Short(self.short()? as i16),
            5 => Value::Float(f32::from_le_bytes(self.take(4)?.try_into().unwrap())),
            6 => Value::Double(f64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            7 => Value::String(Cow::Borrowed(std::str::from_utf8(self.blob()?).map_err(|_| Failure::Malformed("Invalid UTF-8."))?)),
            8 => Value::Null,
            9 => Value::Int(self.signed(32)? as i32),
            10 => Value::Long(self.signed(64)?),
            11 => Value::Int(i32::from(self.byte()?)),
            12 => Value::Int(-i32::from(self.byte()?)),
            13 => Value::Int(i32::from(self.short()?)),
            14 => Value::Int(-i32::from(self.short()?)),
            15 => Value::Long(i64::from(self.byte()?)),
            16 => Value::Long(-i64::from(self.byte()?)),
            17 => Value::Long(i64::from(self.short()?)),
            18 => Value::Long(-i64::from(self.short()?)),
            19 => {
                let custom = self.byte()?;
                Value::Opaque(custom, Cow::Borrowed(self.blob()?))
            }
            20 => {
                let key = self.byte()?;
                let value = self.byte()?;
                if value == 20 || value == 64 {
                    return Err(Failure::Unsupported);
                }
                self.map(key, value, depth)?
            }
            21 => self.map(0, 0, depth)?,
            23 | 64 => self.array(0, depth)?,
            24..=26 => {
                let message = self.message(kind - 22, depth + 1)?;
                Value::Message(Cow::Borrowed(std::slice::from_ref(self.arena.alloc(message))))
            }
            27 => Value::Bool(false),
            28 => Value::Bool(true),
            29 => Value::Short(0),
            30 => Value::Int(0),
            31 => Value::Long(0),
            32 => Value::Float(0.0),
            33 => Value::Double(0.0),
            34 => Value::Byte(0),
            66 => {
                let count = self.length(65_536)?;
                Value::Bools(count, Cow::Borrowed(self.take(count.div_ceil(8))?))
            }
            67 => Value::Bytes(Cow::Borrowed(self.blob()?)),
            68 => self.array(4, depth)?,
            69 => self.array(5, depth)?,
            70 => self.array(6, depth)?,
            71 => self.array(7, depth)?,
            73 => self.array(9, depth)?,
            74 => self.array(10, depth)?,
            83 => {
                let total = self.length(65_536)?;
                let custom = self.byte()?;
                let mut values = BumpVec::with_capacity_in(total.min(self.remaining.len()), self.arena);
                for _ in 0..total {
                    values.push(Cow::Borrowed(self.blob()?));
                }
                Value::Opaques(custom, Cow::Borrowed(values.into_bump_slice()))
            }
            85 => self.array(21, depth)?,
            _ => return Err(Failure::Unsupported),
        })
    }
    fn array(&mut self, kind: u8, depth: u32) -> Read<Value<'a>> {
        let count = self.length(65_536)?;
        if count as i64 > self.budget {
            return Err(Failure::Malformed("Array exceeds the value budget."));
        }
        // Every element takes at least one byte, so the remaining bytes bound the capacity an attacker can request.
        let mut values = BumpVec::with_capacity_in(count.min(self.remaining.len()), self.arena);
        for _ in 0..count {
            values.push(self.value(kind, depth + 1)?);
        }
        Ok(Value::Array(Cow::Borrowed(values.into_bump_slice())))
    }
    fn map(&mut self, key: u8, value: u8, depth: u32) -> Read<Value<'a>> {
        let count = self.length(65_536)?;
        if count as i64 > self.budget / 2 {
            return Err(Failure::Malformed("Map exceeds the value budget."));
        }
        let mut values = BumpVec::with_capacity_in(count.min(self.remaining.len()), self.arena);
        for _ in 0..count {
            let k = self.value(key, depth + 1)?;
            let v = self.value(value, depth + 1)?;
            values.push((k, v));
        }
        Ok(Value::Map(Cow::Borrowed(values.into_bump_slice())))
    }
}
