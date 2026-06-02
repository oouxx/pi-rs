use std::collections::HashMap;

/// Convert a reqwest HeaderMap to a plain HashMap of string key-value pairs.
pub fn headers_to_record(headers: &reqwest::header::HeaderMap) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for (key, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            result.insert(key.as_str().to_string(), v.to_string());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    #[test]
    fn test_headers_to_record_empty() {
        let headers = HeaderMap::new();
        let record = headers_to_record(&headers);
        assert!(record.is_empty());
    }

    #[test]
    fn test_headers_to_record_single() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("application/json"),
        );
        let record = headers_to_record(&headers);
        assert_eq!(record.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn test_headers_to_record_multiple() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer test123"),
        );
        headers.insert(
            HeaderName::from_static("x-request-id"),
            HeaderValue::from_static("req_001"),
        );
        let record = headers_to_record(&headers);
        assert_eq!(record.len(), 2);
        assert_eq!(record.get("authorization").unwrap(), "Bearer test123");
        assert_eq!(record.get("x-request-id").unwrap(), "req_001");
    }
}
