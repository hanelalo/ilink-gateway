//! Media utilities: AES-128-ECB encryption/decryption, CDN URL validation,
//! and CDN media upload (encrypt-then-upload) for outgoing media replies.

use crate::error::{GatewayError, Result};
use crate::ilink::client::Client;
use crate::ilink::types::{msg_type, GetUploadUrlRequest};

/// Validates that a CDN URL belongs to an allowed WeChat CDN domain.
#[allow(dead_code)]
pub fn is_valid_cdn_url(url: &str) -> bool {
    // Only allow *.cdn.weixin.qq.com
    url.starts_with("https://novac2c.cdn.weixin.qq.com/")
        || url.starts_with("https://cdn.weixin.qq.com/")
}

/// Decrypt AES-128-ECB encrypted data with PKCS7 padding removal.
#[allow(dead_code)]
pub fn aes128_ecb_decrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    use aes::cipher::{BlockDecrypt, KeyInit};

    let key_bytes: [u8; 16] = key
        .try_into()
        .map_err(|_| GatewayError::Config("AES key must be 16 bytes".to_string()))?;

    let cipher =
        Aes128::new_from_slice(&key_bytes)
            .map_err(|e| GatewayError::Config(format!("AES key init error: {e}")))?;

    let mut blocks: Vec<aes::Block> = data
        .chunks(16)
        .map(|chunk| {
            let mut block = [0u8; 16];
            let len = chunk.len().min(16);
            block[..len].copy_from_slice(&chunk[..len]);
            *aes::Block::from_slice(&block)
        })
        .collect();

    for block in &mut blocks {
        cipher.decrypt_block(block);
    }

    let mut result: Vec<u8> = blocks.iter().flat_map(|b| b.to_vec()).collect();

    // PKCS7 unpad
    if let Some(&pad_len) = result.last() {
        let pad_len = pad_len as usize;
        if pad_len > 0 && pad_len <= 16 {
            let len = result.len() - pad_len;
            result.truncate(len);
        }
    }

    Ok(result)
}

/// Encrypt data with AES-128-ECB with PKCS7 padding.
pub fn aes128_ecb_encrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    use aes::cipher::{BlockEncrypt, KeyInit};

    let key_bytes: [u8; 16] = key
        .try_into()
        .map_err(|_| GatewayError::Config("AES key must be 16 bytes".to_string()))?;

    let cipher =
        Aes128::new_from_slice(&key_bytes)
            .map_err(|e| GatewayError::Config(format!("AES key init error: {e}")))?;

    // PKCS7 padding
    let pad_len = 16 - (data.len() % 16);
    let mut padded = data.to_vec();
    padded.extend(std::iter::repeat(pad_len as u8).take(pad_len));

    let mut blocks: Vec<aes::Block> = padded
        .chunks(16)
        .map(|chunk| *aes::Block::from_slice(chunk))
        .collect();

    for block in &mut blocks {
        cipher.encrypt_block(block);
    }

    Ok(blocks.iter().flat_map(|b| b.to_vec()).collect())
}

/// Build a CDN download URL from CDN base and encrypt_query_param.
#[allow(dead_code)]
pub fn build_cdn_download_url(cdn_base: &str, encrypt_query_param: &str) -> String {
    format!(
        "{}/c2c?{}",
        cdn_base.trim_end_matches('/'),
        encrypt_query_param
    )
}

/// Determine iLink media type from a file path extension.
pub fn media_type_from_extension(path: &str) -> i32 {
    let lower = path.to_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".png")
        || lower.ends_with(".gif") || lower.ends_with(".webp") || lower.ends_with(".bmp")
    {
        msg_type::IMAGE
    } else if lower.ends_with(".silk") || lower.ends_with(".mp3") || lower.ends_with(".wav")
        || lower.ends_with(".ogg") || lower.ends_with(".amr") || lower.ends_with(".aac")
    {
        msg_type::VOICE
    } else if lower.ends_with(".mp4") || lower.ends_with(".mov") || lower.ends_with(".avi")
        || lower.ends_with(".mkv") || lower.ends_with(".webm")
    {
        msg_type::VIDEO
    } else {
        msg_type::FILE
    }
}

