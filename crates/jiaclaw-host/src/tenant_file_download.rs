// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Current human-authorized file capture, separate from model tools and receipts.
use axum::{
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};

pub(super) fn target(index: &str, policy: &str) -> Option<usize> {
    let n: usize = index.parse().ok()?;
    (n < 32
        && n.to_string() == index
        && policy.len() == 64
        && policy
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
    .then_some(n)
}
pub(super) fn empty_request(headers: &HeaderMap) -> bool {
    let lengths: Vec<_> = headers.get_all(header::CONTENT_LENGTH).iter().collect();
    !headers.contains_key(header::TRANSFER_ENCODING)
        && !headers.contains_key(header::RANGE)
        && (lengths.is_empty() || lengths.len() == 1 && lengths[0] == "0")
}
fn one<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}
pub(super) fn valid(headers: &HeaderMap, bytes: &[u8], index: usize, policy: &str) -> bool {
    bytes.len() <= jiaclaw::READ_FILE_MAX_BYTES
        && !bytes.contains(&0)
        && std::str::from_utf8(bytes).is_ok()
        && !headers.contains_key(header::CONTENT_ENCODING)
        && one(headers, "content-type") == Some("application/octet-stream")
        && one(headers, "x-jiaclaw-file-protocol") == Some("1")
        && one(headers, "x-jiaclaw-file-index") == Some(index.to_string().as_str())
        && one(headers, "x-jiaclaw-file-policy") == Some(policy)
        && one(headers, "x-jiaclaw-file-bytes") == Some(bytes.len().to_string().as_str())
        && one(headers, "x-jiaclaw-file-sha256")
            == Some(format!("{:x}", Sha256::digest(bytes)).as_str())
}
pub(super) fn response(bytes: Vec<u8>, index: usize, policy: &str) -> Response {
    let size = bytes.len();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let mut r = bytes.into_response();
    for (name, value) in [
        ("content-type", "application/octet-stream".into()),
        ("cache-control", "no-store".into()),
        (
            "content-disposition",
            "attachment; filename=\"jiaclaw-file.txt\"".into(),
        ),
        ("x-content-type-options", "nosniff".into()),
        ("x-jiaclaw-file-protocol", "1".into()),
        ("x-jiaclaw-file-index", index.to_string()),
        ("x-jiaclaw-file-policy", policy.to_owned()),
        ("x-jiaclaw-file-bytes", size.to_string()),
        ("x-jiaclaw-file-sha256", digest),
    ] {
        r.headers_mut().insert(
            name,
            HeaderValue::from_str(&value).expect("bounded protocol ASCII"),
        );
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_capture_wire_is_whole_bounded_and_headers_are_unambiguous() {
        let policy = "a".repeat(64);
        for data in [
            vec![],
            "\u{feff}完整\r\n".as_bytes().to_vec(),
            vec![b'x'; jiaclaw::READ_FILE_MAX_BYTES],
        ] {
            let response = response(data.clone(), 0, &policy);
            assert!(valid(response.headers(), &data, 0, &policy));
            assert!(!valid(response.headers(), &data, 1, &policy));
            let mut headers = response.headers().clone();
            headers.append(
                "x-jiaclaw-file-policy",
                HeaderValue::from_str(&policy).unwrap(),
            );
            assert!(!valid(&headers, &data, 0, &policy));
        }
        for data in [
            vec![0],
            vec![255],
            vec![b'x'; jiaclaw::READ_FILE_MAX_BYTES + 1],
        ] {
            let response = response(data.clone(), 0, &policy);
            assert!(!valid(response.headers(), &data, 0, &policy));
        }
        assert_eq!(target("31", &policy), Some(31));
        for index in ["32", "00", "+0", "%30", "-1"] {
            assert_eq!(target(index, &policy), None);
        }
        assert!(!jiaclaw_core::HttpConfig::default().tenant_file_download);
    }
}
