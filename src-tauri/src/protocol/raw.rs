use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

pub const MAX_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_BATCH_EVENTS: usize = 200;

#[derive(Debug, PartialEq, Eq)]
pub struct RawError {
    pub message: String,
    pub index: Option<usize>,
    pub status: u16,
}

const MAX_NESTING_DEPTH: usize = 64;

struct UniqueValue(usize);
impl<'de> DeserializeSeed<'de> for UniqueValue {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        if self.0 > MAX_NESTING_DEPTH { return Err(de::Error::custom("JSON nesting limit exceeded")); }
        let raw = <Box<serde_json::value::RawValue> as serde::Deserialize>::deserialize(d)?;
        let text = raw.get();
        if text.starts_with('{') || text.starts_with('[') {
            let mut inner = serde_json::Deserializer::from_str(text);
            return de::Deserializer::deserialize_any(&mut inner, ValueVisitor(self.0)).map_err(de::Error::custom);
        }
        serde_json::from_str(text).map_err(de::Error::custom)
    }
}
struct ValueVisitor(usize);
impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("a JSON value") }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value,E> { Ok(Value::Bool(v)) }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value,E> { Ok(Value::from(v)) }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value,E> { Ok(Value::from(v)) }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value,E> { serde_json::Number::from_f64(v).map(Value::Number).ok_or_else(|| E::custom("invalid number")) }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value,E> { Ok(Value::String(v.to_owned())) }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Value,E> { Ok(Value::String(v)) }
    fn visit_none<E: de::Error>(self) -> Result<Value,E> { Ok(Value::Null) }
    fn visit_unit<E: de::Error>(self) -> Result<Value,E> { Ok(Value::Null) }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Value,A::Error> {
        let mut values = Vec::new();
        while let Some(value) = a.next_element_seed(UniqueValue(self.0 + 1))? { values.push(value); }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Value,A::Error> {
        let mut map = Map::new();
        while let Some(key) = a.next_key::<String>()? {
            if map.contains_key(&key) { return Err(de::Error::custom(format!("duplicate key: {key}"))); }
            map.insert(key, a.next_value_seed(UniqueValue(self.0 + 1))?);
        }
        Ok(Value::Object(map))
    }
}
struct IndexedValue<'a> {
    index: &'a mut Option<usize>,
    position: usize,
}
impl<'de> DeserializeSeed<'de> for IndexedValue<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        *self.index = Some(self.position);
        let value = UniqueValue(1).deserialize(d)?;
        *self.index = None;
        Ok(value)
    }
}
struct BatchSeed<'a>(&'a mut Option<usize>);
impl<'de> DeserializeSeed<'de> for BatchSeed<'_> {
    type Value = Vec<Value>;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value,D::Error> { d.deserialize_seq(BatchVisitor(self.0)) }
}
struct BatchVisitor<'a>(&'a mut Option<usize>);
impl<'de> Visitor<'de> for BatchVisitor<'_> {
    type Value = Vec<Value>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("a JSON array") }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value,A::Error> {
        let mut values = Vec::new();
        loop {
            match a.next_element_seed(IndexedValue { index: self.0, position: values.len() })? {
                Some(value) => values.push(value),
                None => break,
            }
        }
        *self.0 = None;
        Ok(values)
    }
}

/// Parse a complete JSON batch while rejecting duplicate object keys at any depth.
pub fn parse_batch(bytes: &[u8]) -> Result<Vec<Value>, RawError> {
    if bytes.len() > MAX_BODY_BYTES { return Err(RawError { message: "request body too large".into(), index: None, status: 413 }); }
    let mut index = None;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let values = BatchSeed(&mut index).deserialize(&mut deserializer).map_err(|e| RawError { message: e.to_string(), index, status: 400 })?;
    deserializer.end().map_err(|e| RawError { message: e.to_string(), index: None, status: 400 })?;
    if values.is_empty() || values.len() > MAX_BATCH_EVENTS { return Err(RawError { message: "batch must contain between 1 and 200 items".into(), index: None, status: 400 }); }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn rejects_duplicates_at_any_depth_and_reports_index() {
        for input in [r#"[{}, {"a":1,"a":2}]"#, r#"[{}, {"a":{"x":1,"\u0078":2}}]"#, r#"[{}, [{"x":1,"x":2}]]"#] {
            let error = parse_batch(input.as_bytes()).unwrap_err();
            assert_eq!((error.status, error.index), (400, Some(1)));
        }
    }
    #[test] fn checks_shape_length_and_trailing_data() {
        for input in ["{}", "[]", "[{}] true", "[{]"] { assert_eq!(parse_batch(input.as_bytes()).unwrap_err().status, 400); }
        let oversized = format!("[{}]", vec!["{}"; 201].join(","));
        assert_eq!(parse_batch(oversized.as_bytes()).unwrap_err().status, 400);
    }
    #[test] fn reports_only_actual_element_errors() {
        for input in ["[{}", "[{} {}]"] {
            assert_eq!(parse_batch(input.as_bytes()).unwrap_err().index, None);
        }
        assert_eq!(parse_batch(br#"[{}, {"x":}]"#).unwrap_err().index, Some(1));
    }
    #[test] fn checks_byte_limit() {
        let mut input = b"[{}]".to_vec(); input.resize(MAX_BODY_BYTES, b' ');
        assert_eq!(parse_batch(&input).unwrap().len(), 1);
        input.push(b' ');
        assert_eq!(parse_batch(&input).unwrap_err().status, 413);
    }
    #[test] fn strict_single_value_checks_limit_and_nested_duplicate_keys() {
        assert_eq!(parse_strict_value(br#"{"a":[{"x":1,"\u0078":2}]}"#, 100).unwrap_err().status, 400);
        assert_eq!(parse_strict_value(br#"{"a":1}"#, 6).unwrap_err().status, 413);
        assert!(parse_strict_value(br#"{"a":1}"#, 7).is_ok());
        assert_eq!(parse_strict_value(b"{} {}", 10).unwrap_err().status, 400);
    }
}




/// Decode one stored value with duplicate-key and nesting checks, independently of HTTP body limits.
pub(crate) fn parse_stored_event(bytes: &[u8]) -> Result<Value, RawError> {
    // Serialization may expand normalized numbers beyond the original request size.
    if bytes.len() > MAX_BODY_BYTES * 2 { return Err(RawError { message:"stored event too large".into(), index:None, status:400 }); }
    parse_strict_value(bytes, MAX_BODY_BYTES * 2)
}

/// Parse one complete value, rejecting repeated keys and respecting a caller-supplied byte limit.
pub fn parse_strict_value(bytes: &[u8], limit: usize) -> Result<Value, RawError> {
    if bytes.len() > limit { return Err(RawError { message:"value too large".into(), index:None, status:413 }); }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue(0).deserialize(&mut deserializer).map_err(|e| RawError { message:e.to_string(), index:None, status:400 })?;
    deserializer.end().map_err(|e| RawError { message:e.to_string(), index:None, status:400 })?;
    Ok(value)
}