/// Process a local file for CDN upload: calculate MD5, get upload URL, encrypt, upload.
///
/// Returns (item_type, encrypt_query_param, aes_key_hex) for use in build_media_reply.
pub async fn process_media_upload(
    ilink_client: &Client,
    token: &str,
    file_path: &str,
) -> Result<(i32, String, String)> {
    // 1. Read file
    let file_data = std::fs::read(file_path)?;
    let file_size = file_data.len() as i64;
    use md5::{Digest, Md5};
    let file_md5 = format!("{:x}", Md5::digest(&file_data));
    let item_type = media_type_from_extension(file_path);

    // 2. Generate random 16-byte AES key as hex string
    use rand::Rng;
    let aes_key_bytes: [u8; 16] = rand::rng().random();
    let aes_key_hex = hex::encode(&aes_key_bytes);

    // 3. Get upload URL from iLink
    let req = GetUploadUrlRequest {
        aes_key: aes_key_hex.clone(),
        item_type,
        file_size,
        file_md5,
        base_info: None,
    };
    let upload_resp = ilink_client.get_upload_url(token, &req).await?;

    let cdn_url = upload_resp.cdnurl.ok_or_else(|| {
        GatewayError::Ilink("getuploadurl returned no cdnurl".to_string())
    })?;
    let resp_aes_key = upload_resp.aes_key.ok_or_else(|| {
        GatewayError::Ilink("getuploadurl returned no aes_key".to_string())
    })?;

    // 4. AES-128-ECB encrypt the file data
    let encrypted = aes128_ecb_encrypt(&aes_key_bytes, &file_data)?;

    // 5. Upload encrypted data to CDN via PUT
    let http_client = reqwest::Client::builder()
        .build()?;
    let upload_resp = http_client
        .put(&cdn_url)
        .header("Content-Type", "application/octet-stream")
        .body(encrypted)
        .send()
        .await?;
    if !upload_resp.status().is_success() {
        return Err(GatewayError::Ilink(format!(
            "CDN upload returned HTTP {}",
            upload_resp.status()
        )));
    }

    // 6. Extract encrypt_query_param from CDN URL
    let encrypt_query_param = cdn_url.split('?').nth(1).unwrap_or(&cdn_url).to_string();

    Ok((item_type, encrypt_query_param, resp_aes_key))
}

