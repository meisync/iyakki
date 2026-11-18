//! Apache Thrift calls over HTTP with the JSON protocol (F-FLOW-14), written here without the Thrift libraries or a generated client.
//!
//! The call is described in the node: the method, and for every argument its field number, type and value. The reply is
//! self-describing in the JSON protocol, so it is turned into JSON without a description of the service. Only the call and reply
//! messages, the data types of the protocol (bool, i8..i64, double, string, binary, struct, list, set, map) and the standard
//! application exception are handled; framing, the binary and compact protocols and multiplexing other than a name prefix are not.

use serde_json::{json, Map, Value};

/// The message header number of the JSON protocol.
pub const JSON_HEADER: i64 = 1;
pub const CALL: i64 = 1;
pub const REPLY: i64 = 2;
pub const EXCEPTION: i64 = 3;
pub const ONEWAY: i64 = 4;

/// Deepest nesting of types or values that is followed.
const MAX_DEPTH: usize = 24;
/// Most elements in a list, set, map or struct.
const MAX_ITEMS: usize = 100_000;

/// A Thrift type as written in a node: `"i32"`, `"string"`, `{"list": "i64"}`, `{"map": ["string", "double"]}`, `{"struct": [fields]}`.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Bool,
    I8,
    I16,
    I32,
    I64,
    Double,
    Str,
    Binary,
    Struct(Vec<Field>),
    List(Box<Ty>),
    Set(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: i16,
    pub name: String,
    pub ty: Ty,
}

impl Ty {
    /// The type name the JSON protocol writes.
    pub fn tag(&self) -> &'static str {
        match self {
            Ty::Bool => "tf",
            Ty::I8 => "i8",
            Ty::I16 => "i16",
            Ty::I32 => "i32",
            Ty::I64 => "i64",
            Ty::Double => "dbl",
            Ty::Str | Ty::Binary => "str",
            Ty::Struct(_) => "rec",
            Ty::List(_) => "lst",
            Ty::Set(_) => "set",
            Ty::Map(..) => "map",
        }
    }
}

/// Read a type description.
pub fn parse_ty(v: &Value, depth: usize) -> Result<Ty, String> {
    if depth > MAX_DEPTH {
        return Err("the type is nested too deeply".into());
    }
    match v {
        Value::String(t) => match t.to_ascii_lowercase().as_str() {
            "bool" => Ok(Ty::Bool),
            "byte" | "i8" => Ok(Ty::I8),
            "i16" => Ok(Ty::I16),
            "i32" => Ok(Ty::I32),
            "i64" => Ok(Ty::I64),
            "double" => Ok(Ty::Double),
            "string" => Ok(Ty::Str),
            "binary" => Ok(Ty::Binary),
            other => Err(format!("`{other}` is not a Thrift type (bool, byte, i16, i32, i64, double, string, binary, or a list, set, map or struct)")),
        },
        Value::Object(o) if o.len() == 1 => {
            let (k, x) = o.iter().next().unwrap();
            match k.as_str() {
                "list" => Ok(Ty::List(Box::new(parse_ty(x, depth + 1)?))),
                "set" => Ok(Ty::Set(Box::new(parse_ty(x, depth + 1)?))),
                "map" => match x.as_array().map(Vec::as_slice) {
                    Some([k, v]) => Ok(Ty::Map(Box::new(parse_ty(k, depth + 1)?), Box::new(parse_ty(v, depth + 1)?))),
                    _ => Err("a map type is {\"map\": [key type, value type]}".into()),
                },
                "struct" => {
                    let fields = x.as_array().ok_or("a struct type is {\"struct\": [fields]}")?;
                    Ok(Ty::Struct(parse_fields(fields, depth + 1)?))
                }
                other => Err(format!("`{other}` is not a type form (list, set, map, struct)")),
            }
        }
        _ => Err("a type is a name such as \"i32\" or one of {\"list\": T}, {\"set\": T}, {\"map\": [K, V]}, {\"struct\": [fields]}".into()),
    }
}

