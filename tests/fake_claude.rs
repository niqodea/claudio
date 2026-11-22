//! The fake Claude Code writes what claudio assumes the real one writes, as far as the checks
//! that make no call to the API tell, so manual tests with it show what the real one would.

use std::path::Path;

#[test]
fn the_fake_passes_the_free_checks() {
    let fake = Path::new(env!("CARGO_BIN_EXE_fake-claude"));
    // A config directory of the test's own keeps the fake's copy of the usage out of the user's
    let config = tempfile::tempdir().expect("a temporary directory");
    // The test is alone in its binary, so no other thread reads the environment meanwhile
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", config.path()) };
    let failures: Vec<String> = claudio::contract::CHECKS
        .iter()
        .filter(|check| !check.setup.is_paid())
        .filter_map(|check| {
            check
                .setup
                .run(fake)
                .err()
                .map(|error| format!("{}: {error}", check.name))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
