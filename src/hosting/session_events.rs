//! Record native session transitions without persisting wire payloads.
use serde_json::Value;

#[derive(Default)]
pub(super) struct Events {
    authenticated: Option<bool>,
    online: Option<bool>,
    logged_out: bool,
}

impl Events {
    pub fn observe(&mut self, event: &Value) -> Option<&'static str> {
        match event["messageType"].as_str()? {
            "UpdateUserInfo" => {
                let authenticated = event["payload"]["user"]["isAuthenticated"].as_bool()?;
                if self.authenticated == Some(authenticated) {
                    return None;
                }
                self.authenticated = Some(authenticated);
                if authenticated {
                    self.logged_out = false;
                    Some("Warcraft III confirmed Battle.net authentication.")
                } else {
                    self.online = None;
                    Some("Warcraft III authentication is pending.")
                }
            }
            "OnNetProviderInitialized" => {
                let online = event["payload"]["message"]["isOnline"].as_bool()?;
                if self.online == Some(online) {
                    return None;
                }
                self.online = Some(online);
                Some(if online {
                    "Warcraft III confirmed online multiplayer initialization."
                } else {
                    "Warcraft III multiplayer initialization returned offline."
                })
            }
            "LoggedOut" if !self.logged_out => {
                self.logged_out = true;
                self.authenticated = Some(false);
                self.online = None;
                Some("Warcraft III reported a Battle.net sign-out.")
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn auth(value: bool) -> Value {
        json!({"messageType":"UpdateUserInfo","payload":{"user":{
            "isAuthenticated":value,"battleTag":"private-account","token":"private-token"
        }}})
    }

    #[test]
    fn authentication_callbacks_are_deduplicated_without_losing_sign_out() {
        let mut events = Events::default();
        assert_eq!(
            events.observe(&auth(false)),
            Some("Warcraft III authentication is pending.")
        );
        assert_eq!(events.observe(&auth(false)), None);
        assert_eq!(
            events.observe(&auth(true)),
            Some("Warcraft III confirmed Battle.net authentication.")
        );
        assert_eq!(events.observe(&auth(true)), None);
        let logout = json!({"messageType":"LoggedOut","payload":{"account":"private-account"}});
        assert_eq!(
            events.observe(&logout),
            Some("Warcraft III reported a Battle.net sign-out.")
        );
        assert_eq!(events.observe(&logout), None);
        assert_eq!(events.observe(&auth(false)), None);
        assert!(events.observe(&auth(true)).is_some());
        assert!(events.observe(&logout).is_some());
    }

    #[test]
    fn provider_reports_are_distinct_from_authentication() {
        let mut events = Events::default();
        for (online, expected) in [
            (
                true,
                "Warcraft III confirmed online multiplayer initialization.",
            ),
            (
                false,
                "Warcraft III multiplayer initialization returned offline.",
            ),
        ] {
            let event = json!({"messageType":"OnNetProviderInitialized","payload":{"message":{"isOnline":online}}});
            assert_eq!(events.observe(&event), Some(expected));
            assert_eq!(events.observe(&event), None);
        }
        let ready = json!({"messageType":"OnNetProviderInitialized","payload":{"message":{"isOnline":true}}});
        assert!(events.observe(&ready).is_some());
        assert!(events.observe(&auth(false)).is_some());
        assert!(events.observe(&ready).is_some());
    }

    #[test]
    fn malformed_or_unrelated_payloads_never_become_log_text() {
        let mut events = Events::default();
        for event in [
            json!({"messageType":"ChatMessage","payload":"private message"}),
            json!({"messageType":"ShowModal","payload":{"message":"private message"}}),
            json!({"messageType":"UpdateUserInfo","payload":{"user":{"isAuthenticated":"private-token"}}}),
            json!({"messageType":"OnNetProviderInitialized","payload":{"message":{"isOnline":1}}}),
            Value::Null,
        ] {
            assert_eq!(events.observe(&event), None);
        }
        assert!(events.observe(&auth(true)).is_some());
    }
}