fn parse_fields(fields: &[Value], depth: usize) -> Result<Vec<Field>, String> {
    if fields.len() > 256 {
        return Err("a struct has at most 256 fields here".into());
    }
    let mut out: Vec<Field> = Vec::new();
    for f in fields {
        let id = f.get("id").and_then(Value::as_i64).filter(|i| (1..=32_767).contains(i)).ok_or("every field needs a number `id` from 1 to 32767")? as i16;
        if out.iter().any(|x| x.id == id) {
            return Err(format!("field number {id} is used twice"));
        }
        let ty = parse_ty(f.get("type").ok_or("every field needs a `type`")?, depth)?;
        out.push(Field { id, name: f.get("name").and_then(Value::as_str).unwrap_or("").to_string(), ty });
    }
    Ok(out)
}

/// The arguments of a call: a list of fields, each with its `value`.
pub fn parse_args(v: &Value) -> Result<Vec<(Field, Value)>, String> {
    let list = match v {
        Value::Null => return Ok(Vec::new()),
        Value::Array(a) => a,
        _ => return Err("`args` is a list of {id, type, value}".into()),
    };
    let fields = parse_fields(list, 0)?;
    Ok(fields.into_iter().zip(list.iter()).map(|(f, raw)| (f, raw.get("value").cloned().unwrap_or(Value::Null))).collect())
}

fn json_str(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

/// Base64 without padding, for the tests of [`unb64`] (a binary result stays the text the server sent).
#[cfg(test)]
fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if c.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if c.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn int_in(v: &Value, lo: i64, hi: i64, what: &str) -> Result<i64, String> {
    let n = match v {
        Value::Number(n) => n.as_i64(),
        Value::String(t) => t.trim().parse().ok(),
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    }
    .ok_or_else(|| format!("{what}: {v} is not a whole number"))?;
    if n < lo || n > hi {
        return Err(format!("{what}: {n} does not fit ({lo} to {hi})"));
    }
    Ok(n)
}

/// One value in the JSON protocol (without its type tag).
fn write_value(ty: &Ty, v: &Value, depth: usize, out: &mut String) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("the value is nested too deeply".into());
    }
    match ty {
        Ty::Bool => {
            let b = match v {
                Value::Bool(b) => *b,
                Value::Number(n) => n.as_i64().is_some_and(|x| x != 0),
                Value::String(t) => matches!(t.to_ascii_lowercase().as_str(), "true" | "1" | "yes"),
                _ => return Err(format!("{v} is not true or false")),
            };
            out.push(if b { '1' } else { '0' });
        }
        Ty::I8 => out.push_str(&int_in(v, -128, 127, "byte")?.to_string()),
        Ty::I16 => out.push_str(&int_in(v, -32_768, 32_767, "i16")?.to_string()),
        Ty::I32 => out.push_str(&int_in(v, i32::MIN as i64, i32::MAX as i64, "i32")?.to_string()),
        Ty::I64 => out.push_str(&int_in(v, i64::MIN, i64::MAX, "i64")?.to_string()),
        Ty::Double => {
            let d = match v {
                Value::Number(n) => n.as_f64(),
                Value::String(t) => t.trim().parse().ok(),
                _ => None,
            }
            .ok_or_else(|| format!("{v} is not a number"))?;
            if d.is_nan() {
                out.push_str("\"NaN\"");
            } else if d.is_infinite() {
                out.push_str(if d > 0.0 { "\"Infinity\"" } else { "\"-Infinity\"" });
            } else {
                out.push_str(&serde_json::Number::from_f64(d).map(|n| n.to_string()).unwrap_or_else(|| "0".into()));
            }
        }
        Ty::Str => out.push_str(&json_str(&crate::expr::text(v))),
        Ty::Binary => {
            let t = crate::expr::text(v);
            // a binary argument is given as base64 text
            if unb64(&t).is_none() {
                return Err("a binary value is written as base64 text".into());
            }
            out.push_str(&json_str(t.trim_end_matches('=')));
        }
        Ty::Struct(fields) => {
            let o = v.as_object().ok_or_else(|| format!("{v} is not an object (a struct is given as {{\"field name or number\": value}})"))?;
            out.push('{');
            let mut first = true;
            for f in fields {
                let val = o.get(&f.id.to_string()).or_else(|| (!f.name.is_empty()).then(|| o.get(&f.name)).flatten());
                let Some(val) = val.filter(|x| !x.is_null()) else { continue };
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&format!("\"{}\":{{\"{}\":", f.id, f.ty.tag()));
                write_value(&f.ty, val, depth + 1, out)?;
                out.push('}');
            }
            out.push('}');
        }
        Ty::List(t) | Ty::Set(t) => {
            let a = v.as_array().ok_or_else(|| format!("{v} is not a list"))?;
            if a.len() > MAX_ITEMS {
                return Err("a list is too long".into());
            }
            out.push_str(&format!("[\"{}\",{}", t.tag(), a.len()));
            for x in a {
                out.push(',');
                write_value(t, x, depth + 1, out)?;
            }
            out.push(']');
        }
        Ty::Map(k, t) => {
            let o = v.as_object().ok_or_else(|| format!("{v} is not an object (a map is given as {{key: value}})"))?;
            if o.len() > MAX_ITEMS {
                return Err("a map is too long".into());
            }
            out.push_str(&format!("[\"{}\",\"{}\",{},{{", k.tag(), t.tag(), o.len()));
            let mut first = true;
            for (key, val) in o {
                if !first {
                    out.push(',');
                }
                first = false;
                // the key is always a JSON string; its own form (a number, a flag) is written inside the quotes
                let mut kk = String::new();
                write_value(k, &Value::String(key.clone()), depth + 1, &mut kk)?;
                out.push_str(&if kk.starts_with('"') { kk } else { json_str(&kk) });
                out.push(':');
                write_value(t, val, depth + 1, out)?;
            }
            out.push_str("}]");
        }
    }
    Ok(())
}

