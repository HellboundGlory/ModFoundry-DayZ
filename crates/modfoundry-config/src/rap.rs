//! DayZ `config.bin` — the binarized config format (RaP): reader, writer, and a
//! CfgConvert-compatible text emitter.
//!
//! Layout, verified byte-for-byte against the official `CfgConvert.exe` over the
//! vanilla and workshop corpora:
//!
//! ```text
//! header     := "\0raP" u32(0) u32(8) u32(enum_offset)
//! class body := cstring parent, cint entry_count, entry*, u32 body_end,
//!               followed by the bodies of nested classes, in entry order
//! entry      := 0x00 cstring name u32 body_offset        // class
//!             | 0x01 u8 subtype cstring name value       // scalar
//!             | 0x02 cstring name array                  // array
//!             | 0x03 cstring name                        // `class X;`
//!             | 0x04 cstring name                        // `delete X;`
//!             | 0x05 u32 cstring name array              // `x[] += { .. };`
//! scalar     := cstring (subtype 0x00) | f32 (0x01) | i32 (0x02) | i64 (0x06)
//! array      := cint element_count, element*
//! element    := 0x00 cstring | 0x01 f32 | 0x02 i32 | 0x03 array
//! enum table := u32 count, count * ( cstring name, u32 value )
//! ```
//!
//! `cint` is LEB128 (7 bits per byte, low bits first). All integers are
//! little-endian and all offsets absolute from the start of the file.
//! `body_end` is the offset of the byte after that class's body, nested bodies
//! included; the root body ends where the enum table starts. The `0x05` record
//! carries a literal `1` between its tag and name.
//!
//! Reading is strict: entries, nested-body offsets and body-end markers must
//! agree with the data, so a truncated or corrupt file is reported rather than
//! silently mis-parsed.

use std::collections::HashMap;
use std::fmt::Write as _;

use modfoundry_core::Span;

use crate::ast::{ConfigClass, ConfigFile, ConfigProperty, EnumEntry, Member, Value};

const MAGIC: [u8; 4] = [0x00, b'r', b'a', b'P'];
const FORMAT_VERSION: u32 = 8;
const HEADER_LEN: usize = 16;

/// Nesting allowed for class bodies and for arrays, counted separately. A
/// workshop config.bin read by an index thread must not exhaust its stack.
pub(crate) const MAX_NESTING: usize = 64;

/// Cap on allocating from a count read out of the file: a corrupt count must
/// not ask for gigabytes before the reader finds it cannot deliver the bytes.
const MAX_PREALLOC: u32 = 256;

const TAG_CLASS: u8 = 0x00;
const TAG_VALUE: u8 = 0x01;
const TAG_ARRAY: u8 = 0x02;
const TAG_FORWARD: u8 = 0x03;
const TAG_DELETE: u8 = 0x04;
const TAG_APPEND: u8 = 0x05;

const SUB_STRING: u8 = 0x00;
const SUB_FLOAT: u8 = 0x01;
const SUB_INT: u8 = 0x02;
const SUB_LONG: u8 = 0x06;

const ELEM_STRING: u8 = 0x00;
const ELEM_FLOAT: u8 = 0x01;
const ELEM_INT: u8 = 0x02;
const ELEM_ARRAY: u8 = 0x03;

/// A config.bin read/write failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RapError {
    #[error("not a RaP config.bin (bad `\\0raP` signature)")]
    Signature,
    #[error("config.bin ends early at byte {at}")]
    Truncated { at: usize },
    #[error("unknown {kind} tag {tag:#04x} at byte {at}")]
    UnknownTag {
        kind: &'static str,
        tag: u8,
        at: usize,
    },
    #[error("config.bin layout mismatch at byte {at}: expected offset {expected}")]
    Layout { at: usize, expected: usize },
    #[error("integer at byte {at} is not a valid compressed count")]
    BadCount { at: usize },
    #[error("string at byte {at} is not valid UTF-8")]
    NotUtf8 { at: usize },
    #[error("config.bin nests deeper than 64 levels at byte {at}")]
    TooDeep { at: usize },
    #[error("`{name}` cannot be written to config.bin: {why}")]
    NotBinarizable { name: String, why: &'static str },
    #[error("config.bin offsets are 32-bit; the file would exceed 4 GiB")]
    TooLarge,
}

/// Decode a config.bin into the config AST.
pub fn read_rap(bytes: &[u8]) -> Result<ConfigFile, RapError> {
    if bytes.len() < HEADER_LEN || bytes[..4] != MAGIC {
        return Err(RapError::Signature);
    }
    let enum_offset = u32::from_le_bytes(bytes[12..16].try_into().expect("4 bytes")) as usize;
    if !(HEADER_LEN..=bytes.len()).contains(&enum_offset) {
        return Err(RapError::Layout {
            at: enum_offset,
            expected: HEADER_LEN,
        });
    }

    let mut r = Reader {
        bytes,
        pos: HEADER_LEN,
    };
    let (_, members) = read_members(&mut r, 0)?;
    if r.pos != enum_offset {
        return Err(RapError::Layout {
            at: r.pos,
            expected: enum_offset,
        });
    }
    let enums = read_enums(&mut r)?;
    if r.pos != bytes.len() {
        return Err(RapError::Layout {
            at: r.pos,
            expected: bytes.len(),
        });
    }
    Ok(ConfigFile {
        members,
        enums,
        ..Default::default()
    })
}

