//! Decoding EPT1 records with the opcode table.

use std::collections::HashMap;

use super::DRAW_CALLS;
use super::json::{Json, JsonParser};
use crate::gl::{Arg, IrisCall};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
    Ptr,
}

impl Ty {
    fn parse(s: &str) -> Result<Ty, String> {
        Ok(match s {
            "char" => Ty::I8,
            "unsigned char" => Ty::U8,
            "short" => Ty::I16,
            "unsigned short" => Ty::U16,
            "int" | "long" => Ty::I32,
            "unsigned int" | "unsigned long" => Ty::U32,
            "float" => Ty::F32,
            "double" => Ty::F64,
            s if s.ends_with('*') => Ty::Ptr,
            s => return Err(format!("unknown type {s:?}")),
        })
    }

    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::F64 => 8,
            _ => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Extent {
    Fixed(usize),
    /// The float count is the value of another argument.
    Arg(usize),
}

#[derive(Clone, Debug)]
struct OpDef {
    name: &'static str,
    params: Vec<(Ty, Option<Extent>)>,
    outs: usize,
    ret: Option<Ty>,
}

/// The tracer's opcode table.
#[derive(Clone, Debug)]
pub(crate) struct OpsTable {
    ops: HashMap<u16, OpDef>,
}

impl OpsTable {
    pub(crate) fn parse(json: &str) -> Result<OpsTable, String> {
        let j = JsonParser::parse(json)?;
        let Some(Json::Obj(ops)) = j.get("ops") else {
            return Err("no \"ops\" object".into());
        };
        let mut table = HashMap::new();
        for (code, d) in ops {
            let code: u16 = code.parse().map_err(|_| format!("bad opcode {code:?}"))?;
            let name = d
                .get("name")
                .and_then(Json::as_str)
                .ok_or("op without name")?;
            let Some(Json::Arr(params)) = d.get("params") else {
                return Err(format!("{name}: no params"));
            };
            let mut ps = Vec::new();
            for (i, p) in params.iter().enumerate() {
                let ty = Ty::parse(p.as_str().ok_or("bad param")?)?;
                let ext = match d.get("extents").and_then(|e| e.get(&i.to_string())) {
                    Some(Json::Num(n)) => Some(Extent::Fixed(*n as usize)),
                    Some(Json::Str(s)) => {
                        let k = s
                            .strip_prefix("arg")
                            .and_then(|k| k.parse().ok())
                            .ok_or("bad extent")?;
                        Some(Extent::Arg(k))
                    }
                    _ => None,
                };
                ps.push((ty, ext));
            }
            let outs = match d.get("outs") {
                Some(Json::Arr(a)) => a.len(),
                _ => 0,
            };
            let ret = match d.get("ret").and_then(Json::as_str) {
                None | Some("void") => None,
                Some(t) => Some(Ty::parse(t)?),
            };
            table.insert(
                code,
                OpDef {
                    name: static_name(name),
                    params: ps,
                    outs,
                    ret,
                },
            );
        }
        Ok(OpsTable { ops: table })
    }
}

/// `IrisCall` names are `&'static str`. Names the simulation emits are
/// matched to its own literals; any other name is leaked once (a table
/// has about a hundred).
fn static_name(name: &str) -> &'static str {
    DRAW_CALLS
        .iter()
        .copied()
        .find(|n| *n == name)
        .unwrap_or_else(|| Box::leak(name.to_owned().into_boxed_str()))
}

/// One decoded trace record.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Record {
    pub name: &'static str,
    pub args: Vec<Arg>,
    pub ret: Option<Arg>,
}

impl Record {
    /// As a call log entry (`drand48`'s return value becomes its argument).
    pub(crate) fn to_call(&self) -> IrisCall {
        let mut args = self.args.clone();
        if self.name == "drand48" {
            args.extend(self.ret.clone());
        }
        IrisCall {
            name: self.name,
            args,
        }
    }
}

fn be<const N: usize>(p: &[u8], off: usize) -> Option<[u8; N]> {
    p.get(off..off + N)?.try_into().ok()
}