/// The body of a call message: `[1,"method",1,seqid,{"1":{"i32":5}}]`. `service` (if any) is put before the method name with a
/// colon, as the multiplexed protocol does.
pub fn encode_call(service: &str, method: &str, seq: i32, oneway: bool, args: &[(Field, Value)]) -> Result<String, String> {
    if method.is_empty() || !method.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("the method name has letters, digits and underscores only".into());
    }
    if !service.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
        return Err("the service name has letters, digits, dots and underscores only".into());
    }
    let name = if service.is_empty() { method.to_string() } else { format!("{service}:{method}") };
    let mut s = format!("[{JSON_HEADER},{},{},{seq},{{", json_str(&name), if oneway { ONEWAY } else { CALL });
    for (i, (f, v)) in args.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("\"{}\":{{\"{}\":", f.id, f.ty.tag()));
        write_value(&f.ty, v, 0, &mut s).map_err(|e| format!("argument {}: {e}", f.id))?;
        s.push('}');
    }
    s.push_str("}]");
    Ok(s)
}

/// What came back.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// The method returned (`result` is `null` for a method that returns nothing); `fields` holds every field of the reply struct.
    Return { result: Value, fields: Map<String, Value> },
    /// The service threw one of its declared exceptions (field 1 and up of the reply struct).
    Declared { field: String, value: Value },
    /// The server's own failure (Thrift's application exception): its message and type number.
    Application { message: String, kind: i64 },
}