/// Encode the AST as config.bin. Rejects AST shapes the format cannot express.
pub fn write_rap(file: &ConfigFile) -> Result<Vec<u8>, RapError> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // enum table offset, patched below
    let mut patches = Vec::new();
    emit_body(&mut out, "", &file.members, &mut patches)?;
    let enum_offset = u32_of(out.len())?;
    out[12..16].copy_from_slice(&enum_offset.to_le_bytes());
    out.extend_from_slice(&u32_of(file.enums.len())?.to_le_bytes());
    for entry in &file.enums {
        push_cstring(&mut out, &entry.name);
        out.extend_from_slice(&entry.value.to_le_bytes());
    }
    Ok(out)
}

/// Render the config as CfgConvert-compatible text (CRLF line endings, one
/// member per line, tab indentation).
///
/// CfgConvert behaviours reproduced here, all measured against the official
/// tool:
///
/// - The enum table has no text form and is dropped.
/// - In strings, `\r` bytes are dropped, `"` is doubled, and every `\n` splits
///   the value into separately quoted parts joined by ` \n `.
/// - Floats print as C `%.8g` of the underlying `f32`.
/// - An array prints inline only when every element is a number.
/// - A class parent is resolved case-insensitively against the names declared
///   before it in the enclosing scope (or in a base class's scope), and is
///   dropped when nothing matches; the resolved declaration's spelling is what
///   gets printed.
pub fn to_text(file: &ConfigFile) -> String {
    let mut out = String::new();
    let mut registry = Registry::new();
    write_members(&mut out, &file.members, 0, &mut registry, 0);
    out
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], RapError> {
        let end = self.pos + n;
        if end > self.bytes.len() {
            return Err(RapError::Truncated { at: self.pos });
        }
        let taken = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(taken)
    }

    fn u8(&mut self) -> Result<u8, RapError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, RapError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn i32(&mut self) -> Result<i32, RapError> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn i64(&mut self) -> Result<i64, RapError> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    fn f32(&mut self) -> Result<f32, RapError> {
        Ok(f32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn cstring(&mut self) -> Result<&'a str, RapError> {
        let at = self.pos;
        let rest = &self.bytes[at..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(RapError::Truncated { at })?;
        self.pos = at + len + 1;
        std::str::from_utf8(&rest[..len]).map_err(|_| RapError::NotUtf8 { at })
    }

    fn cint(&mut self) -> Result<u32, RapError> {
        let at = self.pos;
        let mut value = 0u32;
        let mut shift = 0u32;
        loop {
            let byte = self.u8()?;
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift >= 32 {
                return Err(RapError::BadCount { at });
            }
        }
    }
}

fn read_members(r: &mut Reader<'_>, depth: usize) -> Result<(String, Vec<Member>), RapError> {
    if depth > MAX_NESTING {
        return Err(RapError::TooDeep { at: r.pos });
    }
    let parent = r.cstring()?.to_owned();
    let count = r.cint()?;
    if count as usize > r.remaining() {
        return Err(RapError::Truncated { at: r.pos });
    }

    let mut members: Vec<Member> = Vec::with_capacity(count.min(MAX_PREALLOC) as usize);
    let mut nested: Vec<(usize, usize)> = Vec::new();
    for _ in 0..count {
        let at = r.pos;
        match r.u8()? {
            TAG_CLASS => {
                let name = r.cstring()?.to_owned();
                nested.push((members.len(), r.u32()? as usize));
                members.push(class(name, Vec::new(), false, Vec::new()));
            }
            TAG_FORWARD => {
                let name = r.cstring()?.to_owned();
                members.push(class(name, Vec::new(), true, Vec::new()));
            }
            TAG_DELETE => {
                let name = r.cstring()?.to_owned();
                members.push(Member::Delete {
                    name,
                    span: Span::new(0, 0),
                    statement: Span::new(0, 0),
                });
            }
            TAG_VALUE => {
                let subtype = r.u8()?;
                let name = r.cstring()?.to_owned();
                let value = match subtype {
                    SUB_STRING => Value::Str(r.cstring()?.to_owned()),
                    SUB_FLOAT => Value::Float(r.f32()?),
                    SUB_INT => Value::Number(r.i32()?.to_string()),
                    SUB_LONG => Value::Number(r.i64()?.to_string()),
                    _ => {
                        return Err(RapError::UnknownTag {
                            kind: "scalar",
                            tag: subtype,
                            at,
                        })
                    }
                };
                members.push(property(name, false, false, value));
            }
            tag @ (TAG_ARRAY | TAG_APPEND) => {
                let append = tag == TAG_APPEND;
                if append {
                    r.u32()?;
                }
                let name = r.cstring()?.to_owned();
                members.push(property(name, true, append, read_array(r, 1)?));
            }
            tag => {
                return Err(RapError::UnknownTag {
                    kind: "entry",
                    tag,
                    at,
                })
            }
        }
    }

    let body_end = r.u32()? as usize;
    for (index, offset) in nested {
        if r.pos != offset {
            return Err(RapError::Layout {
                at: r.pos,
                expected: offset,
            });
        }
        let (parent, sub) = read_members(r, depth + 1)?;
        if let Member::Class(c) = &mut members[index] {
            c.parents = parent_of(&parent);
            c.members = sub;
        }
    }
    if r.pos != body_end {
        return Err(RapError::Layout {
            at: r.pos,
            expected: body_end,
        });
    }
    Ok((parent, members))
}

fn read_array(r: &mut Reader<'_>, depth: usize) -> Result<Value, RapError> {
    if depth > MAX_NESTING {
        return Err(RapError::TooDeep { at: r.pos });
    }
    let count = r.cint()?;
    if count as usize > r.remaining() {
        return Err(RapError::Truncated { at: r.pos });
    }
    let mut items = Vec::with_capacity(count.min(MAX_PREALLOC) as usize);
    for _ in 0..count {
        let at = r.pos;
        items.push(match r.u8()? {
            ELEM_STRING => Value::Str(r.cstring()?.to_owned()),
            ELEM_FLOAT => Value::Float(r.f32()?),
            ELEM_INT => Value::Number(r.i32()?.to_string()),
            ELEM_ARRAY => read_array(r, depth + 1)?,
            tag => {
                return Err(RapError::UnknownTag {
                    kind: "array element",
                    tag,
                    at,
                })
            }
        });
    }
    Ok(Value::Array(items))
}

fn read_enums(r: &mut Reader<'_>) -> Result<Vec<EnumEntry>, RapError> {
    let count = r.u32()?;
    if count as usize > r.remaining() {
        return Err(RapError::Truncated { at: r.pos });
    }
    let mut enums = Vec::with_capacity(count.min(MAX_PREALLOC) as usize);
    for _ in 0..count {
        let name = r.cstring()?.to_owned();
        enums.push(EnumEntry {
            name,
            value: r.i32()?,
        });
    }
    Ok(enums)
}

fn class(name: String, parents: Vec<String>, forward: bool, members: Vec<Member>) -> Member {
    Member::Class(ConfigClass {
        name,
        span: Span::new(0, 0),
        spans: Default::default(),
        parents,
        forward,
        members,
    })
}

fn property(name: String, declared_array: bool, array_append: bool, value: Value) -> Member {
    Member::Property(ConfigProperty {
        name,
        span: Span::new(0, 0),
        spans: Default::default(),
        declared_array,
        array_append,
        value: Some(value),
    })
}

fn parent_of(parent: &str) -> Vec<String> {
    if parent.is_empty() {
        Vec::new()
    } else {
        vec![parent.to_owned()]
    }
}

/// Numeric literal classification, matching CfgConvert: an integer that fits
/// `i32` stays an int, one that fits `i64` becomes a long, and anything else
/// (including out-of-range integers, which overflow to `f32`) a float.
enum Number {
    Int(i32),
    Long(i64),
    Float(f32),
}

impl Number {
    fn parse(literal: &str) -> Option<Self> {
        if !literal.contains(['.', 'e', 'E']) {
            if let Ok(v) = literal.parse::<i32>() {
                return Some(Self::Int(v));
            }
            if let Ok(v) = literal.parse::<i64>() {
                return Some(Self::Long(v));
            }
        }
        literal.parse::<f32>().ok().map(Self::Float)
    }

    fn subtype(&self) -> u8 {
        match self {
            Self::Int(_) => SUB_INT,
            Self::Long(_) => SUB_LONG,
            Self::Float(_) => SUB_FLOAT,
        }
    }

    fn push(&self, out: &mut Vec<u8>) {
        match self {
            Self::Int(v) => out.extend_from_slice(&v.to_le_bytes()),
            Self::Long(v) => out.extend_from_slice(&v.to_le_bytes()),
            Self::Float(v) => out.extend_from_slice(&v.to_le_bytes()),
        }
    }
}

fn emit_body(
    out: &mut Vec<u8>,
    parent: &str,
    members: &[Member],
    patches: &mut Vec<usize>,
) -> Result<(), RapError> {
    push_cstring(out, parent);
    push_cint(out, u32_of(members.len())?);
    let open = patches.len();
    for m in members {
        match m {
            Member::Class(c) if c.forward => {
                out.push(TAG_FORWARD);
                push_cstring(out, &c.name);
            }
            Member::Class(c) => {
                out.push(TAG_CLASS);
                push_cstring(out, &c.name);
                patches.push(out.len());
                out.extend_from_slice(&0u32.to_le_bytes());
            }
            Member::Delete { name, .. } => {
                out.push(TAG_DELETE);
                push_cstring(out, name);
            }
            Member::Property(p) => emit_property(out, p)?,
        }
    }

    let body_end = out.len();
    out.extend_from_slice(&0u32.to_le_bytes());
    let mut patched = open;
    for m in members {
        if let Member::Class(c) = m {
            if c.forward {
                continue;
            }
            let offset = u32_of(out.len())?;
            out[patches[patched]..patches[patched] + 4].copy_from_slice(&offset.to_le_bytes());
            patched += 1;
            let parent = single_parent(c)?;
            emit_body(out, parent, &c.members, patches)?;
        }
    }
    let end = u32_of(out.len())?;
    out[body_end..body_end + 4].copy_from_slice(&end.to_le_bytes());
    patches.truncate(open);
    Ok(())
}

fn emit_property(out: &mut Vec<u8>, p: &ConfigProperty) -> Result<(), RapError> {
    let value = p.value.as_ref().ok_or(RapError::NotBinarizable {
        name: p.name.clone(),
        why: "no value",
    })?;
    match value {
        Value::Array(items) if p.array_append => {
            out.push(TAG_APPEND);
            out.extend_from_slice(&1u32.to_le_bytes());
            push_cstring(out, &p.name);
            emit_array(out, &p.name, items)?;
        }
        Value::Array(items) => {
            out.push(TAG_ARRAY);
            push_cstring(out, &p.name);
            emit_array(out, &p.name, items)?;
        }
        _ => {
            if p.array_append {
                return Err(RapError::NotBinarizable {
                    name: p.name.clone(),
                    why: "array append without an array value",
                });
            }
            if p.declared_array {
                return Err(RapError::NotBinarizable {
                    name: p.name.clone(),
                    why: "array declared without an array value",
                });
            }
            out.push(TAG_VALUE);
            match value {
                // A bareword is a string in the binarized form.
                Value::Str(s) | Value::Ident(s) => {
                    out.push(SUB_STRING);
                    push_cstring(out, &p.name);
                    push_cstring(out, s);
                }
                Value::Float(f) => {
                    out.push(SUB_FLOAT);
                    push_cstring(out, &p.name);
                    out.extend_from_slice(&f.to_le_bytes());
                }
                Value::Number(literal) => {
                    let number = Number::parse(literal).ok_or(RapError::NotBinarizable {
                        name: p.name.clone(),
                        why: "number literal cannot be converted",
                    })?;
                    out.push(number.subtype());
                    push_cstring(out, &p.name);
                    number.push(out);
                }
                Value::Array(_) => unreachable!("matched above"),
                Value::Other(_) => {
                    return Err(RapError::NotBinarizable {
                        name: p.name.clone(),
                        why: "value is not a scalar or array literal",
                    })
                }
            }
        }
    }
    Ok(())
}

fn emit_array(out: &mut Vec<u8>, name: &str, items: &[Value]) -> Result<(), RapError> {
    push_cint(out, u32_of(items.len())?);
    for item in items {
        match item {
            Value::Str(s) | Value::Ident(s) => {
                out.push(ELEM_STRING);
                push_cstring(out, s);
            }
            Value::Float(f) => {
                out.push(ELEM_FLOAT);
                out.extend_from_slice(&f.to_le_bytes());
            }
            Value::Number(literal) => {
                let number = Number::parse(literal).ok_or(RapError::NotBinarizable {
                    name: name.to_owned(),
                    why: "number literal cannot be converted",
                })?;
                // Arrays hold no 64-bit integers; CfgConvert narrows them to f32.
                match number {
                    Number::Int(v) => {
                        out.push(ELEM_INT);
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                    Number::Long(v) => {
                        out.push(ELEM_FLOAT);
                        out.extend_from_slice(&(v as f32).to_le_bytes());
                    }
                    Number::Float(v) => {
                        out.push(ELEM_FLOAT);
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
            Value::Array(sub) => {
                out.push(ELEM_ARRAY);
                emit_array(out, name, sub)?;
            }
            Value::Other(_) => {
                return Err(RapError::NotBinarizable {
                    name: name.to_owned(),
                    why: "array element is not a literal",
                })
            }
        }
    }
    Ok(())
}

fn single_parent(c: &ConfigClass) -> Result<&str, RapError> {
    match c.parents.len() {
        0 => Ok(""),
        1 => Ok(&c.parents[0]),
        _ => Err(RapError::NotBinarizable {
            name: c.name.clone(),
            why: "config.bin stores a single parent class",
        }),
    }
}

fn u32_of(value: usize) -> Result<u32, RapError> {
    u32::try_from(value).map_err(|_| RapError::TooLarge)
}

fn push_cstring(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn push_cint(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

#[derive(Default)]
struct Scope<'a> {
    base: Option<u32>,
    enclosing: Option<u32>,
    names: HashMap<String, (&'a str, u32)>,
}

struct Registry<'a> {
    scopes: Vec<Scope<'a>>,
    lower: String,
}

impl<'a> Registry<'a> {
    fn new() -> Self {
        Self {
            scopes: vec![Scope::default()],
            lower: String::new(),
        }
    }

    fn resolve(&self, scope: u32, lower: &str) -> Option<(&'a str, u32)> {
        let mut cursor = Some(scope);
        while let Some(index) = cursor {
            let current = &self.scopes[index as usize];
            if let Some(&hit) = current.names.get(lower) {
                return Some(hit);
            }
            if let Some(base) = current.base {
                if let Some(hit) = self.resolve(base, lower) {
                    return Some(hit);
                }
            }
            cursor = current.enclosing;
        }
        None
    }

    fn lookup(&mut self, scope: u32, name: &str) -> Option<(&'a str, u32)> {
        self.fold(name);
        self.resolve(scope, &self.lower)
    }

    fn open(&mut self, enclosing: u32, base: Option<u32>) -> u32 {
        self.scopes.push(Scope {
            base,
            enclosing: Some(enclosing),
            names: HashMap::new(),
        });
        (self.scopes.len() - 1) as u32
    }

    fn declare(&mut self, scope: u32, name: &'a str, own: u32) {
        self.fold(name);
        let key = std::mem::take(&mut self.lower);
        self.scopes[scope as usize]
            .names
            .entry(key)
            .or_insert((name, own));
    }

    fn fold(&mut self, name: &str) {
        self.lower.clear();
        self.lower.extend(name.chars().flat_map(char::to_lowercase));
    }
}

fn write_members<'a>(
    out: &mut String,
    members: &'a [Member],
    depth: usize,
    registry: &mut Registry<'a>,
    scope: u32,
) {
    for member in members {
        indent(out, depth);
        match member {
            Member::Class(c) => {
                let mut parents = String::new();
                let mut base = None;
                for parent in &c.parents {
                    if let Some((spelling, parent_scope)) = registry.lookup(scope, parent) {
                        parents.push_str(if parents.is_empty() { ": " } else { ", " });
                        parents.push_str(spelling);
                        base = Some(parent_scope);
                    }
                }
                let _ = write!(out, "class {}{}", c.name, parents);
                let own = registry.open(scope, base);
                registry.declare(scope, &c.name, own);
                if c.forward {
                    out.push_str(";\r\n");
                } else {
                    out.push_str("\r\n");
                    indent(out, depth);
                    out.push_str("{\r\n");
                    write_members(out, &c.members, depth + 1, registry, own);
                    indent(out, depth);
                    out.push_str("};\r\n");
                }
            }
            Member::Property(p) => {
                out.push_str(&p.name);
                if p.declared_array {
                    out.push_str("[]");
                }
                if let Some(value) = &p.value {
                    out.push_str(if p.array_append { "+=" } else { "=" });
                    write_value(out, value, depth);
                }
                out.push_str(";\r\n");
            }
            Member::Delete { name, .. } => {
                let _ = write!(out, "delete {name};\r\n");
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
}

fn write_value(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Str(s) => write_string(out, s),
        Value::Number(literal) => out.push_str(literal),
        Value::Float(f) => out.push_str(&format_g8(*f)),
        Value::Ident(s) | Value::Other(s) => out.push_str(s),
        Value::Array(items) => write_array(out, items, depth),
    }
}

fn write_array(out: &mut String, items: &[Value], depth: usize) {
    if items.is_empty() {
        out.push_str("{}");
    } else if numeric(items) {
        out.push('{');
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_value(out, item, depth);
        }
        out.push('}');
    } else {
        out.push_str("\r\n");
        indent(out, depth);
        out.push_str("{\r\n");
        for (i, item) in items.iter().enumerate() {
            indent(out, depth + 1);
            match item {
                Value::Array(sub) if !numeric(sub) => write_array(out, sub, depth + 1),
                _ => write_value(out, item, depth + 1),
            }
            if i + 1 < items.len() {
                out.push(',');
            }
            out.push_str("\r\n");
        }
        indent(out, depth);
        out.push('}');
    }
}

fn numeric(items: &[Value]) -> bool {
    items
        .iter()
        .all(|v| matches!(v, Value::Number(_) | Value::Float(_)))
}

fn write_string(out: &mut String, s: &str) {
    for (i, part) in s.split('\n').enumerate() {
        if i > 0 {
            out.push_str(" \\n ");
        }
        out.push('"');
        for ch in part.chars() {
            match ch {
                '\r' => {}
                '"' => out.push_str("\"\""),
                _ => out.push(ch),
            }
        }
        out.push('"');
    }
}

/// C `%.8g` of an `f32` — the reference is `CfgConvert -txt` output.
fn format_g8(value: f32) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_owned();
    }

    const PRECISION: i32 = 8;
    let scientific = format!("{:.*e}", (PRECISION - 1) as usize, f64::from(value));
    let (negative, scientific) = match scientific.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, scientific.as_str()),
    };
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`LowerExp` adds an exponent");
    let exponent: i32 = exponent.parse().expect("`LowerExp` exponent is an integer");

    let mut out = String::with_capacity(16);
    if negative {
        out.push('-');
    }
    // `mantissa` is `d.ddddddd` — `PRECISION` significant digits, all present.
    let digits = &mantissa[2..];
    if !(-4..PRECISION).contains(&exponent) {
        out.push(mantissa.as_bytes()[0] as char);
        let digits = digits.trim_end_matches('0');
        if !digits.is_empty() {
            out.push('.');
            out.push_str(digits);
        }
        let _ = write!(
            out,
            "e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        );
    } else if exponent >= 0 {
        out.push(mantissa.as_bytes()[0] as char);
        let split = exponent as usize;
        out.push_str(&mantissa[2..2 + split]);
        let digits = digits[split..].trim_end_matches('0');
        if !digits.is_empty() {
            out.push('.');
            out.push_str(digits);
        }
    } else {
        out.push('0');
        out.push('.');
        for _ in 0..(-exponent - 1) {
            out.push('0');
        }
        out.push(mantissa.as_bytes()[0] as char);
        out.push_str(digits.trim_end_matches('0'));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected strings for these bit patterns, produced by `CfgConvert.exe -txt`.
    const FLOAT_CASES: &[(u32, &str)] = &[
        (0x3dcccccd, "0.1"),
        (0x3e4ccccd, "0.2"),
        (0x3e99999a, "0.30000001"),
        (0x3f333333, "0.69999999"),
        (0x3f000000, "0.5"),
        (0x3f800000, "1"),
        (0x40000000, "2"),
        (0x41a00000, "20"),
        (0x3e800000, "0.25"),
        (0x3f400000, "0.75"),
        (0x42c80000, "100"),
        (0x447a0000, "1000"),
        (0x47c35000, "100000"),
        (0x49742400, "1000000"),
        (0x4b189680, "10000000"),
        (0x4cbebc20, "1e+08"),
        (0x4e6e6b28, "1e+09"),
        (0x501502f9, "1e+10"),
        (0x505f8476, "1.5000001e+10"),
        (0x4f800000, "4.2949673e+09"),
        (0x3c23d70a, "0.0099999998"),
        (0x3a83126f, "0.001"),
        (0x38d1b717, "9.9999997e-05"),
        (0x3727c5ac, "9.9999997e-06"),
        (0x322bcc77, "9.9999999e-09"),
        (0x006ce3ee, "9.9999994e-39"),
        (0x5f800000, "1.8446744e+19"),
        (0xcf000000, "-2.1474836e+09"),
        (0xbf000000, "-0.5"),
        (0xbf800000, "-1"),
        (0x7f7fffff, "3.4028235e+38"),
        (0x00800000, "1.1754944e-38"),
        (0x00000001, "1.4012985e-45"),
        (0x42f6e979, "123.456"),
        (0x3a83126e, "0.00099999993"),
        (0x4b3c614e, "12345678"),
        (0x4ceb79a3, "1.2345679e+08"),
        (0x4b18967f, "9999999"),
        (0x4996b438, "1234567"),
        (0x3901725b, "0.00012344999"),
        (0x00000002, "2.8025969e-45"),
        (0x40200000, "2.5"),
        (0x3fc00000, "1.5"),
        (0x00000000, "0"),
        (0x80000000, "-0"),
    ];

    fn forward(name: &str) -> Member {
        class(name.into(), Vec::new(), true, Vec::new())
    }

    fn child(name: &str, parents: &[&str], members: Vec<Member>) -> Member {
        class(
            name.into(),
            parents.iter().map(|p| (*p).to_string()).collect(),
            false,
            members,
        )
    }

    #[test]
    fn float_formatting_matches_cfgconvert() {
        for (bits, expected) in FLOAT_CASES {
            assert_eq!(
                format_g8(f32::from_bits(*bits)),
                *expected,
                "bits {bits:#010x}"
            );
        }
    }

    #[test]
    fn compressed_int_boundaries() {
        let cases: &[(u32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (300, &[0xac, 0x02]),
            (16383, &[0xff, 0x7f]),
            (16384, &[0x80, 0x80, 0x01]),
            (u32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        ];
        for (value, expected) in cases {
            let mut encoded = Vec::new();
            push_cint(&mut encoded, *value);
            assert_eq!(encoded, *expected, "encoding {value}");
            let mut reader = Reader {
                bytes: &encoded,
                pos: 0,
            };
            assert_eq!(reader.cint().expect("decodes"), *value);
        }
    }

    #[test]
    fn compressed_int_rejects_overlong_encoding() {
        let mut reader = Reader {
            bytes: &[0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
            pos: 0,
        };
        assert_eq!(reader.cint().unwrap_err(), RapError::BadCount { at: 0 });
    }

    #[test]
    fn rejects_bad_signature() {
        assert_eq!(read_rap(b"").unwrap_err(), RapError::Signature);
        let mut bytes = b"\0rab".to_vec();
        bytes.extend_from_slice(&[0; 12]);
        assert_eq!(read_rap(&bytes).unwrap_err(), RapError::Signature);
    }

    #[test]
    fn rejects_truncated_body() {
        let mut bytes = header(18);
        bytes[12..16].copy_from_slice(&18u32.to_le_bytes());
        bytes.extend_from_slice(&[0x00, 0x02]); // parent "", claim two entries
        assert_eq!(
            read_rap(&bytes).unwrap_err(),
            RapError::Truncated { at: 18 }
        );
    }

    #[test]
    fn rejects_unknown_entry_tag() {
        let mut body = vec![0x00, 0x01, 0x07, 0x00]; // parent "", one entry, tag 0x07
        body.extend_from_slice(&0u32.to_le_bytes()); // body end
        assert_eq!(
            read_rap(&with_body(body)).unwrap_err(),
            RapError::UnknownTag {
                kind: "entry",
                tag: 0x07,
                at: 18
            }
        );
    }

    #[test]
    fn rejects_mismatched_body_end() {
        let mut body = vec![0x00, 0x00];
        body.extend_from_slice(&99u32.to_le_bytes());
        assert_eq!(
            read_rap(&with_body(body)).unwrap_err(),
            RapError::Layout {
                at: 22,
                expected: 99
            }
        );
    }

    #[test]
    fn rejects_non_utf8_string() {
        let mut body = vec![0x00, 0x01, TAG_VALUE, SUB_STRING];
        body.extend_from_slice(b"x\0\xff\xfe\0");
        body.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            read_rap(&with_body(body)).unwrap_err(),
            RapError::NotUtf8 { at: 22 }
        );
    }

    fn header(enum_offset: u32) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&enum_offset.to_le_bytes());
        bytes
    }

    fn with_body(body: Vec<u8>) -> Vec<u8> {
        let enum_offset = HEADER_LEN + body.len();
        let mut bytes = header(enum_offset as u32);
        bytes.extend_from_slice(&body);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes
    }

    /// `x` declared as an array whose innermost element is the next array, `levels` deep.
    fn nested_array_bin(levels: usize) -> Vec<u8> {
        let mut array = [0x01, ELEM_ARRAY].repeat(levels);
        array.push(0x00); // the innermost array is empty
        let mut body = vec![0x00, 0x01, TAG_ARRAY];
        body.extend_from_slice(b"x\0");
        body.extend_from_slice(&array);
        let body_end = (HEADER_LEN + body.len() + 4) as u32;
        body.extend_from_slice(&body_end.to_le_bytes());
        with_body(body)
    }

    /// A single chain of `class A` bodies with `levels` classes nested inside the root.
    fn nested_class_bin(levels: usize) -> Vec<u8> {
        fn body(out: &mut Vec<u8>, children: usize) {
            out.push(0x00); // parent ""
            out.push(u8::from(children > 0));
            let parent_offset = if children > 0 {
                out.push(TAG_CLASS);
                out.extend_from_slice(b"A\0");
                let at = out.len();
                out.extend_from_slice(&0u32.to_le_bytes());
                Some(at)
            } else {
                None
            };
            let body_end = out.len();
            out.extend_from_slice(&0u32.to_le_bytes());
            if let Some(parent_offset) = parent_offset {
                let child = (HEADER_LEN + out.len()) as u32;
                out[parent_offset..parent_offset + 4].copy_from_slice(&child.to_le_bytes());
                body(out, children - 1);
            }
            let end = (HEADER_LEN + out.len()) as u32;
            out[body_end..body_end + 4].copy_from_slice(&end.to_le_bytes());
        }
        let mut bytes = Vec::new();
        body(&mut bytes, levels);
        with_body(bytes)
    }

    #[test]
    fn a_config_bin_nested_past_the_limit_is_refused() {
        assert!(matches!(
            read_rap(&nested_array_bin(65)),
            Err(RapError::TooDeep { .. })
        ));
        assert!(matches!(
            read_rap(&nested_class_bin(65)),
            Err(RapError::TooDeep { .. })
        ));

        let deep = nested_array_bin(100_000);
        let read = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || read_rap(&deep))
            .expect("spawns")
            .join()
            .expect("the reader must not overflow its stack");
        assert!(matches!(read, Err(RapError::TooDeep { .. })));
    }

    #[test]
    fn empty_file_round_trips() {
        let file = ConfigFile {
            members: vec![],
            ..Default::default()
        };
        let bytes = write_rap(&file).expect("writes");
        assert_eq!(bytes.len(), HEADER_LEN + 1 + 1 + 4 + 4);
        assert_eq!(read_rap(&bytes).expect("reads"), file);
    }

    #[test]
    fn parents_follow_declaration_order() {
        let file = ConfigFile {
            members: vec![
                forward("Base"),
                child("Child", &["bAsE"], vec![]),
                child("Late", &["Later"], vec![]),
                forward("Later"),
                child("Self", &["Self"], vec![]),
            ],
            ..Default::default()
        };
        assert_eq!(
            to_text(&file),
            "class Base;\r\nclass Child: Base\r\n{\r\n};\r\nclass Late\r\n{\r\n};\r\n\
             class Later;\r\nclass Self\r\n{\r\n};\r\n"
        );
    }

    #[test]
    fn parent_resolves_through_inherited_scope() {
        let file = ConfigFile {
            members: vec![
                child("Base", &[], vec![child("Inner", &[], vec![])]),
                child(
                    "Derived",
                    &["Base"],
                    vec![child("Inner", &["Inner"], vec![])],
                ),
            ],
            ..Default::default()
        };
        assert_eq!(
            to_text(&file),
            "class Base\r\n{\r\n\tclass Inner\r\n\t{\r\n\t};\r\n};\r\n\
             class Derived: Base\r\n{\r\n\tclass Inner: Inner\r\n\t{\r\n\t};\r\n};\r\n"
        );
    }

    #[test]
    fn single_parent_rejects_inheritance_lists() {
        let file = ConfigFile {
            members: vec![child("X", &["A", "B"], vec![])],
            ..Default::default()
        };
        assert_eq!(
            write_rap(&file).unwrap_err(),
            RapError::NotBinarizable {
                name: "X".into(),
                why: "config.bin stores a single parent class"
            }
        );
    }

    #[test]
    fn rejects_unbinarizable_values() {
        let cases = [
            ("bad", false, None, "no value"),
            (
                "bad",
                false,
                Some(Value::Other("1 + 1".into())),
                "value is not a scalar or array literal",
            ),
            (
                "bad",
                false,
                Some(Value::Number("twelve".into())),
                "number literal cannot be converted",
            ),
            (
                "bad",
                false,
                Some(Value::Array(vec![Value::Number("twelve".into())])),
                "number literal cannot be converted",
            ),
            (
                "bad",
                false,
                Some(Value::Array(vec![Value::Other("1 + 1".into())])),
                "array element is not a literal",
            ),
            (
                "x",
                true,
                Some(Value::Number("1".into())),
                "array declared without an array value",
            ),
        ];
        for (name, declared_array, value, why) in cases {
            let file = ConfigFile {
                members: vec![Member::Property(ConfigProperty {
                    name: name.into(),
                    span: Span::new(0, 0),
                    spans: Default::default(),
                    declared_array,
                    array_append: false,
                    value,
                })],
                ..Default::default()
            };
            assert_eq!(
                write_rap(&file).unwrap_err(),
                RapError::NotBinarizable {
                    name: name.into(),
                    why
                }
            );
        }
    }

    #[test]
    fn barewords_binarize_as_strings() {
        let file = ConfigFile {
            members: vec![Member::Property(ConfigProperty {
                name: "type".into(),
                span: Span::new(0, 0),
                spans: Default::default(),
                declared_array: false,
                array_append: false,
                value: Some(Value::Ident("mod".into())),
            })],
            ..Default::default()
        };
        let reread = read_rap(&write_rap(&file).expect("writes")).expect("reads");
        assert_eq!(
            reread.members[0],
            Member::Property(ConfigProperty {
                name: "type".into(),
                span: Span::new(0, 0),
                spans: Default::default(),
                declared_array: false,
                array_append: false,
                value: Some(Value::Str("mod".into())),
            })
        );
    }

    #[test]
    fn text_renders_strings_like_cfgconvert() {
        let mut out = String::new();
        write_string(&mut out, "a\"b\r\nc\nd\n");
        assert_eq!(out, "\"a\"\"b\" \\n \"c\" \\n \"d\" \\n \"\"");
    }

    #[test]
    fn text_keeps_numeric_arrays_inline() {
        let mut out = String::new();
        write_array(
            &mut out,
            &[
                Value::Number("1".into()),
                Value::Float(0.5),
                Value::Array(vec![Value::Number("2".into())]),
            ],
            1,
        );
        assert_eq!(out, "\r\n\t{\r\n\t\t1,\r\n\t\t0.5,\r\n\t\t{2}\r\n\t}");
    }
}
