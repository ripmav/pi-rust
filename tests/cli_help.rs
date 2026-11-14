//! WP-907 acceptance: the CLI reference (`docs/cli-reference.md`) must match
//! the actual `pi-rust --help` output byte for byte — a flag added to the
//! parser without a doc update (or vice versa) fails this test.

#[test]
fn cli_reference_matches_help_output() {
    let bin = env!("CARGO_BIN_EXE_pi-rust");
    let out = std::process::Command::new(bin)
        .arg("--help")
        .output()
        .expect("spawn pi-rust --help");
    assert!(out.status.success(), "--help must exit 0");
    let help = String::from_utf8_lossy(&out.stdout).into_owned();

    let doc = include_str!("../docs/cli-reference.md");
    let mut in_fence = false;
    let mut fence = String::new();
    for line in doc.lines() {
        if line.trim_start().starts_with("```") {
            if in_fence {
                break;
            }
            in_fence = true;
            continue;
        }
        if in_fence {
            fence.push_str(line);
            fence.push('\n');
        }
    }
    assert!(
        !fence.is_empty(),
        "docs/cli-reference.md must contain a fenced usage block"
    );
    assert_eq!(
        help, fence,
        "pi-rust --help drifted from docs/cli-reference.md — update the doc fence (or the usage text in src/cli.rs)"
    );
}
