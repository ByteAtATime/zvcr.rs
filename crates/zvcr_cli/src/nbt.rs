use fastnbt::Value;

pub fn payload_from_bytes(bytes: &[u8]) -> Result<Value, String> {
    if bytes.is_empty() || bytes[0] == 0x00 {
        return Ok(Value::Compound(std::collections::HashMap::new()));
    }
    let mut buffer = Vec::with_capacity(bytes.len() + 2);
    buffer.push(bytes[0]);
    buffer.extend_from_slice(&[0u8, 0u8]);
    buffer.extend_from_slice(&bytes[1..]);
    fastnbt::from_bytes(&buffer).map_err(|e| e.to_string())
}

#[allow(dead_code)]
pub fn payload_to_bytes(value: &Value) -> Result<Vec<u8>, String> {
    let mut buffer = fastnbt::to_bytes(value).map_err(|e| e.to_string())?;
    if buffer.len() < 3 {
        return Err("serialized nbt too short".to_string());
    }
    buffer.drain(1..3);
    Ok(buffer)
}

pub fn root_compound_to_bytes(value: &Value) -> Result<Vec<u8>, String> {
    fastnbt::to_bytes(value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastnbt::{ByteArray, LongArray};
    use std::collections::HashMap;

    fn sample_value() -> Value {
        let mut inner = HashMap::new();
        inner.insert(
            "nested_list".to_string(),
            Value::List(vec![Value::Int(7), Value::Int(8)]),
        );
        inner.insert(
            "nested_long_array".to_string(),
            Value::LongArray(LongArray::new(vec![100i64, 200, 300])),
        );
        let mut root = HashMap::new();
        root.insert(
            "byte_array".to_string(),
            Value::ByteArray(ByteArray::new(vec![1i8, 2, 3])),
        );
        root.insert("int".to_string(), Value::Int(42));
        root.insert("string".to_string(), Value::String("hello".to_string()));
        root.insert("nested".to_string(), Value::Compound(inner));
        Value::Compound(root)
    }

    #[test]
    fn test_payload_roundtrip() {
        let value = sample_value();
        let bytes = payload_to_bytes(&value).expect("to_bytes");
        assert_eq!(bytes[0], 0x0a);
        let back = payload_from_bytes(&bytes).expect("from_bytes");
        assert_eq!(value, back);
    }

    #[test]
    fn test_payload_layout_type_byte_then_compound() {
        let value = sample_value();
        let bytes = payload_to_bytes(&value).unwrap();
        let back = payload_from_bytes(&bytes).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn test_payload_empty_roundtrip() {
        let mut root = HashMap::new();
        root.insert("only".to_string(), Value::Byte(1));
        let value = Value::Compound(root);
        let bytes = payload_to_bytes(&value).unwrap();
        let back = payload_from_bytes(&bytes).unwrap();
        assert_eq!(value, back);
    }
}
