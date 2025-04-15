use semver::Version;
use wasmedgeup::api::{releases, ReleasesFilter};

const WASM_EDGE_GIT_URL: &str = "https://github.com/WasmEdge/WasmEdge.git";

#[test]
fn test_retrieves_released_versions_stable() {
    let retrieved = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::Stable).unwrap();

    let pos_0_14_1 = retrieved
        .iter()
        .position(|v| v == &Version::new(0, 14, 1))
        .unwrap();
    let pos_0_14_0 = retrieved
        .iter()
        .position(|v| v == &Version::new(0, 14, 0))
        .unwrap();
    assert!(pos_0_14_1 < pos_0_14_0);

    let pos_0_15_0_alpha_1 = retrieved
        .iter()
        .position(|v| v == &Version::parse("0.15.0-alpha.1").unwrap());
    assert!(pos_0_15_0_alpha_1.is_none());

    let pos_0_14_1_rc_2 = retrieved
        .iter()
        .position(|v| v == &Version::parse("0.14.1-rc.2").unwrap());
    assert!(pos_0_14_1_rc_2.is_none());
}

#[test]
fn test_retrieves_released_versions_all() {
    let retrieved = releases::get_all(WASM_EDGE_GIT_URL, ReleasesFilter::All).unwrap();

    let pos_0_15_0_alpha_1 = retrieved
        .iter()
        .position(|v| v == &Version::parse("0.15.0-alpha.1").unwrap())
        .unwrap();
    let pos_0_14_1 = retrieved
        .iter()
        .position(|v| v == &Version::new(0, 14, 1))
        .unwrap();
    let pos_0_14_1_rc_2 = retrieved
        .iter()
        .position(|v| v == &Version::parse("0.14.1-rc.2").unwrap())
        .unwrap();
    let pos_0_14_0 = retrieved
        .iter()
        .position(|v| v == &Version::new(0, 14, 0))
        .unwrap();

    assert!(pos_0_15_0_alpha_1 < pos_0_14_1);
    assert!(pos_0_14_1 < pos_0_14_1_rc_2);
    assert!(pos_0_14_1_rc_2 < pos_0_14_0);
}