fn read_scalar(ty: Ty, p: &[u8], off: usize) -> Option<Arg> {
    Some(match ty {
        Ty::I8 => Arg::Int(i64::from(i8::from_be_bytes(be(p, off)?))),
        Ty::U8 => Arg::Int(i64::from(u8::from_be_bytes(be(p, off)?))),
        Ty::I16 => Arg::Int(i64::from(i16::from_be_bytes(be(p, off)?))),
        Ty::U16 => Arg::Int(i64::from(u16::from_be_bytes(be(p, off)?))),
        Ty::I32 => Arg::Int(i64::from(i32::from_be_bytes(be(p, off)?))),
        Ty::U32 => Arg::Int(i64::from(u32::from_be_bytes(be(p, off)?))),
        Ty::F32 => Arg::Float(f32::from_be_bytes(be(p, off)?)),
        Ty::F64 => Arg::Double(f64::from_be_bytes(be(p, off)?)),
        Ty::Ptr => Arg::Ptr,
    })
}

/// A streaming EPT1 decoder.
pub(crate) struct Decoder<'a> {
    ops: &'a OpsTable,
    data: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(ops: &'a OpsTable, data: &'a [u8]) -> Result<Decoder<'a>, String> {
        if !data.starts_with(b"EPT1") {
            return Err("not an EPT1 trace".into());
        }
        Ok(Decoder { ops, data, pos: 4 })
    }

    fn record(&mut self) -> Option<Result<Record, String>> {
        let head = self.data.get(self.pos..self.pos + 4)?;
        let op = u16::from_be_bytes([head[0], head[1]]);
        let len = usize::from(u16::from_be_bytes([head[2], head[3]]));
        let Some(pay) = self.data.get(self.pos + 4..self.pos + 4 + len) else {
            // A truncated final record (the program died mid-write).
            self.pos = self.data.len();
            return None;
        };
        self.pos += 4 + len;
        let Some(d) = self.ops.ops.get(&op) else {
            return Some(Err(format!("unknown opcode {op}")));
        };
        let (mut args, mut off) = (Vec::with_capacity(d.params.len()), 0);
        for &(ty, ext) in &d.params {
            let Some((arg, size)) = read_param(ty, ext, pay, off, &args) else {
                return Some(Err(format!("{}: short payload", d.name)));
            };
            args.push(arg);
            off += size;
        }
        off += 4 * d.outs.min(pay.len().saturating_sub(off) / 4);
        let ret = d
            .ret
            .filter(|_| off < pay.len())
            .and_then(|t| read_scalar(if t == Ty::Ptr { Ty::U32 } else { t }, pay, off));
        Some(Ok(Record {
            name: d.name,
            args,
            ret,
        }))
    }
}

/// One parameter at byte `off` of a payload: the argument and the bytes it
/// takes. `ext` is the extent of a pointer logged by value, which may come from an
/// earlier argument. `None` for a short payload.
fn read_param(
    ty: Ty,
    ext: Option<Extent>,
    pay: &[u8],
    off: usize,
    args: &[Arg],
) -> Option<(Arg, usize)> {
    if let Some(ext) = ext {
        let n = match ext {
            Extent::Fixed(n) => n,
            Extent::Arg(k) => match args.get(k) {
                Some(Arg::Int(v)) => usize::try_from(*v).unwrap_or(0),
                _ => 0,
            },
        };
        let n = if (1..512).contains(&n) { n } else { 0 };
        let v = (0..n)
            .map(|i| be::<4>(pay, off + 4 * i).map(f32::from_be_bytes))
            .collect::<Option<Vec<f32>>>()?;
        Some((Arg::Floats(v), 4 * n))
    } else if ty == Ty::Ptr {
        // Logged by address, which a comparison can't use.
        Some((Arg::Ptr, 4))
    } else {
        Some((read_scalar(ty, pay, off)?, ty.size()))
    }
}

impl Iterator for Decoder<'_> {
    type Item = Result<Record, String>;
    fn next(&mut self) -> Option<Self::Item> {
        self.record()
    }
}