fn read_value(tag: &str, v: &Value, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err("the reply is nested too deeply".into());
    }
    match tag {
        "tf" => Ok(json!(v.as_i64().ok_or("a flag is 0 or 1")? != 0)),
        "i8" | "i16" | "i32" | "i64" => Ok(v.clone()),
        "dbl" => match v {
            Value::String(t) => Ok(match t.as_str() {
                "NaN" => json!("NaN"),
                other => json!(other),
            }),
            other => Ok(other.clone()),
        },
        "str" => Ok(v.clone()),
        "rec" => {
            let o = v.as_object().ok_or("a struct is an object")?;
            if o.len() > MAX_ITEMS {
                return Err("a struct is too large".into());
            }
            let mut out = Map::new();
            for (id, typed) in o {
                let t = typed.as_object().filter(|t| t.len() == 1).ok_or("a field is {type: value}")?;
                let (tag, val) = t.iter().next().unwrap();
                out.insert(id.clone(), read_value(tag, val, depth + 1)?);
            }
            Ok(Value::Object(out))
        }
        "lst" | "set" => {
            let a = v.as_array().filter(|a| a.len() >= 2).ok_or("a list is [type, count, items...]")?;
            let t = a[0].as_str().ok_or("a list starts with its element type")?;
            if a.len() > MAX_ITEMS + 2 {
                return Err("a list is too long".into());
            }
            a[2..].iter().map(|x| read_value(t, x, depth + 1)).collect::<Result<Vec<_>, _>>().map(Value::Array)
        }
        "map" => {
            let a = v.as_array().filter(|a| a.len() == 4).ok_or("a map is [key type, value type, count, {...}]")?;
            let vt = a[1].as_str().ok_or("a map names its value type")?;
            let o = a[3].as_object().ok_or("a map holds an object")?;
            let mut out = Map::new();
            for (k, x) in o {
                out.insert(k.clone(), read_value(vt, x, depth + 1)?);
            }
            Ok(Value::Object(out))
        }
        other => Err(format!("`{other}` is not a Thrift type tag")),
    }
}

/// Read the reply message of a call. `method` and `seq` must match what was sent.
pub fn decode_reply(text: &str, method: &str, seq: i32) -> Result<Reply, String> {
    if text.len() > 8 << 20 {
        return Err("the reply is larger than 8 MB".into());
    }
    let v: Value = serde_json::from_str(text.trim()).map_err(|_| "the answer is not Thrift JSON".to_string())?;
    let a = v.as_array().filter(|a| a.len() == 5).ok_or("the answer is not a Thrift message")?;
    if a[0].as_i64() != Some(JSON_HEADER) {
        return Err("the answer is not Thrift JSON version 1".into());
    }
    let kind = a[2].as_i64().ok_or("the answer has no message type")?;
    let name = a[1].as_str().unwrap_or("");
    if kind != EXCEPTION && name.rsplit(':').next() != Some(method) {
        return Err(format!("the server answered for `{name}`, not `{method}`"));
    }
    if a[3].as_i64() != Some(seq as i64) {
        return Err("the answer belongs to another call (sequence number)".into());
    }
    let body = read_value("rec", &a[4], 0)?;
    let fields = body.as_object().cloned().unwrap_or_default();
    match kind {
        REPLY => {
            if let Some((f, val)) = fields.iter().filter(|(k, _)| k.as_str() != "0").min_by_key(|(k, _)| k.parse::<u32>().unwrap_or(u32::MAX)) {
                return Ok(Reply::Declared { field: f.clone(), value: val.clone() });
            }
            Ok(Reply::Return { result: fields.get("0").cloned().unwrap_or(Value::Null), fields })
        }
        EXCEPTION => Ok(Reply::Application { message: fields.get("1").and_then(Value::as_str).unwrap_or("the server failed").to_string(), kind: fields.get("2").and_then(Value::as_i64).unwrap_or(0) }),
        _ => Err("the answer is not a reply".into()),
    }
}

