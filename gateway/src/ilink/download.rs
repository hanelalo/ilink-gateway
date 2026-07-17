//! CDN media download and caching.
//!
//! Downloads encrypted media files from WeChat CDN, decrypts them with
//! AES-128-ECB, and saves the result to a local cache directory.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;
use crate::ilink::media::{aes128_ecb_decrypt, build_cdn_download_url, is_valid_cdn_url};

/// Download a media file from CDN, decrypt it, and save to local cache.
///
/// # Arguments
/// * `client` - Shared reqwest HTTP client.
/// * `cdn_base_url` - CDN base URL (e.g. `https://novac2c.cdn.weixin.qq.com`).
/// * `encrypt_query_param` - The encrypted query parameter from the media item.
/// * `aes_key` - 16-byte AES-128 key for decryption.
/// * `cache_dir` - Directory to store decrypted files.
/// * `file_name` - Name for the cached file (e.g. based on MD5).
///
/// Returns the local file path of the decrypted media.
pub async fn download_media(
    client: &reqwest::Client,
    cdn_base_url: &str,
    encrypt_query_param: &str,
    aes_key: &[u8],
    cache_dir: &str,
    file_name: &str,
) -> Result<String> {
    // 1. Build full CDN download URL
    let download_url = build_cdn_download_url(cdn_base_url, encrypt_query_param);

    // 2. Anti-SSRF check
    if !is_valid_cdn_url(&download_url) {
        return Err(crate::error::GatewayError::Ilink(format!(
            "Invalid CDN URL (anti-SSRF blocked): {download_url}"
        )));
    }

    // 3. Download encrypted data
    let resp = client.get(&download_url).send().await?;
    if !resp.status().is_success() {
        return Err(crate::error::GatewayError::Ilink(format!(
            "CDN download returned HTTP {}",
            resp.status()
        )));
    }
    let encrypted_data = resp.bytes().await?;

    // 4. AES-128-ECB decrypt
    let decrypted = aes128_ecb_decrypt(aes_key, &encrypted_data)?;

    // 5. Save to cache_dir/file_name
    let cache_path = PathBuf::from(cache_dir);
    std::fs::create_dir_all(&cache_path)?;

    let file_path = cache_path.join(file_name);
    std::fs::write(&file_path, &decrypted)?;

    // 6. Return local path as string
    Ok(file_path.to_string_lossy().to_string())
}

/// Current timestamp in milliseconds since UNIX epoch.
pub(crate) fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_download_media_rejects_non_wechat_cdn() {
        let mut server = mockito::Server::new_async().await;

        let _mock = server
            .mock("GET", "/c2c")
            .with_status(200)
            .with_body(b"encrypted")
            .create();

        let client = reqwest::Client::new();
        let tmp_dir = tempfile::TempDir::new().unwrap();

        // mockito URL is not a WeChat CDN domain => SSRF rejection
        let result = download_media(
            &client,
            &server.url(),
            "param=value",
            b"0123456789abcdef",
            tmp_dir.path().to_str().unwrap(),
            "test.bin",
        )
        .await;

        assert!(result.is_err(), "non-WeChat CDN should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("anti-SSRF"),
            "error should mention anti-SSRF: {err}"
        );
    }

    #[tokio::test]
    async fn test_download_media_rejects_http_not_https() {
        let client = reqwest::Client::new();
        let tmp_dir = tempfile::TempDir::new().unwrap();

        let result = download_media(
            &client,
            "http://novac2c.cdn.weixin.qq.com",
            "param=val",
            b"0123456789abcdef",
            tmp_dir.path().to_str().unwrap(),
            "test.bin",
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("anti-SSRF"));
    }

    #[test]
    fn test_now_millis_returns_positive() {
        let ts = now_millis();
        assert!(ts > 0, "timestamp should be positive: {ts}");
    }

    #[test]
    fn test_now_millis_increases() {
        let ts1 = now_millis();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let ts2 = now_millis();
        assert!(ts2 > ts1, "second timestamp should be larger");
    }

    #[test]
    fn test_build_cdn_url_passes_ssrf_check_for_valid_domains() {
        let url = build_cdn_download_url("https://novac2c.cdn.weixin.qq.com", "param=abc");
        assert!(
            is_valid_cdn_url(&url),
            "WeChat CDN URL should pass SSRF check: {url}"
        );
    }

    #[test]
    fn test_build_cdn_url_fails_ssrf_check_for_malicious_domain() {
        let url = build_cdn_download_url("https://evil.com", "param=abc");
        assert!(!is_valid_cdn_url(&url), "malicious URL should fail SSRF check");
    }
}
