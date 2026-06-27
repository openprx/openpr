use serde_json::Value;

use crate::error::ApiError;

pub struct ZipEntry {
    pub path: String,
    pub data: Vec<u8>,
}

pub fn sanitize_file_stem(value: &str) -> String {
    let stem = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if stem.is_empty() { "form".to_string() } else { stem }
}

pub fn attachment_package_artifact_key(stored_file_name: &str) -> String {
    format!("packages/{stored_file_name}")
}

pub fn sanitize_zip_file_name(value: &str) -> String {
    let name = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value)
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .replace("..", "_")
        .trim_matches(['.', '_'])
        .to_string();
    if name.is_empty() {
        "attachment".to_string()
    } else {
        name
    }
}

pub fn build_stored_zip(entries: &[ZipEntry]) -> Result<Vec<u8>, ApiError> {
    let mut output = Vec::new();
    let mut central_directory = Vec::new();
    for entry in entries {
        let path = entry.path.as_bytes();
        let name_len = checked_u16(path.len())?;
        let size = checked_u32(entry.data.len())?;
        let offset = checked_u32(output.len())?;
        let crc = crc32(&entry.data);

        write_u32(&mut output, 0x0403_4b50);
        write_u16(&mut output, 20);
        write_u16(&mut output, 0x0800);
        write_u16(&mut output, 0);
        write_u16(&mut output, 0);
        write_u16(&mut output, 33);
        write_u32(&mut output, crc);
        write_u32(&mut output, size);
        write_u32(&mut output, size);
        write_u16(&mut output, name_len);
        write_u16(&mut output, 0);
        output.extend_from_slice(path);
        output.extend_from_slice(&entry.data);

        write_u32(&mut central_directory, 0x0201_4b50);
        write_u16(&mut central_directory, 20);
        write_u16(&mut central_directory, 20);
        write_u16(&mut central_directory, 0x0800);
        write_u16(&mut central_directory, 0);
        write_u16(&mut central_directory, 0);
        write_u16(&mut central_directory, 33);
        write_u32(&mut central_directory, crc);
        write_u32(&mut central_directory, size);
        write_u32(&mut central_directory, size);
        write_u16(&mut central_directory, name_len);
        write_u16(&mut central_directory, 0);
        write_u16(&mut central_directory, 0);
        write_u16(&mut central_directory, 0);
        write_u16(&mut central_directory, 0);
        write_u32(&mut central_directory, 0);
        write_u32(&mut central_directory, offset);
        central_directory.extend_from_slice(path);
    }

    let central_directory_offset = checked_u32(output.len())?;
    let central_directory_size = checked_u32(central_directory.len())?;
    let entry_count = checked_u16(entries.len())?;
    output.extend_from_slice(&central_directory);
    write_u32(&mut output, 0x0605_4b50);
    write_u16(&mut output, 0);
    write_u16(&mut output, 0);
    write_u16(&mut output, entry_count);
    write_u16(&mut output, entry_count);
    write_u32(&mut output, central_directory_size);
    write_u32(&mut output, central_directory_offset);
    write_u16(&mut output, 0);
    Ok(output)
}

pub fn attachment_package_job_result_string(result: &Value, key: &str) -> Result<String, ApiError> {
    let value = result
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest(format!("attachment package job result {key} is missing")))?;
    if key == "stored_file_name"
        && (value.trim().is_empty() || value.contains('/') || value.contains('\\') || value.contains(".."))
    {
        return Err(ApiError::BadRequest(
            "attachment package job artifact reference is invalid".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn checked_u16(value: usize) -> Result<u16, ApiError> {
    u16::try_from(value).map_err(|_| ApiError::BadRequest("attachment package is too large".to_string()))
}

fn checked_u32(value: usize) -> Result<u32, ApiError> {
    u32::try_from(value).map_err(|_| ApiError::BadRequest("attachment package is too large".to_string()))
}

fn write_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xedb8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::{
        ZipEntry, attachment_package_job_result_string, build_stored_zip, sanitize_file_stem, sanitize_zip_file_name,
    };
    use serde_json::json;

    #[test]
    fn sanitizes_export_file_stem() {
        assert_eq!(sanitize_file_stem("order form/01"), "order_form_01");
        assert_eq!(sanitize_file_stem(""), "form");
    }

    #[test]
    fn validates_attachment_package_job_result_strings() {
        let result = json!({
            "stored_file_name": "4c4f73a9-package.zip",
            "file_name": "package.zip",
        });
        assert_eq!(
            attachment_package_job_result_string(&result, "stored_file_name").unwrap(),
            "4c4f73a9-package.zip"
        );
        assert_eq!(
            attachment_package_job_result_string(&result, "file_name").unwrap(),
            "package.zip"
        );

        let traversal = json!({"stored_file_name": "../package.zip"});
        assert!(attachment_package_job_result_string(&traversal, "stored_file_name").is_err());
        let nested = json!({"stored_file_name": "nested/package.zip"});
        assert!(attachment_package_job_result_string(&nested, "stored_file_name").is_err());
    }

    #[test]
    fn sanitizes_zip_file_names() {
        assert_eq!(sanitize_zip_file_name("../bad/report 01.pdf"), "report_01.pdf");
        assert_eq!(sanitize_zip_file_name(".."), "attachment");
        assert_eq!(sanitize_zip_file_name("@@@.png"), "png");
    }

    #[test]
    fn builds_stored_zip_with_manifest_and_file_entries() {
        let zip = build_stored_zip(&[
            ZipEntry {
                path: "manifest.json".to_string(),
                data: br#"{"ok":true}"#.to_vec(),
            },
            ZipEntry {
                path: "files/example.txt".to_string(),
                data: b"hello".to_vec(),
            },
        ])
        .expect("zip should build");

        assert!(zip.starts_with(&[0x50, 0x4b, 0x03, 0x04]));
        assert!(
            zip.windows("manifest.json".len())
                .any(|window| window == b"manifest.json")
        );
        assert!(
            zip.windows("files/example.txt".len())
                .any(|window| window == b"files/example.txt")
        );
        assert!(zip.ends_with(&[0, 0]));
    }
}
