//! The extension's files, built into the binary so that installing it needs
//! nothing beside the binary.
//!
//! Every file under `extension/` but its tests is here, unchanged; a test
//! walks the folder and fails if one is missing or differs.

/// One file of the extension, built in.
macro_rules! file {
    ($path:literal) => {
        (
            $path,
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/extension/", $path)) as &[u8],
        )
    };
}

/// Each file of the extension, by its path inside the extension's folder.
pub static FILES: &[(&str, &[u8])] = &[
    file!("bridge.js"),
    file!("connection.js"),
    file!("icons/icon128.png"),
    file!("icons/icon16.png"),
    file!("icons/icon48.png"),
    file!("manifest.json"),
    file!("policy.js"),
    file!("policy.json"),
    file!("popup.html"),
    file!("popup.js"),
    file!("popup-view.js"),
    file!("protocol.js"),
    file!("scripts.json"),
    file!("service-worker.js"),
];
