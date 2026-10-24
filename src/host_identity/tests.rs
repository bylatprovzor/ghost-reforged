use super::*;
use serde_json::json;
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct State {
    active: Cell<bool>,
    count: Cell<u64>,
    installations: Cell<u32>,
}
struct Fake(Rc<State>);
impl SendFilter for Fake {
    fn install(&mut self) -> Result<(), String> {
        self.0.active.set(true);
        self.0.count.set(0);
        self.0.installations.set(self.0.installations.get() + 1);
        Ok(())
    }
    fn count(&self) -> Result<u64, String> {
        Ok(self.0.count.get())
    }
    fn restore(&mut self) -> Result<(), String> {
        self.0.active.set(false);
        Ok(())
    }
}
fn fixture() -> (Identity<Fake>, Rc<State>) {
    let state = Rc::new(State::default());
    (
        Identity {
            filter: Fake(state.clone()),
            creating: None,
        },
        state,
    )
}
fn lobby() -> Value {
    json!({"isHost":true,"playerHost":"","players":[{"isSelf":true,"name":""}]})
}

#[test]
fn creation_requires_one_omitted_profile_and_restores_before_becoming_ready() {
    let (mut identity, state) = fixture();
    for _ in 0..3 {
        identity.begin_creation().unwrap();
        assert!(state.active.get());
        assert!(!identity.confirm(&lobby()).unwrap());
        assert!(
            state.active.get(),
            "A placeholder must not disarm the filter"
        );
        state.count.set(1);
        assert!(identity.confirm(&lobby()).unwrap());
        assert!(!state.active.get());
        assert!(identity.confirm(&lobby()).unwrap());
    }
    assert_eq!(state.installations.get(), 3);
}

#[test]
fn named_lobbies_missing_names_and_ambiguous_self_are_never_accepted_as_blank() {
    let mut cases = vec![json!({}), lobby(), lobby(), lobby(), lobby()];
    cases[1]["playerHost"] = json!("Owner");
    cases[2]["players"][0]["name"] = json!("Owner");
    cases[3]["players"][0]
        .as_object_mut()
        .unwrap()
        .remove("name");
    cases[4]["players"]
        .as_array_mut()
        .unwrap()
        .push(json!({"isSelf":true,"name":""}));
    for value in cases {
        let (mut identity, state) = fixture();
        identity.begin_creation().unwrap();
        state.count.set(1);
        assert!(identity.confirm(&value).is_err());
        assert!(!state.active.get());
    }
}

#[test]
fn cancellation_timeout_error_and_drop_restore_publication() {
    for reason in 0..4 {
        let (mut identity, state) = fixture();
        identity.begin_creation().unwrap();
        match reason {
            0 => identity.cancel_creation().unwrap(),
            1 => {
                identity.creating = Some(Instant::now() - Duration::from_secs(31));
                assert!(identity.check_timeout(Duration::from_secs(30)).is_err());
            }
            2 => {
                state.count.set(2);
                assert!(identity.confirm(&lobby()).is_err());
            }
            _ => drop(identity),
        }
        assert!(!state.active.get());
    }
}

#[test]
fn resuming_a_blank_lobby_does_not_install_a_filter() {
    let (mut identity, state) = fixture();
    assert!(identity.confirm(&lobby()).unwrap());
    assert_eq!(state.installations.get(), 0);
}

#[test]
fn slow_creation_keeps_identity_protection_until_the_configured_creation_deadline() {
    let (mut identity, state) = fixture();
    let budget = Duration::from_secs(120);
    identity.begin_creation().unwrap();
    identity.creating = Some(Instant::now() - Duration::from_secs(61));
    assert!(identity.check_timeout(budget).is_ok());
    assert!(state.active.get());
    state.count.set(1);
    assert!(identity.confirm(&lobby()).unwrap());
    assert!(!state.active.get());

    identity.begin_creation().unwrap();
    identity.creating = Some(Instant::now() - Duration::from_secs(121));
    assert!(identity.check_timeout(budget).is_err());
    assert!(!state.active.get());
}

#[test]
fn native_blank_header_omission_still_requires_an_explicit_empty_self_name() {
    let (mut identity, state) = fixture();
    identity.begin_creation().unwrap();
    state.count.set(1);
    let mut value = lobby();
    value.as_object_mut().unwrap().remove("playerHost");
    assert!(identity.confirm(&value).unwrap());
    assert!(!state.active.get());
    value["players"][0].as_object_mut().unwrap().remove("name");
    assert!(!Identity::<Fake>::matches(&value));
}
