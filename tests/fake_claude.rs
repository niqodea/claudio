//! The fake Claude Code writes what claudio assumes the real one writes, as far as the checks
//! that make no call to the API tell, so manual tests with it show what the real one would.

use std::path::Path;

#[test]
fn the_fake_passes_the_free_checks() {
    let fake = Path::new(env!("CARGO_BIN_EXE_fake-claude"));
    let failures: Vec<String> = claudio::contract::CHECKS
        .iter()
        .filter(|check| !check.is_paid)
        .filter_map(|check| {
            (check.run)(fake)
                .err()
                .map(|error| format!("{}: {error}", check.name))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
