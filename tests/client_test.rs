use semver::Version;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use tempfile::NamedTempFile;
use wasmedgeup::{
    api::{latest_installed_version, Asset, WasmEdgeApiClient},
    commands::install::InstallArgs,
    error::Error,
};

#[tokio::test]
async fn test_checksum_verification() {
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(b"test data").unwrap();
    temp_file.seek(SeekFrom::Start(0)).unwrap();

    let mut content = Vec::new();
    temp_file.as_file_mut().read_to_end(&mut content).unwrap();

    let mut hasher = Sha256::new();
    hasher.update(&content);
    let checksum = "916f0027a575074ce72a331777c3478d6513f786a591bd892da1a577bf2335f9"; // SHA256 of "test data"

    temp_file.seek(SeekFrom::Start(0)).unwrap();

    let verify_result =
        WasmEdgeApiClient::verify_file_checksum(temp_file.as_file_mut(), checksum).await;

    assert!(verify_result.is_ok(), "Checksum verification failed");

    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(b"different data").unwrap();
    temp_file.seek(SeekFrom::Start(0)).unwrap();

    let mut content = Vec::new();
    temp_file.as_file_mut().read_to_end(&mut content).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&content);

    temp_file.seek(SeekFrom::Start(0)).unwrap();

    let result = WasmEdgeApiClient::verify_file_checksum(temp_file.as_file_mut(), checksum).await;
    assert!(result.is_err());

    match result {
        Err(Error::ChecksumMismatch { expected, actual }) => {
            assert_eq!(expected, checksum);
            assert_eq!(
                actual,
                "608a068b33d18be838bcb07bed01e35521d30840fa24db09192e67bfd186e621"
            ); // Actual SHA256 of "different data"
        }

        _ => panic!("Expected ChecksumMismatch error"),
    }
}

#[test]
fn test_latest_installed_version_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let versions_dir = root.join("versions");
    std::fs::create_dir_all(versions_dir.join("0.9.0")).unwrap();
    std::fs::create_dir_all(versions_dir.join("0.15.0")).unwrap();
    std::fs::create_dir_all(versions_dir.join("not-a-version")).unwrap();

    let latest = latest_installed_version(&versions_dir).unwrap();
    assert_eq!(latest, Some(Version::parse("0.15.0").unwrap()));
}

#[test]
fn test_latest_installed_version_prerelease() {
    let tmp = tempfile::tempdir().unwrap();
    let versions_dir = tmp.path().join("versions");
    std::fs::create_dir_all(versions_dir.join("0.15.0-rc.1")).unwrap();
    std::fs::create_dir_all(versions_dir.join("0.15.0")).unwrap();

    let latest = latest_installed_version(&versions_dir).unwrap();
    assert_eq!(latest.unwrap(), Version::parse("0.15.0").unwrap());
}

#[tokio::test]
async fn test_get_release_checksum() {
    let client = WasmEdgeApiClient::default();
    let version = client.latest_release().unwrap();
    let mut args = InstallArgs {
        version: "latest".to_string(),
        path: None,
        tmpdir: None,
        os: None,
        arch: None,
    };
    let os = args.os.get_or_insert_default();
    let arch = args.arch.get_or_insert_default();
    let asset = Asset::new(&version, os, arch);

    let result = client.get_release_checksum(&version, &asset).await;
    assert!(result.is_ok(), "Failed to get checksum: {result:?}");

    let checksum = result.unwrap();
    assert!(!checksum.is_empty(), "Checksum should not be empty");
    assert_eq!(
        checksum.len(),
        64,
        "SHA256 checksum should be 64 characters long"
    );
    assert!(
        checksum.chars().all(|c| c.is_ascii_hexdigit()),
        "Checksum should be hexadecimal"
    );

    let invalid_version = Version::new(99, 99, 99);
    let result = client.get_release_checksum(&invalid_version, &asset).await;
    assert!(matches!(result, Err(Error::ChecksumNotFound { .. })));
}