/// Thrift's application exception names, for the message.
pub fn exception_name(kind: i64) -> &'static str {
    match kind {
        1 => "unknown method",
        2 => "invalid message type",
        3 => "wrong method name",
        4 => "bad sequence id",
        5 => "missing result",
        6 => "internal error",
        7 => "protocol error",
        8 => "invalid transform",
        9 => "invalid protocol",
        10 => "unsupported client type",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> Vec<(Field, Value)> {
        parse_args(&v).unwrap()
    }

    #[test]
    fn a_call_is_written_the_way_the_json_protocol_writes_it() {
        let a = args(json!([{"id": 1, "type": "string", "value": "hi \"you\""}, {"id": 2, "type": "i32", "value": 7}, {"id": 3, "type": "bool", "value": true}, {"id": 4, "type": "double", "value": 1.5}, {"id": 5, "type": "i64", "value": "9007199254740993"}]));
        assert_eq!(encode_call("", "ping", 1, false, &a).unwrap(), r#"[1,"ping",1,1,{"1":{"str":"hi \"you\""},"2":{"i32":7},"3":{"tf":1},"4":{"dbl":1.5},"5":{"i64":9007199254740993}}]"#);
        assert_eq!(encode_call("Calc", "add", 3, true, &[]).unwrap(), r#"[1,"Calc:add",4,3,{}]"#);
    }

    #[test]
    fn lists_sets_maps_and_structs_nest() {
        let a = args(json!([
            {"id": 1, "type": {"list": "i16"}, "value": [1, 2, 3]},
            {"id": 2, "type": {"map": ["string", "double"]}, "value": {"a": 0.5, "b": 2}},
            {"id": 3, "type": {"map": ["i32", "string"]}, "value": {"7": "seven"}},
            {"id": 4, "type": {"struct": [{"id": 1, "name": "name", "type": "string"}, {"id": 2, "name": "tags", "type": {"set": "string"}}, {"id": 3, "name": "skip", "type": "i32"}]}, "value": {"name": "Ada", "2": ["x", "y"]}},
            {"id": 5, "type": "binary", "value": "AAEC"}
        ]));
        let t = encode_call("", "m", 1, false, &a).unwrap();
        assert_eq!(t, r#"[1,"m",1,1,{"1":{"lst":["i16",3,1,2,3]},"2":{"map":["str","dbl",2,{"a":0.5,"b":2.0}]},"3":{"map":["i32","str",1,{"7":"seven"}]},"4":{"rec":{"1":{"str":"Ada"},"2":{"set":["str",2,"x","y"]}}},"5":{"str":"AAEC"}}]"#);
        assert!(serde_json::from_str::<Value>(&t).is_ok(), "it is valid JSON");
    }

    #[test]
    fn values_that_do_not_fit_their_type_are_refused_in_plain_words() {
        let one = |ty: Value, v: Value| encode_call("", "m", 1, false, &args(json!([{"id": 1, "type": ty, "value": v}])));
        assert!(one(json!("i8"), json!(200)).unwrap_err().contains("does not fit"));
        assert!(one(json!("i16"), json!(70000)).is_err() && one(json!("i32"), json!(1u64 << 33)).is_err());
        assert!(one(json!("i32"), json!("abc")).unwrap_err().contains("whole number"));
        assert!(one(json!("bool"), json!([1])).is_err());
        assert!(one(json!({"list": "i32"}), json!("x")).unwrap_err().contains("list"));
        assert!(one(json!("binary"), json!("not base64!")).unwrap_err().contains("base64"));
        assert!(one(json!({"struct": []}), json!([])).is_err());
        assert!(encode_call("", "bad name", 1, false, &[]).is_err() && encode_call("a b", "ok", 1, false, &[]).is_err() && encode_call("", "", 1, false, &[]).is_err());
        assert!(parse_args(&json!([{"id": 0, "type": "i32"}])).is_err() && parse_args(&json!([{"id": 1, "type": "i32"}, {"id": 1, "type": "i32"}])).is_err());
        assert!(parse_ty(&json!("float"), 0).unwrap_err().contains("not a Thrift type"));
        assert!(parse_ty(&json!({"map": ["i32"]}), 0).is_err() && parse_ty(&json!({"x": 1}), 0).is_err());
        let mut deep = json!("i32");
        for _ in 0..40 {
            deep = json!({ "list": deep });
        }
        assert!(parse_ty(&deep, 0).unwrap_err().contains("deeply"));
    }

    #[test]
    fn replies_come_back_as_plain_json_without_a_description_of_the_service() {
        let r = decode_reply(r#"[1,"add",2,5,{"0":{"i32":12}}]"#, "add", 5).unwrap();
        assert_eq!(r, Reply::Return { result: json!(12), fields: [("0".to_string(), json!(12))].into_iter().collect() });
        let r = decode_reply(r#"[1,"Calc:list",2,1,{"0":{"lst":["rec",2,{"1":{"str":"a"},"2":{"i32":1}},{"1":{"str":"b"}}]}}]"#, "list", 1).unwrap();
        assert_eq!(r, Reply::Return { result: json!([{"1": "a", "2": 1}, {"1": "b"}]), fields: [("0".to_string(), json!([{"1": "a", "2": 1}, {"1": "b"}]))].into_iter().collect() });
        let r = decode_reply(r#"[1,"m",2,1,{"0":{"map":["str","tf",2,{"x":1,"y":0}]}}]"#, "m", 1).unwrap();
        assert!(matches!(r, Reply::Return { result, .. } if result == json!({"x": true, "y": false})));
        // a method that returns nothing
        assert_eq!(decode_reply(r#"[1,"m",2,1,{}]"#, "m", 1).unwrap(), Reply::Return { result: Value::Null, fields: Map::new() });
    }

    #[test]
    fn exceptions_are_told_apart_and_a_wrong_answer_is_not_believed() {
        let r = decode_reply(r#"[1,"add",3,2,{"1":{"str":"unknown method add"},"2":{"i32":1}}]"#, "add", 2).unwrap();
        assert_eq!(r, Reply::Application { message: "unknown method add".into(), kind: 1 });
        assert_eq!(exception_name(1), "unknown method");
        let r = decode_reply(r#"[1,"div",2,2,{"1":{"rec":{"1":{"str":"division by zero"}}}}]"#, "div", 2).unwrap();
        assert_eq!(r, Reply::Declared { field: "1".into(), value: json!({"1": "division by zero"}) });
        assert!(decode_reply(r#"[1,"other",2,1,{}]"#, "add", 1).unwrap_err().contains("not `add`"));
        assert!(decode_reply(r#"[1,"add",2,9,{}]"#, "add", 1).unwrap_err().contains("another call"));
        assert!(decode_reply(r#"[2,"add",2,1,{}]"#, "add", 1).is_err());
        assert!(decode_reply(r#"[1,"add",1,1,{}]"#, "add", 1).unwrap_err().contains("not a reply"));
        for junk in ["", "{}", "[1]", "<html>", "[1,\"add\",2,1,5]", r#"[1,"add",2,1,{"0":5}]"#, r#"[1,"add",2,1,{"0":{"zzz":1}}]"#, r#"[1,"add",2,1,{"0":{"lst":["i32"]}}]"#] {
            assert!(decode_reply(junk, "add", 1).is_err(), "{junk}");
        }
        let deep = format!("[1,\"add\",2,1,{}{}{}]", r#"{"0":{"rec":"#.repeat(60), "{}", "}}".repeat(60));
        assert!(decode_reply(&deep, "add", 1).unwrap_err().contains("deeply"));
    }

    #[test]
    fn base64_round_trips_without_padding() {
        for n in 0..8usize {
            let data: Vec<u8> = (0..n as u8).collect();
            assert_eq!(unb64(&b64(&data)).unwrap(), data);
        }
        assert_eq!(b64(b"Ma"), "TWE");
        assert!(unb64("a*b").is_none());
    }
}

#[cfg(test)]
#[path = "thrift_fuzz_tests.rs"]
mod fuzz;
