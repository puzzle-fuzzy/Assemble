//! 参考旧 Spider 的 bencode 边界，增加整数、重复键和总节点预算检查。
use crate::Invalid;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bytes(Vec<u8>),
    Int(i64),
    List(Vec<Self>),
    Dict(BTreeMap<Vec<u8>, Self>),
}

impl Value {
    pub fn get(&self, key: &str) -> Result<&Self, Invalid> {
        match self {
            Self::Dict(map) => map.get(key.as_bytes()).ok_or(Invalid),
            _ => Err(Invalid),
        }
    }
    pub fn bytes(&self) -> Result<&[u8], Invalid> {
        match self {
            Self::Bytes(bytes) => Ok(bytes),
            _ => Err(Invalid),
        }
    }
    pub fn int(&self) -> Result<i64, Invalid> {
        match self {
            Self::Int(value) => Ok(*value),
            _ => Err(Invalid),
        }
    }
    pub fn list(&self) -> Result<&[Self], Invalid> {
        match self {
            Self::List(value) => Ok(value),
            _ => Err(Invalid),
        }
    }
}

pub fn dict(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Dict(
        pairs
            .into_iter()
            .map(|(key, value)| (key.as_bytes().to_vec(), value))
            .collect(),
    )
}
pub fn bytes(value: impl AsRef<[u8]>) -> Value {
    Value::Bytes(value.as_ref().to_vec())
}

pub fn decode(data: &[u8]) -> Result<Value, Invalid> {
    let (value, consumed) = prefix(data)?;
    if consumed != data.len() {
        return Err(Invalid);
    }
    Ok(value)
}
pub fn prefix(data: &[u8]) -> Result<(Value, usize), Invalid> {
    let mut budget = 250_000;
    item(data, 0, 0, &mut budget)
}

fn string(data: &[u8], start: usize) -> Result<(Vec<u8>, usize), Invalid> {
    let offset = data
        .get(start..)
        .ok_or(Invalid)?
        .iter()
        .take(22)
        .position(|byte| *byte == b':')
        .ok_or(Invalid)?;
    let raw = std::str::from_utf8(&data[start..start + offset]).map_err(|_| Invalid)?;
    if raw.is_empty()
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
        || (raw.len() > 1 && raw.starts_with('0'))
    {
        return Err(Invalid);
    }
    let size: usize = raw.parse().map_err(|_| Invalid)?;
    let begin = start + offset + 1;
    let end = begin.checked_add(size).ok_or(Invalid)?;
    Ok((data.get(begin..end).ok_or(Invalid)?.to_vec(), end))
}

fn item(
    data: &[u8],
    start: usize,
    depth: usize,
    budget: &mut usize,
) -> Result<(Value, usize), Invalid> {
    *budget = budget.checked_sub(1).ok_or(Invalid)?;
    match data.get(start).ok_or(Invalid)? {
        b'0'..=b'9' => string(data, start).map(|(bytes, end)| (Value::Bytes(bytes), end)),
        b'i' => {
            let offset = data[start + 1..]
                .iter()
                .take(22)
                .position(|byte| *byte == b'e')
                .ok_or(Invalid)?;
            let text =
                std::str::from_utf8(&data[start + 1..start + 1 + offset]).map_err(|_| Invalid)?;
            if text.starts_with('+')
                || text == "-0"
                || (text.len() > 1 && text.starts_with('0'))
                || text.starts_with("-0")
            {
                return Err(Invalid);
            }
            Ok((
                Value::Int(text.parse().map_err(|_| Invalid)?),
                start + offset + 2,
            ))
        }
        b'l' | b'd' if depth < 64 => {
            let dictionary = data[start] == b'd';
            let mut index = start + 1;
            let mut list = Vec::new();
            let mut map = BTreeMap::new();
            loop {
                if *data.get(index).ok_or(Invalid)? == b'e' {
                    break;
                }
                let key = if dictionary {
                    let (key, end) = string(data, index)?;
                    index = end;
                    Some(key)
                } else {
                    None
                };
                let (value, end) = item(data, index, depth + 1, budget)?;
                index = end;
                if let Some(key) = key {
                    if map.insert(key, value).is_some() {
                        return Err(Invalid);
                    }
                } else {
                    list.push(value);
                }
            }
            Ok((
                if dictionary {
                    Value::Dict(map)
                } else {
                    Value::List(list)
                },
                index + 1,
            ))
        }
        _ => Err(Invalid),
    }
}

pub fn encode(value: &Value) -> Vec<u8> {
    fn put(value: &Value, output: &mut Vec<u8>) {
        match value {
            Value::Bytes(bytes) => {
                output.extend_from_slice(bytes.len().to_string().as_bytes());
                output.push(b':');
                output.extend_from_slice(bytes);
            }
            Value::Int(value) => output.extend_from_slice(format!("i{value}e").as_bytes()),
            Value::List(items) => {
                output.push(b'l');
                for item in items {
                    put(item, output);
                }
                output.push(b'e');
            }
            Value::Dict(items) => {
                output.push(b'd');
                for (key, value) in items {
                    put(&Value::Bytes(key.clone()), output);
                    put(value, output);
                }
                output.push(b'e');
            }
        }
    }
    let mut output = Vec::new();
    put(value, &mut output);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_deep_data_are_rejected() {
        for data in [
            b"i01e".as_slice(),
            b"i-0e",
            b"999999999999999999999999999:x",
            b"d1:xi1e1:xi2ee",
            b"i1etrailing",
        ] {
            assert!(decode(data).is_err());
        }
        assert!(decode(&[vec![b'l'; 65], vec![b'e'; 65]].concat()).is_err());
        let value = dict([("x", bytes([0, 255])), ("n", Value::Int(4))]);
        assert_eq!(decode(&encode(&value)).unwrap(), value);
    }
}