use aes::Aes128;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ilink::client::Client as IlinkClient;

    const TEST_KEY: &[u8] = b"0123456789abcdef"; // exactly 16 bytes

    #[test]
    fn test_aes_encrypt_decrypt_roundtrip() {
        let data = b"Hello, WeChat Media!";
        let encrypted = aes128_ecb_encrypt(TEST_KEY, data).unwrap();
        let decrypted = aes128_ecb_decrypt(TEST_KEY, &encrypted).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_aes_encrypt_decrypt_padding_multiple_of_16() {
        // exactly 16 bytes
        let data = b"1234567890abcdef";
        let encrypted = aes128_ecb_encrypt(TEST_KEY, data).unwrap();
        let decrypted = aes128_ecb_decrypt(TEST_KEY, &encrypted).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_aes_encrypt_decrypt_empty_data() {
        let data = b"";
        let encrypted = aes128_ecb_encrypt(TEST_KEY, data).unwrap();
        // PKCS7 padding 16 bytes
        assert_eq!(encrypted.len(), 16);
        let decrypted = aes128_ecb_decrypt(TEST_KEY, &encrypted).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_aes_invalid_key_length() {
        let short_key = b"tooshort";
        let result = aes128_ecb_encrypt(short_key, b"data");
        assert!(result.is_err());
    }

    #[test]
    fn test_is_valid_cdn_url() {
        assert!(is_valid_cdn_url(
            "https://novac2c.cdn.weixin.qq.com/c2c?param=abc"
        ));
        assert!(is_valid_cdn_url("https://cdn.weixin.qq.com/path/to/file"));
        assert!(!is_valid_cdn_url("https://evil.com/c2c?param=abc"));
        assert!(!is_valid_cdn_url(
            "http://novac2c.cdn.weixin.qq.com/c2c"
        )); // not https
    }

    #[test]
    fn test_build_cdn_download_url() {
        let url = build_cdn_download_url(
            "https://novac2c.cdn.weixin.qq.com",
            "encrypted_param",
        );
        assert_eq!(
            url,
            "https://novac2c.cdn.weixin.qq.com/c2c?encrypted_param"
        );
    }

    #[test]
    fn test_build_cdn_download_url_trailing_slash() {
        let url =
            build_cdn_download_url("https://novac2c.cdn.weixin.qq.com/", "param");
        assert_eq!(
            url,
            "https://novac2c.cdn.weixin.qq.com/c2c?param"
        );
    }

    #[test]
    fn test_media_type_from_extension_image() {
        assert_eq!(media_type_from_extension("photo.jpg"), msg_type::IMAGE);
        assert_eq!(media_type_from_extension("photo.jpeg"), msg_type::IMAGE);
        assert_eq!(media_type_from_extension("photo.png"), msg_type::IMAGE);
        assert_eq!(media_type_from_extension("photo.gif"), msg_type::IMAGE);
        assert_eq!(media_type_from_extension("photo.webp"), msg_type::IMAGE);
        assert_eq!(media_type_from_extension("photo.bmp"), msg_type::IMAGE);
    }

    #[test]
    fn test_media_type_from_extension_voice() {
        assert_eq!(media_type_from_extension("audio.silk"), msg_type::VOICE);
        assert_eq!(media_type_from_extension("audio.mp3"), msg_type::VOICE);
        assert_eq!(media_type_from_extension("audio.wav"), msg_type::VOICE);
        assert_eq!(media_type_from_extension("audio.ogg"), msg_type::VOICE);
        assert_eq!(media_type_from_extension("audio.amr"), msg_type::VOICE);
        assert_eq!(media_type_from_extension("audio.aac"), msg_type::VOICE);
    }

    #[test]
    fn test_media_type_from_extension_video() {
        assert_eq!(media_type_from_extension("video.mp4"), msg_type::VIDEO);
        assert_eq!(media_type_from_extension("video.mov"), msg_type::VIDEO);
        assert_eq!(media_type_from_extension("video.avi"), msg_type::VIDEO);
        assert_eq!(media_type_from_extension("video.mkv"), msg_type::VIDEO);
        assert_eq!(media_type_from_extension("video.webm"), msg_type::VIDEO);
    }

    #[test]
    fn test_media_type_from_extension_file() {
        assert_eq!(media_type_from_extension("doc.pdf"), msg_type::FILE);
        assert_eq!(media_type_from_extension("archive.zip"), msg_type::FILE);
        assert_eq!(media_type_from_extension("data.txt"), msg_type::FILE);
        assert_eq!(media_type_from_extension("no_extension"), msg_type::FILE);
    }

    #[test]
    fn test_media_type_from_extension_case_insensitive() {
        assert_eq!(media_type_from_extension("photo.JPG"), msg_type::IMAGE);
        assert_eq!(media_type_from_extension("audio.MP3"), msg_type::VOICE);
        assert_eq!(media_type_from_extension("video.MP4"), msg_type::VIDEO);
    }

    #[tokio::test]
    async fn test_process_media_upload_success() {
        let mut server = mockito::Server::new_async().await;
        let client = IlinkClient::new(Some(server.url())).unwrap();

        // Create a temp file to use as "media"
        let tmp_dir = tempfile::TempDir::new().unwrap();
        let media_path = tmp_dir.path().join("test.jpg");
        std::fs::write(&media_path, b"fake image data").unwrap();

        // Mock getuploadurl
        let cdn_url = format!("{}/cdn/upload?encrypted_param_123", server.url());
        let upload_mock = server
            .mock("POST", "/ilink/bot/getuploadurl")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "ret": 0,
                "cdnurl": cdn_url,
                "aes_key": "0123456789abcdef0123456789abcdef"
            }).to_string())
            .create();

        // Mock CDN upload (PUT)
        let cdn_mock = server
            .mock("PUT", "/cdn/upload")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .create();

        let result = process_media_upload(
            &client,
            "test-token",
            &media_path.to_string_lossy(),
        ).await;

        assert!(result.is_ok(), "upload should succeed: {:?}", result.err());
        let (item_type, encrypt_query_param, aes_key) = result.unwrap();
        assert_eq!(item_type, msg_type::IMAGE);
        assert_eq!(encrypt_query_param, "encrypted_param_123");
        assert_eq!(aes_key, "0123456789abcdef0123456789abcdef");

        upload_mock.assert();
        cdn_mock.assert();
    }
}
