use std::thread;
use std::time::{Duration, Instant};

// The stop flag is process-wide, so this test has its own binary to run alone.
#[test]
fn esc_discards_a_running_turn_and_stops_the_shell() {
    let started = Instant::now();
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(60));
        converge::cancel_shell();
    });
    let result = converge::wait_for_turn(|| {
        thread::sleep(Duration::from_millis(400));
        "late"
    })
    .unwrap();
    assert!(result.is_none());
    assert!(started.elapsed() < Duration::from_millis(350));
    converge::clear_shell_cancel();

    thread::spawn(|| {
        thread::sleep(Duration::from_millis(60));
        converge::cancel_shell();
    });
    let cwd = std::env::temp_dir();
    let error = match converge::run_shell_timeout(&cwd, "sleep 30", Duration::from_secs(5)) {
        Ok(_) => panic!("sleep should be cancelled"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("cancelled"));
    converge::clear_shell_cancel();
}
