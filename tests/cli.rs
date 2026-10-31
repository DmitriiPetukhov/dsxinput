use std::process::Command;

#[test]
fn debug_without_game_shows_usage_and_exits_two() {
    let output = Command::new(env!("CARGO_BIN_EXE_dsxinput"))
        .arg("--debug")
        .output()
        .expect("run dsxinput");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Usage:"));
    assert!(stderr.contains("--debug"));
}
