use super::*;

#[test]
fn a_reused_supervisor_pid_is_not_the_retained_controller() {
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let born = birth(unsafe { GetCurrentProcess() }).unwrap();
    let adopted = adopt(&executable, std::process::id(), timestamp(), Some(born - 1));
    assert!(matches!(adopted, Ok(None)));
}

#[test]
fn the_supervisor_cannot_adopt_itself_as_a_live_controller() {
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    let born = birth(unsafe { GetCurrentProcess() }).unwrap();
    for expected in [None, Some(born)] {
        assert!(adopt(&executable, std::process::id(), timestamp(), expected).is_err());
    }
}
