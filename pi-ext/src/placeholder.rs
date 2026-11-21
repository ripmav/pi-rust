//! Placeholder — replaced by the API modules in WP-1202..WP-1206.

#[cfg(test)]
mod tests {
    #[test]
    fn serde_json_roundtrip() {
        let v = serde_json::json!({"ext": "skeleton"});
        let s = v.to_string();
        let back: serde_json::Value = serde_json::from_str(&s).expect("parse");
        assert_eq!(v, back);
    }
}
